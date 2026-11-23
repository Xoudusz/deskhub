//! `deskhub-cli hook`: port of hook_bridge.py. Records whether a Claude Code session is
//! working or waiting for you, in ~/.deskhub/claude_sessions/<session_id>.json.
//! Only state, folder name and notification text are stored, never prompts or tool input.

use std::fs;
use std::io;
use std::path::Path;

use serde_json::{json, Value};

use crate::store;

/// Notification types that mean Claude is waiting for you, and the state each maps to.
const WAITING: [(&str, &str); 4] = [
    ("permission_prompt", "permission"),
    ("idle_prompt", "input"),
    ("elicitation_dialog", "input"),
    ("agent_needs_input", "input"),
];
/// Tools that stop and wait for you without a permission dialog.
const ASKS: [(&str, &str); 2] = [
    ("AskUserQuestion", "Claude has a question for you"),
    ("ExitPlanMode", "Claude has a plan ready for your review"),
];
const MESSAGE_MAX: usize = 200;

/// `event_arg` is the optional event name given on the command line; otherwise the
/// event comes from the hook JSON's `hook_event_name`. Never prints, never fails.
pub fn run(event_arg: Option<&str>, raw: &[u8]) {
    let Ok(data) = serde_json::from_slice::<Value>(raw) else {
        return;
    };
    if !data.is_object() {
        return;
    }
    let event = match event_arg.filter(|e| !e.is_empty()) {
        Some(e) => e.to_string(),
        None => text(data.get("hook_event_name")),
    };
    if let Some(dir) = store::deskhub_dir() {
        let _ = apply(
            &dir.join("claude_sessions"),
            &event,
            &data,
            store::now_secs(),
            crate::procs::ancestors,
        );
    }
}

/// Write or delete the session file for one hook event.
pub fn apply(
    dir: &Path,
    event: &str,
    data: &Value,
    now: i64,
    pids: impl FnOnce() -> Vec<u32>,
) -> io::Result<()> {
    let Some(sid) = session_id(data) else {
        return Ok(());
    };
    let path = dir.join(format!("{sid}.json"));
    if event == "SessionEnd" {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let Some(rec) = record(event, data, now, pids) else {
        return Ok(());
    };
    let bytes = serde_json::to_vec(&rec).map_err(io::Error::other)?;
    store::write_atomic(&path, &bytes)
}

/// The session record for an event, or None when the event should not touch the file.
pub fn record(
    event: &str,
    data: &Value,
    now: i64,
    pids: impl FnOnce() -> Vec<u32>,
) -> Option<Value> {
    let (state, message): (&str, String) = match event {
        "Notification" => {
            let kind = text(data.get("notification_type"));
            let state = WAITING.iter().find(|(k, _)| *k == kind)?.1;
            (state, collapse(&text(data.get("message"))))
        }
        "PermissionRequest" => {
            // A permission dialog is showing (terminal and desktop app alike).
            let tool = text(data.get("tool_name"));
            let message = if tool.is_empty() {
                String::new()
            } else {
                format!("Claude wants to use {tool}")
            };
            ("permission", message)
        }
        "PreToolUse" => {
            let tool = text(data.get("tool_name"));
            let message = ASKS.iter().find(|(t, _)| *t == tool)?.1;
            ("input", message.to_string())
        }
        // Finished its turn. The companion alerts once it has been idle a little while.
        "Stop" => ("idle", String::new()),
        "UserPromptSubmit" | "PostToolUse" => ("working", String::new()),
        _ => return None,
    };
    let cwd = text(data.get("cwd"));
    Some(json!({
        "state": state,
        "cwd": cwd,
        "project": basename(&cwd),
        "message": message,
        "updated": now,
        "pids": pids(),
    }))
}

/// The session id, or None when it is empty or could not be a file name.
fn session_id(data: &Value) -> Option<String> {
    let sid = text(data.get("session_id"));
    if sid.is_empty() || sid.contains(['/', '\\', ':']) {
        return None;
    }
    Some(sid)
}

/// Python-style `str(x or "")`: null and missing become empty.
fn text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Collapse whitespace runs to single spaces and cap the length, as the Python bridge does.
fn collapse(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MESSAGE_MAX)
        .collect()
}

/// Last path segment, accepting both separators (the bridge only ever runs on Windows paths too).
fn basename(cwd: &str) -> String {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(v: Value) -> Value {
        v
    }

    fn state_of(event: &str, d: &Value) -> Option<String> {
        record(event, d, 1, Vec::new).map(|r| r["state"].as_str().unwrap().to_string())
    }

    #[test]
    fn state_mapping_matches_python_bridge() {
        let perm = data(json!({"notification_type": "permission_prompt", "message": "x"}));
        assert_eq!(
            state_of("Notification", &perm).as_deref(),
            Some("permission")
        );
        let idle = data(json!({"notification_type": "idle_prompt"}));
        assert_eq!(state_of("Notification", &idle).as_deref(), Some("input"));
        let other = data(json!({"notification_type": "auth_success"}));
        assert_eq!(state_of("Notification", &other), None);
        let ask = data(json!({"tool_name": "AskUserQuestion"}));
        assert_eq!(state_of("PreToolUse", &ask).as_deref(), Some("input"));
        let bash = data(json!({"tool_name": "Bash"}));
        assert_eq!(state_of("PreToolUse", &bash), None);
        assert_eq!(
            state_of("PermissionRequest", &bash).as_deref(),
            Some("permission")
        );
        assert_eq!(state_of("Stop", &bash).as_deref(), Some("idle"));
        assert_eq!(
            state_of("UserPromptSubmit", &bash).as_deref(),
            Some("working")
        );
        assert_eq!(state_of("PostToolUse", &bash).as_deref(), Some("working"));
        assert_eq!(state_of("SubagentStop", &bash), None);
    }

    #[test]
    fn messages_are_collapsed_and_capped() {
        let d = data(json!({"notification_type": "idle_prompt", "message": "a  b\n c"}));
        let rec = record("Notification", &d, 1, Vec::new).unwrap();
        assert_eq!(rec["message"], "a b c");
        let long = "x".repeat(300);
        let d = data(json!({"notification_type": "idle_prompt", "message": long}));
        let rec = record("Notification", &d, 1, Vec::new).unwrap();
        assert_eq!(
            rec["message"].as_str().unwrap().chars().count(),
            MESSAGE_MAX
        );
        let d = data(json!({"tool_name": "ExitPlanMode"}));
        let rec = record("PreToolUse", &d, 1, Vec::new).unwrap();
        assert_eq!(rec["message"], "Claude has a plan ready for your review");
    }

    #[test]
    fn permission_message_names_the_tool() {
        let d = data(json!({"tool_name": "Bash"}));
        let rec = record("PermissionRequest", &d, 1, Vec::new).unwrap();
        assert_eq!(rec["message"], "Claude wants to use Bash");
        let d = data(json!({}));
        let rec = record("PermissionRequest", &d, 1, Vec::new).unwrap();
        assert_eq!(rec["message"], "");
    }

    #[test]
    fn basename_takes_last_segment_on_either_separator() {
        assert_eq!(basename("C:\\dev\\proj\\"), "proj");
        assert_eq!(basename("/home/u/app"), "app");
        assert_eq!(basename(""), "");
    }

    #[test]
    fn session_ids_that_are_not_file_names_are_ignored() {
        assert_eq!(session_id(&json!({"session_id": "a/b"})), None);
        assert_eq!(session_id(&json!({"session_id": "c:d"})), None);
        assert_eq!(session_id(&json!({})), None);
        assert_eq!(
            session_id(&json!({"session_id": "abc-1"})).as_deref(),
            Some("abc-1")
        );
    }

    #[test]
    fn apply_writes_fields_and_session_end_deletes() {
        let dir = store::temp_dir("hook");
        let sessions = dir.join("claude_sessions");
        let d = data(json!({
            "session_id": "s1",
            "cwd": "/work/demo",
            "notification_type": "permission_prompt",
            "message": "needs  approval"
        }));
        apply(&sessions, "Notification", &d, 42, || vec![7, 8]).unwrap();
        let rec: Value =
            serde_json::from_slice(&fs::read(sessions.join("s1.json")).unwrap()).unwrap();
        assert_eq!(rec["state"], "permission");
        assert_eq!(rec["cwd"], "/work/demo");
        assert_eq!(rec["project"], "demo");
        assert_eq!(rec["message"], "needs approval");
        assert_eq!(rec["updated"], 42);
        assert_eq!(rec["pids"], json!([7, 8]));

        let end = data(json!({"session_id": "s1"}));
        apply(&sessions, "SessionEnd", &end, 43, Vec::new).unwrap();
        assert!(!sessions.join("s1.json").exists());
        // Deleting a missing session is not an error.
        apply(&sessions, "SessionEnd", &end, 44, Vec::new).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
