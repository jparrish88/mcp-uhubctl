//! uhubctl runner: list hubs, resolve board identities to (hub, port),
//! set power (duality handling ON by default — required for VBUS cut on
//! USB3 hubs), and verify presence via lsusb.
//!
//! Identity model (multi-device benches):
//! - every physical board has a USB serial (PPK2 `D4184E334B85`, J-Link
//!   `001160003881`, …) visible in the uhubctl listing, `lsusb -v`, and
//!   `/dev/serial/by-id/`;
//! - a `target` argument resolves in order: config nickname -> legacy short
//!   name (`ppk2`/`jlink`/`sniffer`, must be unique on the bus) -> serial
//!   substring (must be unique);
//! - hub:port is always *discovered* from a fresh listing, never trusted from
//!   the caller, so a keyboard/mouse hub (no allowlisted device) can never be
//!   addressed, and duplicates are an error, never a guess.
//!
//! Safety model: power tools only accept the resolved identities above. Raw
//! location:port operation is intentionally CLI-only.

use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::process::Command;
use std::thread;
use std::time::Duration;

/// Friendly name -> USB VID:PID of the attached board (legacy unique names).
fn short_name_vidpid(name: &str) -> Option<&'static str> {
    match name {
        "ppk2" => Some("1915:c00a"),
        "jlink" | "segger" | "apollo" | "apollo510b" => Some("1366:1024"),
        "sniffer" | "nrf" | "nrf-sniffer" => Some("1915:522a"),
        _ => None,
    }
}

/// One nicknamed board from the config file.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BoardEntry {
    /// USB VID:PID, e.g. "1366:1024".
    pub vidpid: String,
    /// USB serial substring, e.g. "001160003881". Omit only if unique on bus.
    #[serde(default)]
    pub serial: Option<String>,
    /// SEGGER J-Link nickname (assigned once via J-Link Configurator, shown
    /// by `JLinkExe ShowEmuList`). Alternative to `serial` for J-Links:
    /// resolved live, so a replacement probe keeps workingId with zero config
    /// as long as it carries the same SEGGER nickname.
    #[serde(default)]
    pub jlink_nickname: Option<String>,
}

/// A resolved addressing target.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// Stable key for state + reports (nickname, or vidpid[:serial]).
    pub key: String,
    pub vidpid: String,
    pub serial: Option<String>,
    /// SEGGER nickname to resolve live (see [`materialize_target`]).
    pub jlink_nickname: Option<String>,
}

fn boards_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("BOARDS_PATH") {
        return std::path::PathBuf::from(p);
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            std::path::PathBuf::from(home).join(".config")
        });
    base.join("mcp-uhubctl/boards.json")
}

/// Load nickname -> board map. Missing/unreadable file means "no nicknames",
/// never an error (short names and serials still work).
pub fn load_boards_from(path: &std::path::Path) -> HashMap<String, BoardEntry> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        boards: HashMap<String, BoardEntry>,
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<File>(&s).ok())
        .map(|f| {
            f.boards
                .into_iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v))
                .collect()
        })
        .unwrap_or_default()
}

pub fn load_boards() -> HashMap<String, BoardEntry> {
    load_boards_from(&boards_path())
}

/// Resolve a target string with an explicit board map (pure, testable).
pub fn resolve_target_with(
    target: &str,
    boards: &HashMap<String, BoardEntry>,
) -> Result<ResolvedTarget> {
    let t = target.trim();
    if t.is_empty() {
        return Err(anyhow!("empty target"));
    }
    let lower = t.to_ascii_lowercase();
    if let Some(b) = boards.get(&lower) {
        return Ok(ResolvedTarget {
            key: lower,
            vidpid: b.vidpid.to_ascii_lowercase(),
            serial: b.serial.clone(),
            jlink_nickname: b.jlink_nickname.clone(),
        });
    }
    if let Some(vidpid) = short_name_vidpid(&lower) {
        return Ok(ResolvedTarget {
            key: lower,
            vidpid: vidpid.to_string(),
            serial: None,
            jlink_nickname: None,
        });
    }
    // Otherwise a USB serial substring (must be plausibly serial-like).
    if lower.len() >= 4 && lower.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(ResolvedTarget {
            key: format!("serial:{t}"),
            vidpid: String::new(),
            serial: Some(t.to_string()),
            jlink_nickname: None,
        });
    }
    Err(anyhow!(
        "unknown target {target:?} — use a boards.json nickname, ppk2/jlink/sniffer, or a USB serial substring (see usb_targets)"
    ))
}

/// Resolve a target string using the on-disk nickname config.
pub fn resolve_target(target: &str) -> Result<ResolvedTarget> {
    resolve_target_with(target, &load_boards())
}

fn uhubctl_bin() -> String {
    std::env::var("UHUBCTL_BIN").unwrap_or_else(|_| "uhubctl".to_string())
}

fn lsusb_bin() -> String {
    std::env::var("LSUSB_BIN").unwrap_or_else(|_| "lsusb".to_string())
}

fn jlinkexe_bin() -> String {
    std::env::var("JLINKEXE_BIN").unwrap_or_else(|_| "JLinkExe".to_string())
}

/// One J-Link seen by the SEGGER software.
#[derive(Debug, Clone, PartialEq)]
pub struct JlinkEmu {
    /// Serial as reported by JLinkExe (leading zeros stripped, e.g. "1160003881").
    pub serial: String,
    pub product: String,
    /// None when `<not set>`.
    pub nickname: Option<String>,
}

/// Parse `JLinkExe ShowEmuList` output into emulator entries. Pure, tested.
pub fn parse_emu_list(output: &str) -> Vec<JlinkEmu> {
    let mut emus = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        // "J-Link[0]: Connection: USB, Serial number: 1160003881,
        //  ProductName: J-Link-OB-Apollo4-CortexM, Nickname: <not set>"
        let Some(body) = t.split_once("Serial number:") else {
            continue;
        };
        let serial = body.1.split(',').next().unwrap_or("").trim().to_string();
        let product = body
            .1
            .split("ProductName:")
            .nth(1)
            .unwrap_or("")
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let nick = body
            .1
            .split("Nickname:")
            .nth(1)
            .unwrap_or("")
            .trim()
            .to_string();
        if serial.is_empty() {
            continue;
        }
        emus.push(JlinkEmu {
            serial,
            product,
            nickname: if nick.is_empty() || nick == "<not set>" {
                None
            } else {
                Some(nick)
            },
        });
    }
    emus
}

/// Run ShowEmuList and return the parsed emulator list.
pub fn jlink_emu_list_with(bin: &str) -> Result<Vec<JlinkEmu>> {
    // JLinkExe only accepts a real script file (/dev/stdin probing returned
    // nothing), so stage one in temp and remove it afterwards.
    let script_path =
        std::env::temp_dir().join(format!("mcp-uhubctl-{}.jlink", std::process::id()));
    std::fs::write(&script_path, "ShowEmuList\nExit\n").context("temp script write failed")?;
    let script_arg = script_path.to_string_lossy().into_owned();
    let out = Command::new(bin)
        .args(["-NoGui", "1", "-CommanderScript", &script_arg])
        .output()
        .context("failed to spawn JLinkExe (is the J-Link software installed?)");
    let _ = std::fs::remove_file(&script_path);
    Ok(parse_emu_list(&String::from_utf8_lossy(&out?.stdout)))
}

pub fn jlink_emu_list() -> Result<Vec<JlinkEmu>> {
    jlink_emu_list_with(&jlinkexe_bin())
}

/// Map a SEGGER nickname to the USB serial (zero-padded form) of the probe
/// carrying it. Comparison is exact, then case-insensitive; numeric-only
/// nicknames are refused (SEGGER tools read those as serial numbers).
pub fn jlink_serial_for_nickname_with(bin: &str, nickname: &str) -> Result<String> {
    if nickname.len() > 32 {
        return Err(anyhow!("nickname exceeds SEGGER 32-char limit"));
    }
    if nickname.chars().all(|c| c.is_ascii_digit()) {
        return Err(anyhow!(
            "nickname {nickname:?} is numeric-only — SEGGER tools read that as a serial number; add a letter"
        ));
    }
    let emus = jlink_emu_list_with(bin)?;
    if let Some(e) = emus
        .iter()
        .find(|e| e.nickname.as_deref() == Some(nickname))
    {
        return Ok(e.serial.clone());
    }
    let lower = nickname.to_ascii_lowercase();
    if let Some(e) = emus.iter().find(|e| {
        e.nickname
            .as_deref()
            .is_some_and(|n| n.to_ascii_lowercase() == lower)
    }) {
        return Ok(e.serial.clone());
    }
    let have: Vec<String> = emus
        .iter()
        .map(|e| {
            format!(
                "{} ({})",
                e.nickname.as_deref().unwrap_or("<not set>"),
                e.serial
            )
        })
        .collect();
    Err(anyhow!(
        "no J-Link carries nickname {nickname:?}; seen: {}",
        have.join(", ")
    ))
}

pub fn jlink_serial_for_nickname(nickname: &str) -> Result<String> {
    jlink_serial_for_nickname_with(&jlinkexe_bin(), nickname)
}

/// Fill in a resolved target's USB serial from its SEGGER nickname, if any.
/// USB serials are zero-padded (`001160003881`) while JLinkExe strips leading
/// zeros (`1160003881`); matching strips zeros on both sides.
pub fn materialize_target(rt: &ResolvedTarget) -> Result<ResolvedTarget> {
    if let Some(nick) = &rt.jlink_nickname {
        let serial = jlink_serial_for_nickname(nick)?;
        let mut rt = rt.clone();
        rt.serial = Some(serial);
        return Ok(rt);
    }
    Ok(rt.clone())
}

/// Run uhubctl with args, return combined stdout+stderr. Non-zero exit is an error.
pub fn run_uhubctl_with(bin: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .context("failed to spawn uhubctl (is it installed? see README)")?;
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        s.push_str("\n[stderr]\n");
        s.push_str(err.trim());
    }
    if !out.status.success() {
        return Err(anyhow!("uhubctl failed:\n{s}"));
    }
    Ok(s)
}

pub fn run_uhubctl(args: &[&str]) -> Result<String> {
    run_uhubctl_with(&uhubctl_bin(), args)
}

/// Full hub listing (read-only).
pub fn list() -> Result<String> {
    run_uhubctl(&[])
}

/// A located board port discovered from a listing.
#[derive(Debug)]
pub struct FoundPort {
    pub location: String,
    pub port: String,
    pub hub_desc: String,
    pub port_line: String,
}

/// Find the hub+port whose attached-device description contains `vidpid`
/// (when non-empty) and `serial` (when given). Missing = error, ambiguous =
/// error listing candidates with serials so the caller can disambiguate.
pub fn find_device(list_output: &str, vidpid: &str, serial: Option<&str>) -> Result<FoundPort> {
    let vp = vidpid.to_ascii_lowercase();
    let sn = serial.map(|s| s.to_ascii_lowercase());
    let mut current_loc = String::new();
    let mut current_desc = String::new();
    let mut hits: Vec<FoundPort> = Vec::new();

    for line in list_output.lines() {
        let t = line.trim();
        if let Some(rest) = t
            .strip_prefix("Current status for hub ")
            .or_else(|| t.strip_prefix("New status for hub "))
        {
            let (loc, desc) = rest.split_once(' ').unwrap_or((rest, ""));
            current_loc = loc.to_string();
            current_desc = desc.to_string();
            continue;
        }
        if let Some(rest) = t.strip_prefix("Port ") {
            // "1: 0103 power enable connect [1915:c00a ... serial]"
            // Only match the attached-device bracket, not hub flags.
            let bracket = rest.split('[').nth(1).unwrap_or("");
            let b = bracket.to_ascii_lowercase();
            let vp_ok = vp.is_empty() || b.contains(&vp);
            let sn_ok = sn.as_ref().is_none_or(|s| b.contains(s));
            if vp_ok && sn_ok && (!vp.is_empty() || sn.is_some()) {
                let port = rest.split(':').next().unwrap_or("").trim().to_string();
                hits.push(FoundPort {
                    location: current_loc.clone(),
                    port,
                    hub_desc: current_desc.clone(),
                    port_line: t.to_string(),
                });
            }
        }
    }

    match hits.len() {
        0 => Err(anyhow!(
            "no port matches vidpid={vidpid:?} serial={serial:?}"
        )),
        1 => Ok(hits.into_iter().next().unwrap()),
        _ => {
            let list = hits
                .iter()
                .map(|h| format!("{}:{} [{}]", h.location, h.port, h.port_line))
                .collect::<Vec<_>>()
                .join("; ");
            Err(anyhow!(
                "ambiguous match ({list}) — use a nickname or fuller serial"
            ))
        }
    }
}

/// Set power on a located port. Duality handling stays ON (no -e): on USB3
/// hubs VBUS only drops when both the USB2 and USB3 sides go off (verified
/// on VIA 2109:2817/0817 with PPK2 LEDs). Returns the raw uhubctl output.
pub fn set_power_with(bin: &str, location: &str, port: &str, action: &str) -> Result<String> {
    run_uhubctl_with(bin, &["-l", location, "-p", port, "-a", action])
}

pub fn set_power(location: &str, port: &str, action: &str) -> Result<String> {
    set_power_with(&uhubctl_bin(), location, port, action)
}

/// True when `lsusb -d VID:PID` reports the device.
pub fn usb_present_with(bin: &str, vidpid: &str) -> bool {
    Command::new(bin)
        .args(["-d", vidpid])
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

pub fn usb_present(vidpid: &str) -> bool {
    usb_present_with(&lsusb_bin(), vidpid)
}

fn state_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            std::path::PathBuf::from(home).join(".local/share")
        });
    base.join("mcp-uhubctl/last.json")
}

/// Remember where a target was last seen (so power_on works while it is off
/// and absent from the listing).
fn save_last(key: &str, location: &str, port: &str) {
    let path = state_path();
    let mut map: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    map.insert(
        key.to_ascii_lowercase(),
        serde_json::json!({"location": location, "port": port}),
    );
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, serde_json::to_string(&map).unwrap_or_default());
}

fn load_last(key: &str) -> Option<(String, String)> {
    let map: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())?;
    let v = map.get(key.to_ascii_lowercase().as_str())?;
    Some((
        v.get("location")?.as_str()?.to_string(),
        v.get("port")?.as_str()?.to_string(),
    ))
}

/// VID:PID of the device attached to a port, taken from the bracket in a
/// `Port N: ... [vidpid vendor product serial]` listing line.
pub fn attached_vidpid(port_line: &str) -> Option<String> {
    let bracket = port_line.split('[').nth(1)?;
    bracket.split_whitespace().next().map(|s| s.to_string())
}

/// Presence-check id for a resolved target: the VID:PID when known, else the
/// VID:PID read off the discovered port line (a raw serial is not a valid
/// `lsusb -d` argument).
fn verify_id(rt: &ResolvedTarget, found: &FoundPort) -> String {
    if rt.vidpid.is_empty() {
        attached_vidpid(&found.port_line).unwrap_or_default()
    } else {
        rt.vidpid.clone()
    }
}

fn sleep_s(s: u64) {
    thread::sleep(Duration::from_secs(s));
}

fn locate(rt: &ResolvedTarget, listing: &str) -> Result<FoundPort> {
    find_device(listing, &rt.vidpid, rt.serial.as_deref())
}

/// Power-cycle a board: off, wait `off_seconds`, on, wait for re-enumeration,
/// verify presence. Returns a human-readable report.
pub fn power_cycle_with(
    target: &str,
    off_seconds: u64,
    ubin: &str,
    lbin: &str,
    boards: &HashMap<String, BoardEntry>,
    list_override: Option<&str>,
) -> Result<String> {
    if !(1..=120).contains(&off_seconds) {
        return Err(anyhow!("off_seconds must be in 1..=120"));
    }
    let rt = resolve_target_with(target, boards)?;
    let rt = materialize_target(&rt)?;
    let listing = match list_override {
        Some(s) => s.to_string(),
        None => run_uhubctl_with(ubin, &[])?,
    };
    let found = locate(&rt, &listing)?;
    save_last(&rt.key, &found.location, &found.port);
    let id = verify_id(&rt, &found);
    let mut report = format!(
        "target={} ({}) at {}:{} [{}]\n",
        rt.key, id, found.location, found.port, found.port_line
    );

    report.push_str(&format!(
        "--- off ---\n{}\n",
        set_power_with(ubin, &found.location, &found.port, "off")?
    ));
    sleep_s(off_seconds);
    let gone = !usb_present_with(lbin, &id);
    report.push_str(&format!(
        "after {off_seconds}s off: lsusb present={}\n",
        !gone
    ));

    report.push_str(&format!(
        "--- on ---\n{}\n",
        set_power_with(ubin, &found.location, &found.port, "on")?
    ));
    sleep_s(4);
    let back = usb_present_with(lbin, &id);
    report.push_str(&format!("after on + 4s: lsusb present={back}\n"));
    if !back {
        return Err(anyhow!(
            "{report}device did not re-enumerate — check cables/hub power"
        ));
    }
    Ok(report)
}

pub fn power_cycle(target: &str, off_seconds: u64) -> Result<String> {
    power_cycle_with(
        target,
        off_seconds,
        &uhubctl_bin(),
        &lsusb_bin(),
        &load_boards(),
        None,
    )
}

/// Power off (duality on) and verify the device leaves lsusb.
pub fn power_off(target: &str) -> Result<String> {
    let rt = materialize_target(&resolve_target(target)?)?;
    let found = locate(&rt, &list()?)?;
    save_last(&rt.key, &found.location, &found.port);
    let out = set_power(&found.location, &found.port, "off")?;
    sleep_s(2);
    let id = verify_id(&rt, &found);
    let gone = !usb_present(&id);
    Ok(format!(
        "target={} at {}:{}\n{out}\nlsusb present={} (want false)\n",
        rt.key, found.location, found.port, !gone
    ))
}

/// Power on (duality on) and verify the device re-enumerates. If the device is
/// currently off it is absent from the listing, so fall back to the last-known
/// location recorded by a previous off/cycle call.
pub fn power_on(target: &str) -> Result<String> {
    let rt = materialize_target(&resolve_target(target)?)?;
    let id_hint = if rt.vidpid.is_empty() {
        rt.serial.clone().unwrap_or_default()
    } else {
        rt.vidpid.clone()
    };
    let (loc, port, id) = match locate(&rt, &list()?) {
        Ok(f) => {
            let id = verify_id(&rt, &f);
            (f.location, f.port, id)
        }
        Err(_) => load_last(&rt.key)
            .map(|(l, p)| (l, p, id_hint))
            .ok_or_else(|| {
                anyhow!(
                    "device {} not in listing and no last-known location — \
                 power it on once via uhubctl CLI (-l/-p), then use these tools",
                    rt.key
                )
            })?,
    };
    let out = set_power(&loc, &port, "on")?;
    sleep_s(4);
    let back = usb_present(&id);
    Ok(format!(
        "target={} at {loc}:{port}\n{out}\nlsusb present={back} (want true)\n",
        rt.key
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "\
Current status for hub 1-11 [2109:2817 VIA Labs, Inc. USB2.0 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0103 power enable connect [1915:c00a Nordic Semiconductor PPK2 D4184E334B85]
  Port 2: 0100 power
  Port 3: 0507 power highspeed suspend enable connect [2109:2817 VIA Labs, Inc. USB2.0 Hub, USB 2.10, 4 ports, ppps]
  Port 4: 1103 power indicator enable connect [1366:1024 SEGGER J-Link_Apollo510B_r2.0 001160002965]
Current status for hub 1-12 [2109:2817 VIA Labs, Inc. USB2.0 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0103 power enable connect [1366:1024 SEGGER J-Link 001160003881]
  Port 2: 0103 power enable connect [1915:c00a Nordic Semiconductor PPK2 AABBCCDDEEFF]
";

    #[test]
    fn find_unique_by_vidpid() {
        let f = find_device(LISTING, "1915:522a", None);
        assert!(f.is_err()); // sniffer absent
        // PPK2 vidpid alone is ambiguous here (two PPK2s) — must fail, not guess.
        assert!(find_device(LISTING, "1915:c00a", None).is_err());
    }

    #[test]
    fn find_by_vidpid_plus_serial() {
        let f = find_device(LISTING, "1915:c00a", Some("d4184e334b85")).unwrap();
        assert_eq!((f.location.as_str(), f.port.as_str()), ("1-11", "1"));
        let g = find_device(LISTING, "1366:1024", Some("001160003881")).unwrap();
        assert_eq!((g.location.as_str(), g.port.as_str()), ("1-12", "1"));
    }

    #[test]
    fn find_by_serial_only() {
        assert!(find_device("", "", None).is_err());
        let _ = find_device(LISTING, "", Some("001160002965")).unwrap();
    }

    #[test]
    fn bracket_scoping_avoids_hub_text() {
        // "2109:2817" appears in hub descs AND attached descs; attached-only
        // matching must still locate the downstream hub port.
        let f = find_device(LISTING, "2109:2817", None).unwrap();
        assert_eq!((f.location.as_str(), f.port.as_str()), ("1-11", "3"));
    }

    #[test]
    fn resolve_order_nickname_short_serial() {
        let mut boards = HashMap::new();
        boards.insert(
            "apollo510b".to_string(),
            BoardEntry {
                vidpid: "1366:1024".to_string(),
                serial: Some("001160002965".to_string()),
                jlink_nickname: None,
            },
        );
        let r = resolve_target_with("apollo510b", &boards).unwrap();
        assert_eq!(r.serial.as_deref(), Some("001160002965"));
        let r = resolve_target_with("PPK2", &boards).unwrap();
        assert_eq!(r.vidpid, "1915:c00a");
        assert!(r.serial.is_none());
        let r = resolve_target_with("001160003881", &boards).unwrap();
        assert_eq!(r.serial.as_deref(), Some("001160003881"));
        assert!(resolve_target_with("nope!", &boards).is_err());
    }

    #[test]
    fn load_boards_missing_file_is_empty() {
        let m = load_boards_from(std::path::Path::new("/nonexistent/boards.json"));
        assert!(m.is_empty());
    }

    #[test]
    fn attached_vidpid_parses_bracket() {
        assert_eq!(
            attached_vidpid(
                "Port 4: 1103 power indicator enable connect [1366:1024 SEGGER J-Link 001160003881]"
            )
            .as_deref(),
            Some("1366:1024")
        );
        assert_eq!(attached_vidpid("Port 2: 0100 power"), None);
    }

    const EMU_LIST: &str = "\
J-Link Command File read successfully.
Processing script file...
J-Link>ShowEmuList
J-Link[0]: Connection: USB, Serial number: 1160002965, ProductName: J-Link-OB-Test, Nickname: apollo510b
J-Link[1]: Connection: USB, Serial number: 1160003881, ProductName: J-Link-OB-Apollo4-CortexM, Nickname: <not set>
J-Link>Exit
Script processing completed.";

    #[test]
    fn parse_emu_list_reads_nicknames() {
        let emus = parse_emu_list(EMU_LIST);
        assert_eq!(emus.len(), 2);
        assert_eq!(emus[0].serial, "1160002965");
        assert_eq!(emus[0].nickname.as_deref(), Some("apollo510b"));
        assert_eq!(emus[1].nickname, None);
    }

    #[test]
    fn nickname_serial_matches_zero_padded_usb() {
        // JLinkExe strips leading zeros; USB keeps them. Substring matching
        // must bridge both directions.
        assert!("001160003881".contains("1160003881"));
        let f = find_device(LISTING, "1366:1024", Some("1160003881")).unwrap();
        assert_eq!(f.port.as_str(), "1");
    }
}
