//! `deskhub-cli hook`: port of hook_bridge.py. Records whether a Claude Code session is
//! working or waiting for you, in ~/.deskhub/claude_sessions/<session_id>.json.
//! Stored: state, folder name, notification text and, for a permission request, a short
//! summary of what it asks for (the Bash command, file path or URL, 120 characters).
//! Prompts and Claude's replies are never stored.
//!
//! Board decisions (a server running deskhub-relay, marker file ~/.deskhub/board_decisions):
//! a permission request then also waits for ~/.deskhub/claude_decisions/<session_id>.json
//! (written by the relay when you tap Allow or Deny on the board) and answers Claude Code
//! with it. The terminal dialog stays usable meanwhile; whichever answers first wins.

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

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
    ("ExitPlanMode", PLAN_READY),
];
const MESSAGE_MAX: usize = 200;
/// What a permission request asks for, on the board.
const DETAIL_MAX: usize = 120;
/// Present on a server whose relay forwards the board's Allow / Deny.
pub const DECIDE_MARK: &str = "board_decisions";
/// Under Claude Code's 600 s hook timeout.
const DECIDE_WAIT: Duration = Duration::from_secs(590);
const DECIDE_POLL: Duration = Duration::from_millis(300);
const PLAN_READY: &str = "Claude has a plan ready for your review";
const QUESTION: &str = "Claude has a question for you";

/// `event_arg` is the optional event name given on the command line; otherwise the
/// event comes from the hook JSON's `hook_event_name`. Never fails. Returns the line to
/// print: only a board decision for a permission request (see the module comment).
pub fn run(event_arg: Option<&str>, raw: &[u8]) -> Option<String> {
    let data = serde_json::from_slice::<Value>(raw).ok()?;
    if !data.is_object() {
        return None;
    }
    let event = match event_arg.filter(|e| !e.is_empty()) {
        Some(e) => e.to_string(),
        None => text(data.get("hook_event_name")),
    };
    let dir = store::deskhub_dir()?;
    let sessions = dir.join("claude_sessions");
    let _ = apply(
        &sessions,
        &event,
        &data,
        store::now_secs(),
        crate::procs::ancestors,
    );
    if event != "PermissionRequest" || !dir.join(DECIDE_MARK).exists() {
        return None;
    }
    let sid = session_id(&data)?;
    let rec = read_record(&sessions.join(format!("{sid}.json")))?;
    let request = rec["request"].as_str().filter(|r| !r.is_empty())?.to_string();
    wait_decision(
        &dir.join("claude_decisions"),
        &sessions,
        &sid,
        &request,
        DECIDE_WAIT,
        DECIDE_POLL,
    )
}

fn read_record(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Waits for the board's answer to permission request `request` of session `sid`.
/// Gives up (None, Claude Code keeps its dialog) once the session moved on: answered in
/// the terminal, another request, or ended.
pub fn wait_decision(
    decisions: &Path,
    sessions: &Path,
    sid: &str,
    request: &str,
    timeout: Duration,
    poll: Duration,
) -> Option<String> {
    let file = decisions.join(format!("{sid}.json"));
    let session = sessions.join(format!("{sid}.json"));
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if let Some(d) = read_record(&file) {
            let _ = fs::remove_file(&file);
            if d["request"].as_str() == Some(request) {
                return match d["behavior"].as_str() {
                    Some("allow") => Some(decision(json!({"behavior": "allow"}))),
                    Some("deny") => Some(decision(json!({
                        "behavior": "deny",
                        "message": "Denied from the Desk Hub board",
                    }))),
                    _ => None,
                };
            }
        }
        let pending = read_record(&session).is_some_and(|r| {
            r["state"].as_str() == Some("permission") && r["request"].as_str() == Some(request)
        });
        if !pending {
            return None;
        }
        std::thread::sleep(poll);
    }
    None
}

fn decision(d: Value) -> String {
    json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": d}})
        .to_string()
}

/// A fresh id for one permission request (binds the board's answer to it).
fn new_request() -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.write_u32(std::process::id());
    let a = h.finish();
    let mut h = RandomState::new().build_hasher();
    h.write_u64(a);
    format!("{a:016x}{:016x}", h.finish())
}

/// What a permission request asks for: "Bash: npm test", "Edit: src/main.rs".
pub fn summary(tool: &str, input: Option<&Value>) -> String {
    let field = |k: &str| input.and_then(|i| i.get(k)).and_then(Value::as_str).unwrap_or("");
    let what = match tool {
        "Bash" => field("command"),
        "Edit" | "MultiEdit" | "Write" | "Read" => field("file_path"),
        "NotebookEdit" => field("notebook_path"),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        _ => "",
    };
    let line = if what.is_empty() {
        tool.to_string()
    } else {
        format!("{tool}: {what}")
    };
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(DETAIL_MAX)
        .collect()
}

/// A turn that ends by asking you something ("Ready to execute this plan, or ...?").
fn asks_back(reply: &str) -> Option<&'static str> {
    let last = reply.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let last = last.trim_end_matches(['*', '_', '`', ')', '"', ' ']);
    if !last.ends_with('?') {
        return None;
    }
    let tail: String = reply.chars().rev().take(300).collect::<String>().to_lowercase();
    // `tail` is reversed: "plan" reads "nalp".
    Some(if tail.contains("nalp") { PLAN_READY } else { QUESTION })
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
    let Some(mut rec) = record(event, data, now, pids) else {
        return Ok(());
    };
    // Claude Code's own "needs your permission" notice follows a permission request:
    // keep that request's id and summary so the board can still answer it.
    if event == "Notification" && rec["state"] == "permission" {
        if let Some(old) = read_record(&path).filter(|o| o["state"] == "permission") {
            for k in ["request", "detail"] {
                if let Some(v) = old.get(k) {
                    rec[k] = v.clone();
                }
            }
        }
    }
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
    let mut extra = None;
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
            extra = Some((new_request(), summary(&tool, data.get("tool_input"))));
            ("permission", message)
        }
        "PreToolUse" => {
            let tool = text(data.get("tool_name"));
            let message = ASKS.iter().find(|(t, _)| *t == tool)?.1;
            ("input", message.to_string())
        }
        // Finished its turn. The companion alerts once it has been idle a little while,
        // at once when the turn ends with a question (such as a plan to approve).
        // Still running its own background jobs (a build, CI): not waiting for you yet.
        "Stop" if data
            .get("background_tasks")
            .and_then(Value::as_array)
            .is_some_and(|t| !t.is_empty()) =>
        {
            ("working", String::new())
        }
        "Stop" => match asks_back(&text(data.get("last_assistant_message"))) {
            Some(m) => ("input", m.to_string()),
            None => ("idle", String::new()),
        },
        "UserPromptSubmit" | "PostToolUse" => ("working", String::new()),
        _ => return None,
    };
    let cwd = text(data.get("cwd"));
    let mut rec = json!({
        "state": state,
        "cwd": cwd,
        "project": basename(&cwd),
        "message": message,
        "updated": now,
        "pids": pids(),
    });
    if let Some((request, detail)) = extra {
        rec["request"] = json!(request);
        rec["detail"] = json!(detail);
    }
    Some(rec)
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
    fn permission_request_has_an_id_and_a_summary() {
        let d = data(json!({"tool_name": "Bash", "tool_input": {"command": "rm  -rf\n build"}}));
        let a = record("PermissionRequest", &d, 1, Vec::new).unwrap();
        let b = record("PermissionRequest", &d, 1, Vec::new).unwrap();
        assert_eq!(a["detail"], "Bash: rm -rf build");
        assert_eq!(a["request"].as_str().unwrap().len(), 32);
        assert_ne!(a["request"], b["request"]);
        assert_eq!(summary("Edit", Some(&json!({"file_path": "/a/b.rs"}))), "Edit: /a/b.rs");
        assert_eq!(summary("mcp__x__y", Some(&json!({"q": 1}))), "mcp__x__y");
        assert_eq!(summary("Bash", Some(&json!({"command": "x".repeat(300)}))).len(), DETAIL_MAX);
        // Other events carry neither.
        let stop = record("Stop", &d, 1, Vec::new).unwrap();
        assert!(stop.get("request").is_none());
    }

    #[test]
    fn a_turn_ending_in_a_question_waits_for_input_at_once() {
        let stop = |m: &str| {
            let r = record("Stop", &json!({"last_assistant_message": m}), 1, Vec::new).unwrap();
            (r["state"].as_str().unwrap().to_string(), r["message"].as_str().unwrap().to_string())
        };
        assert_eq!(
            stop("Steps...\n\nReady to execute this plan, or do you want changes?\n"),
            ("input".into(), PLAN_READY.into())
        );
        assert_eq!(stop("Which port should it use?"), ("input".into(), QUESTION.into()));
        assert_eq!(stop("**Want me to merge it?**"), ("input".into(), QUESTION.into()));
        assert_eq!(stop("Done. Tests pass."), ("idle".into(), String::new()));
        assert_eq!(stop("Is it fixed? Yes, all green."), ("idle".into(), String::new()));
        let none = record("Stop", &json!({}), 1, Vec::new).unwrap();
        assert_eq!(none["state"], "idle");
        // Waiting on its own background jobs: still working, even after a question.
        let bg = json!({"last_assistant_message": "Merge it?", "background_tasks": [{"id": "b1"}]});
        assert_eq!(record("Stop", &bg, 1, Vec::new).unwrap()["state"], "working");
        let idle = json!({"last_assistant_message": "Done.", "background_tasks": []});
        assert_eq!(record("Stop", &idle, 1, Vec::new).unwrap()["state"], "idle");
    }

    #[test]
    fn the_permission_notice_keeps_the_request() {
        let dir = store::temp_dir("hook-keep");
        let sessions = dir.join("claude_sessions");
        let req = json!({"session_id": "s1", "tool_name": "Bash", "tool_input": {"command": "ls"}});
        apply(&sessions, "PermissionRequest", &req, 1, Vec::new).unwrap();
        let first = read_record(&sessions.join("s1.json")).unwrap();
        let note = json!({"session_id": "s1", "notification_type": "permission_prompt", "message": "m"});
        apply(&sessions, "Notification", &note, 2, Vec::new).unwrap();
        let now = read_record(&sessions.join("s1.json")).unwrap();
        assert_eq!(now["request"], first["request"]);
        assert_eq!(now["detail"], "Bash: ls");
        assert_eq!(now["updated"], 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn board_decisions_answer_only_their_own_request() {
        let dir = store::temp_dir("hook-decide");
        let (sessions, decisions) = (dir.join("claude_sessions"), dir.join("claude_decisions"));
        fs::create_dir_all(&decisions).unwrap();
        let pending = json!({"state": "permission", "request": "r1", "updated": 1});
        store::write_atomic(&sessions.join("s1.json"), pending.to_string().as_bytes()).unwrap();
        let wait = || wait_decision(&decisions, &sessions, "s1", "r1", Duration::from_millis(400), Duration::from_millis(10));
        let put = |v: Value| store::write_atomic(&decisions.join("s1.json"), v.to_string().as_bytes()).unwrap();

        // Nothing decided: gives up at the timeout.
        assert_eq!(wait(), None);
        // Another request's answer is dropped, not used.
        put(json!({"request": "old", "behavior": "allow"}));
        assert_eq!(wait(), None);
        assert!(!decisions.join("s1.json").exists());
        put(json!({"request": "r1", "behavior": "allow"}));
        let out: Value = serde_json::from_str(&wait().unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PermissionRequest");
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert!(!decisions.join("s1.json").exists());
        put(json!({"request": "r1", "behavior": "deny"}));
        let out: Value = serde_json::from_str(&wait().unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "deny");
        // Answered in the terminal (the session moved on): stops waiting at once.
        let working = json!({"state": "working", "updated": 2});
        store::write_atomic(&sessions.join("s1.json"), working.to_string().as_bytes()).unwrap();
        let t = Instant::now();
        assert_eq!(wait_decision(&decisions, &sessions, "s1", "r1", Duration::from_secs(5), Duration::from_millis(10)), None);
        assert!(t.elapsed() < Duration::from_secs(1));
        let _ = fs::remove_dir_all(&dir);
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
