//! mcp-uhubctl entry point. Serves the tool surface over stdio so it can be
//! launched by an MCP client (Claude Code / Claude Desktop) as a subprocess.
//!
//! `--uhubctl-path <path>` (or `UHUBCTL_BIN` env) overrides the uhubctl binary
//! location; `--boards-path <path>` (or `BOARDS_PATH` env) overrides the
//! nickname config file. Defaults are `uhubctl` on PATH (`sudo make install`
//! puts it at /usr/sbin/uhubctl) and `~/.config/mcp-uhubctl/boards.json`.

use anyhow::{Result, anyhow};
use mcp_uhubctl::server::UhubServer;
use rmcp::{ServiceExt, transport::stdio};

fn resolve_args() -> (Option<String>, Option<String>) {
    let mut ubin = None;
    let mut bpath = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--uhubctl-path" {
            ubin = args.next();
        } else if let Some(v) = a.strip_prefix("--uhubctl-path=") {
            ubin = Some(v.to_string());
        } else if a == "--boards-path" {
            bpath = args.next();
        } else if let Some(v) = a.strip_prefix("--boards-path=") {
            bpath = Some(v.to_string());
        } else if a == "-h" || a == "--help" {
            eprintln!("usage: mcp-uhubctl [--uhubctl-path <path>] [--boards-path <path>]");
            std::process::exit(0);
        }
    }
    (ubin, bpath)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let (ubin, bpath) = resolve_args();
    if let Some(p) = ubin {
        if !(std::path::Path::new(&p).exists() || p.contains('/')) {
            return Err(anyhow!("uhubctl binary not found at {p:?}"));
        }
        // SAFETY: single-threaded at startup, before any threads spawn.
        unsafe { std::env::set_var("UHUBCTL_BIN", &p) };
        tracing::info!(uhubctl_bin = %p, "using uhubctl binary");
    }
    if let Some(p) = bpath {
        // SAFETY: single-threaded at startup, before any threads spawn.
        unsafe { std::env::set_var("BOARDS_PATH", &p) };
        tracing::info!(boards_path = %p, "using boards file");
    }
    let service = UhubServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
