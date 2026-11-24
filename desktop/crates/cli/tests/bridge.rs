//! Runs the built deskhub-cli binary with stdin fixtures and checks the files it writes.
//! DESKHUB_HOME points the binary at a temp home.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_home(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "deskhub-cli-it-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(dir.join(".deskhub")).unwrap();
    dir
}

fn run(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskhub-cli"))
        .args(args)
        .env("DESKHUB_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn deskhub-cli");
    // A mode that ignores stdin may exit before we write: a broken pipe is fine.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().unwrap()
}

#[test]
fn statusline_saves_limits_and_prints_model_without_previous() {
    let home = temp_home("status");
    let input = r#"{"model":{"display_name":"Opus"},"rate_limits":{"five_hour":{"used_percentage":12.5,"resets_at":1700000000},"seven_day":{"used_percentage":40,"resets_at":1700500000}}}"#;
    let out = run(&home, &["statusline"], input);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Opus\n");
    let saved: serde_json::Value = serde_json::from_slice(
        &fs::read(home.join(".deskhub").join("claude_status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["model"], "Opus");
    assert_eq!(saved["rate_limits"]["five_hour"]["used_percentage"], 12.5);
    assert_eq!(saved["rate_limits"]["seven_day"]["resets_at"], 1700500000);
    let _ = fs::remove_dir_all(&home);
}

#[cfg(unix)]
#[test]
fn statusline_forwards_stdin_to_previous_command() {
    let home = temp_home("prev");
    fs::write(
        home.join(".deskhub").join("statusline_previous.json"),
        r#"{"command": "cat"}"#,
    )
    .unwrap();
    let input = r#"{"model":{"display_name":"Opus"},"rate_limits":{}}"#;
    let out = run(&home, &["statusline"], input);
    assert!(out.status.success());
    // `cat` echoes the stdin, so the previous command's output is exactly the input.
    assert_eq!(String::from_utf8_lossy(&out.stdout), input);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn statusline_never_fails_on_garbage() {
    let home = temp_home("garbage");
    let out = run(&home, &["statusline"], "not json");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn hook_records_state_and_session_end_removes_it() {
    let home = temp_home("hook");
    let notify = r#"{"session_id":"abc-1","cwd":"/work/demo/","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs   your permission"}"#;
    let out = run(&home, &["hook"], notify);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    let path = home
        .join(".deskhub")
        .join("claude_sessions")
        .join("abc-1.json");
    let rec: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(rec["state"], "permission");
    assert_eq!(rec["project"], "demo");
    assert_eq!(rec["cwd"], "/work/demo/");
    assert_eq!(rec["message"], "Claude needs your permission");
    assert!(rec["updated"].as_i64().unwrap() > 0);
    assert!(rec["pids"].is_array());

    let end = r#"{"session_id":"abc-1","hook_event_name":"SessionEnd"}"#;
    let out = run(&home, &["hook"], end);
    assert!(out.status.success());
    assert!(!path.exists());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn hook_event_name_argument_overrides_stdin() {
    let home = temp_home("hookarg");
    let out = run(
        &home,
        &["hook", "Stop"],
        r#"{"session_id":"s2","cwd":"C:\\dev\\app"}"#,
    );
    assert!(out.status.success());
    let rec: serde_json::Value = serde_json::from_slice(
        &fs::read(
            home.join(".deskhub")
                .join("claude_sessions")
                .join("s2.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(rec["state"], "idle");
    assert_eq!(rec["project"], "app");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn hook_ignores_garbage_and_unknown_modes() {
    let home = temp_home("junk");
    let out = run(&home, &["hook"], "{oops");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    let out = run(&home, &["nope"], "{}");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(!home.join(".deskhub").join("claude_sessions").exists());
    let _ = fs::remove_dir_all(&home);
}
