//! End-to-end power_cycle against stubbed uhubctl/lsusb (no hardware).
//! FAKE_PRESENT drives the lsusb stub; the cycle flow is:
//! off -> absent check -> on -> present check. The stub always reports the
//! device present, so the "absent after off" line just records the stub's
//! answer — the assertions that matter are nickname/serial routing and the
//! re-enumeration success path.

use mcp_uhubctl::hub;
use std::collections::HashMap;

fn stubs() -> (String, String) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stubs");
    (
        format!("{dir}/uhubctl-fake.sh"),
        format!("{dir}/lsusb-fake.sh"),
    )
}

fn boards() -> HashMap<String, hub::BoardEntry> {
    let mut m = HashMap::new();
    m.insert(
        "apollo510b".to_string(),
        hub::BoardEntry {
            vidpid: "1366:1024".to_string(),
            serial: Some("001160002965".to_string()),
            jlink_nickname: None,
        },
    );
    m
}

#[test]
fn cycle_by_nickname_routes_to_serial_port() {
    let (ubin, lbin) = stubs();
    // SAFETY: test-only env mutation for the stub subprocess (not read by test threads).
    unsafe { std::env::set_var("FAKE_PRESENT", "1366:1024") };
    let report = hub::power_cycle_with("apollo510b", 1, &ubin, &lbin, &boards(), None).unwrap();
    assert!(report.contains("1-11:4"), "wrong port routed:\n{report}");
    assert!(
        report.contains("lsusb present=true"),
        "no verify:\n{report}"
    );
}

#[test]
fn cycle_by_serial_picks_second_jlink() {
    let (ubin, lbin) = stubs();
    unsafe { std::env::set_var("FAKE_PRESENT", "1366:1024") };
    let report = hub::power_cycle_with("001160003881", 1, &ubin, &lbin, &boards(), None).unwrap();
    assert!(report.contains("1-12:1"), "wrong J-Link routed:\n{report}");
}

#[test]
fn ambiguous_short_name_fails_not_guesses() {
    let (ubin, lbin) = stubs();
    // Two PPK2s on the fake bench: bare "ppk2" must refuse.
    let err = hub::power_cycle_with("ppk2", 1, &ubin, &lbin, &boards(), None).unwrap_err();
    assert!(format!("{err:#}").contains("ambiguous"));
}

#[test]
fn unknown_target_is_rejected() {
    let (ubin, lbin) = stubs();
    let err = hub::power_cycle_with("microwave!", 1, &ubin, &lbin, &boards(), None).unwrap_err();
    assert!(format!("{err:#}").contains("unknown target"));
}

fn boards_nick() -> HashMap<String, hub::BoardEntry> {
    let mut m = HashMap::new();
    m.insert(
        "fonz".to_string(),
        hub::BoardEntry {
            vidpid: "1366:1024".to_string(),
            serial: None,
            jlink_nickname: Some("apollo510b".to_string()),
        },
    );
    m
}

fn jlink_stub() -> String {
    format!(
        "{}/tests/stubs/jlinkexe-fake.sh",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn cycle_by_segger_nickname_resolves_live_serial() {
    let (ubin, lbin) = stubs();
    // SAFETY: only this test reads JLINKEXE_BIN (its boards carry nicknames).
    unsafe { std::env::set_var("JLINKEXE_BIN", jlink_stub()) };
    unsafe { std::env::set_var("FAKE_PRESENT", "1366:1024") };
    let report = hub::power_cycle_with("fonz", 1, &ubin, &lbin, &boards_nick(), None).unwrap();
    // apollo510b -> JLink serial 1160002965 -> USB 001160002965 on 1-11:4.
    assert!(report.contains("1-11:4"), "wrong port routed:\n{report}");
    assert!(
        report.contains("lsusb present=true"),
        "no verify:\n{report}"
    );
}

#[test]
fn uhubctl_failure_propagates() {
    let (_, lbin) = stubs();
    let err = hub::run_uhubctl_with(
        &format!("{}/tests/stubs/uhubctl-fake.sh", env!("CARGO_MANIFEST_DIR")),
        &["--fail"],
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("uhubctl failed"));
    let _ = lbin;
}
