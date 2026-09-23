//! rmcp stdio server exposing uhubctl power control as MCP tools.
//!
//! Power tools only accept friendly board names (`ppk2`, `jlink`, `sniffer`)
//! resolved to (hub, port) from a fresh `uhubctl` listing on every call —
//! raw location:port operation is CLI-only by design, so a keyboard/mouse hub
//! can never be addressed. Blocking subprocess calls run on `spawn_blocking`.

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use serde::Deserialize;

/// MCP server for USB per-port power control.
pub struct UhubServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<UhubServer>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TargetArgs {
    /// Board to act on: a boards.json nickname, a short name (ppk2, jlink, sniffer — must be unique on the bus), or a USB serial substring (must be unique). Use usb_targets to discover.
    pub target: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CycleArgs {
    /// Board to act on: nickname, short name, or USB serial substring.
    pub target: String,
    /// Seconds to hold power off before turning back on (1–120). Defaults to 5.
    #[serde(default)]
    pub off_seconds: Option<u64>,
}

impl UhubServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    async fn blocking<F, T>(&self, f: F) -> Result<T, McpError>
    where
        F: FnOnce() -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| McpError::internal_error(format!("worker panicked: {e}"), None))?
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))
    }
}

impl Default for UhubServer {
    fn default() -> Self {
        Self::new()
    }
}

fn text(s: String) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(s)])
}

#[tool_router]
impl UhubServer {
    #[tool(
        description = "List all uhubctl-compatible hubs and per-port power state (read-only). Use to find where boards are attached before any power action."
    )]
    async fn usb_list(&self) -> Result<CallToolResult, McpError> {
        let s = self.blocking(crate::hub::list).await?;
        Ok(text(s))
    }

    #[tool(
        description = "Report a board's current hub:port, port state, and lsusb presence (read-only). Target is a nickname, short name, or USB serial substring."
    )]
    async fn usb_status(
        &self,
        Parameters(args): Parameters<TargetArgs>,
    ) -> Result<CallToolResult, McpError> {
        let s = self
            .blocking(move || {
                let rt = crate::hub::resolve_target(&args.target)?;
                let listing = crate::hub::list()?;
                match crate::hub::find_device(&listing, &rt.vidpid, rt.serial.as_deref()) {
                    Ok(f) => {
                        let id = if rt.vidpid.is_empty() {
                            crate::hub::attached_vidpid(&f.port_line).unwrap_or_default()
                        } else {
                            rt.vidpid.clone()
                        };
                        let present = crate::hub::usb_present(&id);
                        Ok(format!(
                            "target={} ({id}) at {}:{} [{}]\nlsusb present={present}\n",
                            rt.key, f.location, f.port, f.port_line
                        ))
                    }
                    Err(e) => Ok(format!("target={} not in listing ({e:#})\n", rt.key)),
                }
            })
            .await?;
        Ok(text(s))
    }

    #[tool(
        description = "List configured nicknames plus every allowlisted board currently on the bus with its USB serial — the discovery tool for agents to pick exact targets on multi-board benches."
    )]
    async fn usb_targets(&self) -> Result<CallToolResult, McpError> {
        let s = self
            .blocking(|| {
                let boards = crate::hub::load_boards();
                let mut out = String::from("nicknames (boards.json):\n");
                if boards.is_empty() {
                    out.push_str("  (none)\n");
                } else {
                    let mut names: Vec<_> = boards.keys().collect();
                    names.sort();
                    for n in names {
                        let b = &boards[n];
                        out.push_str(&format!(
                            "  {n}: {} serial={}\n",
                            b.vidpid,
                            b.serial.as_deref().unwrap_or("(any)")
                        ));
                    }
                }
                out.push_str("attached allowlisted devices:\n");
                let listing = crate::hub::list()?;
                let mut found = 0;
                for line in listing.lines() {
                    let t = line.trim();
                    if let Some(rest) = t.strip_prefix("Port ") {
                        if let Some(br) = rest.split('[').nth(1) {
                            let desc = br.trim_end_matches(']');
                            let lower = desc.to_ascii_lowercase();
                            if lower.contains("1915:c00a")
                                || lower.contains("1366:1024")
                                || lower.contains("1915:522a")
                            {
                                out.push_str(&format!("  {t}\n"));
                                found += 1;
                            }
                        }
                    }
                }
                if found == 0 {
                    out.push_str("  (none)\n");
                }
                Ok(out)
            })
            .await?;
        Ok(text(s))
    }

    #[tool(
        description = "Power OFF a board's USB port (both USB2+USB3 sides, so VBUS actually drops on USB3 hubs) and verify it leaves lsusb. Target is a nickname, short name, or USB serial substring. For a stuck board prefer usb_power_cycle."
    )]
    async fn usb_power_off(
        &self,
        Parameters(args): Parameters<TargetArgs>,
    ) -> Result<CallToolResult, McpError> {
        let s = self
            .blocking(move || crate::hub::power_off(&args.target))
            .await?;
        Ok(text(s))
    }

    #[tool(
        description = "Power ON a board's USB port (both USB2+USB3 sides) and verify it re-enumerates in lsusb. Target is a nickname, short name, or USB serial substring."
    )]
    async fn usb_power_on(
        &self,
        Parameters(args): Parameters<TargetArgs>,
    ) -> Result<CallToolResult, McpError> {
        let s = self
            .blocking(move || crate::hub::power_on(&args.target))
            .await?;
        Ok(text(s))
    }

    #[tool(
        description = "Power-CYCLE a stuck board: off, hold off_seconds (default 5), on, verify re-enumeration in lsusb. Both USB2+USB3 sides are switched so VBUS drops. Target is a nickname, short name, or USB serial substring."
    )]
    async fn usb_power_cycle(
        &self,
        Parameters(args): Parameters<CycleArgs>,
    ) -> Result<CallToolResult, McpError> {
        let s = self
            .blocking(move || crate::hub::power_cycle(&args.target, args.off_seconds.unwrap_or(5)))
            .await?;
        Ok(text(s))
    }
}

#[tool_handler]
impl ServerHandler for UhubServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                    .with_title("uhubctl USB power"),
            )
            .with_instructions(
                "Controls USB per-port power for lab boards via uhubctl. \
             Read-only: usb_list, usb_status, usb_targets. Power: usb_power_off, \
             usb_power_on, usb_power_cycle — targets are nicknames, short names, \
             or USB serial substrings, resolved from a fresh listing every call, \
             both USB2+USB3 sides switched so VBUS drops. Raw location:port \
             operation is CLI-only and refused here; hubs with no allowlisted \
             device (e.g. keyboard/mouse hubs) can never be addressed. \
             Requires the udev permissions from the README (one-time sudo); \
             without them every call fails with a permissions error.",
            )
    }
}
