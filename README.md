# mcp-uhubctl

An [MCP](https://modelcontextprotocol.io) server wrapping
[`uhubctl`](https://github.com/mvp/uhubctl) for USB per-port power control of
lab boards — built so a stuck PPK2 / J-Link / nRF Sniffer can be power-cycled
from chat. Follows the same Rust + [`rmcp`](https://crates.io/crates/rmcp)
stdio pattern as `mcp-nordic-ppk2`.

## Tools

| Tool | Arguments | Description |
|------|-----------|-------------|
| `usb_list` | — | List all uhubctl-compatible hubs + per-port state (read-only). |
| `usb_targets` | — | Configured nicknames + attached allowlisted boards with serials (read-only discovery for agents). |
| `usb_status` | `target` | Board's current hub:port, port state, lsusb presence (read-only). |
| `usb_power_off` | `target` | Port off (USB2+USB3 sides), verify it leaves lsusb. |
| `usb_power_on` | `target` | Port on (USB2+USB3 sides), verify re-enumeration. |
| `usb_power_cycle` | `target`, `off_seconds?` (1–120, default 5) | Off, hold, on, verify back in lsusb. The stuck-board tool. |

Targets are friendly names only: `ppk2` (`1915:c00a`), `jlink`
(`1366:1024`, also `segger`/`apollo`), `sniffer` (`1915:522a`).

## Multi-device benches: nicknames and serials

Every board has a USB serial — uhubctl prints it (`PPK2 D4184E334B85`,
`J-Link 001160003881`), and it is also in `lsusb -v` (`iSerial`) and
`/dev/serial/by-id/`. A `target` resolves in order:

1. **nickname** from `boards.json` (exact board even with duplicates),
2. **short name** (`ppk2`/`jlink`/`sniffer` — must be unique on the bus),
3. **USB serial substring** (must be unique).

Ambiguity is an error listing the candidates — never a guess. `usb_targets`
shows configured nicknames plus every allowlisted board currently attached
with its serial, so an agent driving several boards (e.g. one PPK2 measuring
while two J-Links flash firmware) can pick exact targets:

```json
// ~/.config/mcp-uhubctl/boards.json (see boards.example.json)
{ "boards": {
    "apollo510b": { "vidpid": "1366:1024", "serial": "001160003881" },
    "apollo-spare": { "vidpid": "1366:1024", "serial": "001160002965" },
    "ppk2-main":  { "vidpid": "1915:c00a", "serial": "D4184E334B85" }
} }
```

Override the path with `--boards-path <path>` or `BOARDS_PATH`.

## Safety

- **Allowlist by VID:PID.** Hub:port is re-discovered from a fresh `uhubctl`
  listing on every call. Raw `location:port` operation is CLI-only and refused
  here, so a keyboard/mouse hub (which hosts no allowlisted device) can never
  be addressed. Duplicate matching devices are an error, not a guess.
- **Duality ON by default.** USB3 hubs share one VBUS across a USB2 + USB3
  port pair — both sides are always switched together. Verified on VIA
  `2109:2817`/`0817`: USB2-side-only off drops data but leaves PPK2 LEDs on;
  both sides off kills VBUS. (`-e` is never passed.)
- **Verify, don't assume.** Every power tool re-checks `lsusb -d VID:PID`
  afterwards and reports presence; `power_cycle` fails loudly if the board
  doesn't re-enumerate.

## Permissions (one-time sudo — answers "will this need sudo?")

`uhubctl` needs hub control transfers + Linux 6.0+ sysfs `disable` writes.
Two options; option 1 is set up here:

1. **udev rules (recommended, no runtime sudo).** Install the tailored rules:
   ```sh
   sudo cp udev/rules.d/52-usb.rules /etc/udev/rules.d/
   sudo udevadm trigger --attr-match=subsystem=usb
   ```
   Replug the hubs (or reboot), then verify `uhubctl` (no sudo) lists hubs.
   Rules cover `2109` (VIA bench hubs) + `1d6b` (root hubs, needed for USB3
   duality) and open the sysfs `disable` nodes.
2. Passwordless sudoers for one binary — not used here; an interactive
   password prompt would hang the server.

Without step 1, every tool fails with uhubctl's permissions error. The server
never runs `sudo` itself.

## Install

Prerequisites: Rust 1.88+, `uhubctl` built (`~/Projects/uhubctl`, `make`) and
installed on PATH:

```sh
cd ~/Projects/uhubctl && sudo make install   # -> /usr/sbin/uhubctl
cd ~/Projects/mcp-uhubctl
cargo install --path .                        # -> ~/.cargo/bin/mcp-uhubctl
```

`.mcp.json` registers the server (assumes `mcp-uhubctl` on PATH):

```json
{ "mcpServers": { "usb-power": { "command": "mcp-uhubctl" } } }
```

Override the uhubctl binary with `--uhubctl-path <path>` or `UHUBCTL_BIN`.

## Use

- *"The Apollo board is stuck — power-cycle the J-Link."* → `usb_power_cycle`
  with `target=jlink`, `off_seconds=5`.
- *"Is the PPK2 up?"* → `usb_status` with `target=ppk2`.

## Testing

```sh
cargo test   # unit (parser, resolution) + integration (stubbed uhubctl/lsusb)
cargo clippy --all-targets
cargo fmt --check
```

Hardware is stubbed: `tests/stubs/uhubctl-fake.sh` serves a canned
two-J-Link / two-PPK2 bench ( ambiguity, serial routing) and `lsusb-fake.sh`
answers presence from `$FAKE_PRESENT`. Override real binaries per call via
`UHUBCTL_BIN` / `LSUSB_BIN` env vars. CI (`.github/workflows/ci.yml`) runs
fmt, clippy, and tests on push/PR.

## Layout

- `src/hub.rs` — uhubctl runner, listing parser, nickname/short/serial
  resolution, off/on/cycle with lsusb verification, last-known-location
  state (`~/.local/share/mcp-uhubctl/`).
- `src/server.rs` — rmcp tool surface.
- `src/main.rs` — stdio entry point.
- `tests/` — stubbed-hardware integration tests.
- `udev/rules.d/52-usb.rules` — one-time permission setup.
- `boards.example.json` — nickname config template.

## License

MIT.
