//! `deskhub-cli statusline`: port of statusline_bridge.py. Saves the plan usage limits from
//! Claude Code's status line JSON to ~/.deskhub/claude_status.json, then runs the previous
//! status line command (recorded by the installer) with the same stdin and prints its output.

use std::fs;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::store;

const TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_FILE: &str = "claude_status.json";
const PREVIOUS_FILE: &str = "statusline_previous.json";
/// A previous command mentioning this name would call us again forever; ignore it.
const SELF_MARK: &str = "deskhub-cli";

pub fn run(raw: &[u8]) {
    let data: Option<Value> = serde_json::from_slice(raw).ok();
    let dir = store::deskhub_dir();
    if let (Some(dir), Some(v)) = (&dir, data.as_ref()) {
        if let Some(rec) = record(v, store::now_secs()) {
            let bytes = serde_json::to_vec(&rec).unwrap_or_default();
            let _ = store::write_atomic(&dir.join(STATUS_FILE), &bytes);
        }
    }

    if let Some(cmd) = dir.as_deref().and_then(previous_command) {
        if let Some(out) = run_previous(&cmd, raw) {
            let mut stdout = io::stdout();
            let _ = stdout.write_all(&out);
            let _ = stdout.flush();
        }
        return;
    }
    if let Some(name) = data.as_ref().and_then(display_name) {
        let mut stdout = io::stdout();
        let _ = writeln!(stdout, "{name}");
    }
}

/// The status saved for the companion, or None when the Python bridge would skip saving.
pub fn record(data: &Value, now: i64) -> Option<Value> {
    let limits = data.get("rate_limits")?.as_object()?;
    let model = display_name(data)?;
    let mut rate_limits = Map::new();
    for key in ["five_hour", "seven_day"] {
        if let Some(window) = limits.get(key).and_then(Value::as_object) {
            rate_limits.insert(
                key.to_string(),
                json!({
                    "used_percentage": window.get("used_percentage").cloned().unwrap_or(Value::Null),
                    "resets_at": window.get("resets_at").cloned().unwrap_or(Value::Null),
                }),
            );
        }
    }
    Some(json!({"updated": now, "rate_limits": rate_limits, "model": model}))
}

/// `model.display_name`. None when the JSON is not an object or `model` is not an object
/// (the Python bridge fails in that case and prints nothing).
fn display_name(data: &Value) -> Option<String> {
    let obj = data.as_object()?;
    match obj.get("model") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Some(String::new()),
        Some(Value::Object(m)) => Some(
            m.get("display_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
        Some(_) => None,
    }
}

/// The previous status line command from statusline_previous.json. Accepts the current
/// `{"command": ...}` shape and the Python installer's `{"statusLine": {...}}` wrapper.
fn previous_command(dir: &std::path::Path) -> Option<String> {
    let bytes = fs::read(dir.join(PREVIOUS_FILE)).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let inner = v.get("statusLine").unwrap_or(&v);
    let cmd = inner.get("command")?.as_str()?;
    if cmd.is_empty() || cmd.contains(SELF_MARK) {
        return None;
    }
    Some(cmd.to_string())
}

/// Run the previous command like Python's `subprocess.run(shell=True)` with a 10 s limit.
/// Returns its stdout, or None on spawn failure or timeout.
fn run_previous(command: &str, input: &[u8]) -> Option<Vec<u8>> {
    let mut child = shell(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = input.to_vec();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return reader.join().ok(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(not(windows))]
fn shell(command: &str) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(windows)]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let comspec = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
    let mut cmd = Command::new(comspec);
    // Same command line Python builds for shell=True: cmd.exe /c "<command>", unescaped.
    cmd.raw_arg(format!("/c \"{command}\""));
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn record_keeps_five_hour_and_seven_day_and_model() {
        let data = json!({
            "model": {"display_name": "Opus"},
            "rate_limits": {
                "five_hour": {"used_percentage": 12.5, "resets_at": 1000, "extra": 1},
                "seven_day": {"used_percentage": 40},
                "other": {"used_percentage": 99}
            }
        });
        let rec = record(&data, 5).unwrap();
        assert_eq!(rec["updated"], 5);
        assert_eq!(rec["model"], "Opus");
        assert_eq!(rec["rate_limits"]["five_hour"]["used_percentage"], 12.5);
        assert_eq!(rec["rate_limits"]["five_hour"]["resets_at"], 1000);
        assert_eq!(rec["rate_limits"]["seven_day"]["resets_at"], Value::Null);
        assert!(rec["rate_limits"].get("other").is_none());
    }

    #[test]
    fn record_skips_without_rate_limits_or_with_bad_model() {
        assert!(record(&json!({"model": {"display_name": "Opus"}}), 1).is_none());
        assert!(record(&json!({"rate_limits": [], "model": {}}), 1).is_none());
        assert!(record(&json!({"rate_limits": {}, "model": "Opus"}), 1).is_none());
        assert!(record(&json!([1, 2]), 1).is_none());
    }

    #[test]
    fn display_name_defaults_to_empty() {
        assert_eq!(display_name(&json!({})).as_deref(), Some(""));
        assert_eq!(display_name(&json!({"model": {}})).as_deref(), Some(""));
        assert_eq!(display_name(&json!({"model": "x"})), None);
    }

    #[test]
    fn previous_command_reads_both_file_shapes() {
        let dir = store::temp_dir("prev");
        assert_eq!(previous_command(&dir), None);
        fs::write(dir.join(PREVIOUS_FILE), r#"{"command": "ccusage"}"#).unwrap();
        assert_eq!(previous_command(&dir).as_deref(), Some("ccusage"));
        fs::write(
            dir.join(PREVIOUS_FILE),
            r#"{"statusLine": {"command": "old"}}"#,
        )
        .unwrap();
        assert_eq!(previous_command(&dir).as_deref(), Some("old"));
        fs::write(dir.join(PREVIOUS_FILE), r#"{"statusLine": null}"#).unwrap();
        assert_eq!(previous_command(&dir), None);
        fs::write(
            dir.join(PREVIOUS_FILE),
            r#"{"command": "deskhub-cli statusline"}"#,
        )
        .unwrap();
        assert_eq!(previous_command(&dir), None);
        let _ = fs::remove_dir_all(Path::new(&dir));
    }
}
