//! deskhub-relay: Claude Code on a server, for the Desk Hub app (Settings > AI).
//!
//!   deskhub-relay serve
//!       DESKHUB_RELAY_TOKEN  the token the app sends (at least 16 characters, required)
//!       DESKHUB_RELAY_BIND   address to listen on (default 127.0.0.1:8791)
//!       DESKHUB_RELAY_NAME   the server's name in alerts (default /etc/hostname)
//!   deskhub-relay install-hooks [--cli /usr/local/bin/deskhub-cli]
//!       adds deskhub-cli's hooks to Claude Code's settings.json, so sessions are written
//!       to ~/.deskhub/claude_sessions for the relay to serve, and turns on the board's
//!       Allow / Deny for permission requests (~/.deskhub/board_decisions).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use deskhub_core::claude_install::{install_in, What};
use deskhub_core::relay;

const USAGE: &str = "usage: deskhub-relay serve | install-hooks [--cli PATH]";

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")
}

fn serve() -> Result<()> {
    let token = std::env::var("DESKHUB_RELAY_TOKEN").unwrap_or_default().trim().to_string();
    let bind = std::env::var("DESKHUB_RELAY_BIND").unwrap_or_else(|_| "127.0.0.1:8791".into());
    let host = std::env::var("DESKHUB_RELAY_NAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "server".into());
    let cfg = relay::Config { token, host, home: home()? };
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(relay::run(&bind, cfg))
}

fn install_hooks(args: &[String]) -> Result<()> {
    let cli = match args {
        [] => PathBuf::from("/usr/local/bin/deskhub-cli"),
        [flag, path] if flag == "--cli" => PathBuf::from(path),
        _ => bail!("{USAGE}"),
    };
    if !cli.is_file() {
        bail!("{} not found: install deskhub-cli first", cli.display());
    }
    let home = home()?;
    let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let settings = claude_dir.join("settings.json");
    let dir = home.join(".deskhub");
    install_in(&settings, &dir, &cli, What::Hooks)?;
    // Permission requests then wait for the board's Allow / Deny (deskhub-cli hook.rs).
    std::fs::write(dir.join("board_decisions"), "deskhub-relay install-hooks\n")
        .context("writing ~/.deskhub/board_decisions")?;
    println!("hooks installed in {}", settings.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("serve") => serve(),
        Some("install-hooks") => install_hooks(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("deskhub-relay: {e:#}");
            ExitCode::FAILURE
        }
    }
}
