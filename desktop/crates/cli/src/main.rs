//! deskhub-cli: run by Claude Code, not by people.
//!   deskhub-cli statusline   status line command (stdin JSON from Claude Code)
//!   deskhub-cli hook         hook command (stdin JSON from Claude Code)
//! Port of companion/deskhub/statusline_bridge.py and hook_bridge.py.
//! Always exits 0 and prints nothing on failure, so it never breaks Claude Code.

mod hook;
mod procs;
mod statusline;
mod store;

use std::io::{self, Read};

fn main() {
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    match args.get(1).map(String::as_str) {
        Some("statusline") => statusline::run(&read_stdin()),
        Some("hook") => hook::run(args.get(2).map(String::as_str), &read_stdin()),
        _ => {}
    }
    std::process::exit(0);
}

fn read_stdin() -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = io::stdin().read_to_end(&mut buf);
    buf
}
