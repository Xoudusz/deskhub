//! "Claude is waiting" alerts from the hook files, and the board's Show/Dismiss buttons.
//! Port of companion/deskhub/claude_alerts.py.
//!
//! hook_bridge.py keeps one state file per Claude Code session in
//! `~/.deskhub/claude_sessions/<session>.json`. This picks the session to alert about
//! and publishes it as the "alert" key (or a clear message). The pure parts (parsing,
//! picking, dedup, text folding) are platform-independent and unit-tested on Linux.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::Ctx;
use crate::protocol::Incoming;

/// Publish key for the board's alert panel.
const KEY: &str = "alert";
/// How often the session folder is read.
const POLL: Duration = Duration::from_secs(1);
/// Waiting sessions older than this are ignored (Python FRESH_S).
pub const FRESH_S: f64 = 30.0 * 60.0;
/// Session files older than this are deleted (Python STALE_S).
pub const STALE_S: f64 = 2.0 * 3600.0;

/// One session file, as hook_bridge.py writes it. The session id is the file name.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub state: String,
    pub message: String,
    pub project: String,
    /// Unix seconds, as written by the hook.
    pub updated: f64,
    /// Process ids of the session's ancestors (the windows that run it).
    pub pids: Vec<u32>,
}

/// The alert shown on the board.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub detail: String,
    pub project: String,
}

impl Alert {
    pub fn to_value(&self) -> Value {
        json!({
            "type": "alert",
            "id": self.id,
            "kind": self.kind,
            "agent": "claude",
            "title": self.title,
            "detail": self.detail,
            "project": self.project,
        })
    }
}

pub fn clear_value() -> Value {
    json!({"type": "alert", "clear": true})
}

/// Keep only [A-Za-z0-9_-]. Session ids come from file names and from the board.
pub fn sanitize_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

/// Parse one session file. None unless it is a JSON object with a numeric `updated`.
pub fn parse_session(text: &str) -> Option<Session> {
    let v: Value = serde_json::from_str(text).ok()?;
    let obj = v.as_object()?;
    let updated = obj.get("updated")?.as_f64()?;
    let str_of = |key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let pids = obj
        .get("pids")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .filter_map(|p| u32::try_from(p).ok())
                .collect()
        })
        .unwrap_or_default();
    Some(Session {
        state: str_of("state"),
        message: str_of("message"),
        project: str_of("project"),
        updated,
        pids,
    })
}

/// Read every session file in `root`. Files older than STALE_S are deleted; unreadable
/// or malformed files are skipped. Ids are sanitised.
pub fn load_sessions(root: &Path, now: f64) -> HashMap<String, Session> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let id = sanitize_id(stem);
        if id.is_empty() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(session) = parse_session(&text) else {
            continue;
        };
        if now - session.updated > STALE_S {
            let _ = std::fs::remove_file(&path);
        } else {
            out.insert(id, session);
        }
    }
    out
}

/// The session to alert about: the newest waiting one (permission, input, or idle
/// for `idle_after` seconds), skipping dismissed and too-old ones.
pub fn pick(
    sessions: &HashMap<String, Session>,
    now: f64,
    idle_after: f64,
    dismissed: &HashMap<String, f64>,
) -> Option<Alert> {
    let mut best: Option<(&String, &Session)> = None;
    for (sid, d) in sessions {
        let age = now - d.updated;
        if !matches!(d.state.as_str(), "permission" | "input" | "idle") || age > FRESH_S {
            continue;
        }
        if d.state == "idle" && age < idle_after {
            continue; // just finished: probably still being looked at
        }
        if dismissed.get(sid) == Some(&d.updated) {
            continue;
        }
        let newer = match best {
            None => true,
            // Ties go to the smaller id so the choice does not depend on hash order.
            Some((bid, b)) => d.updated > b.updated || (d.updated == b.updated && sid < bid),
        };
        if newer {
            best = Some((sid, d));
        }
    }
    let (sid, d) = best?;
    let permission = d.state == "permission";
    let kind = if permission { "permission" } else { "input" };
    let generic = if permission {
        "Claude needs your permission"
    } else {
        "Claude is waiting for you"
    };
    // Claude Code's own message ("Claude needs your permission to use Bash") is the most
    // specific line, so it becomes the title. Any other message goes under the generic title.
    let message = d.message.trim();
    let (title, detail) = if message.starts_with("Claude ") {
        (message, "")
    } else if !message.is_empty() {
        (generic, message)
    } else if permission {
        (generic, "")
    } else {
        (generic, "Finished. Waiting for your reply")
    };
    Some(Alert {
        id: sid.clone(),
        kind,
        title: ascii_fold(title, 60),
        detail: ascii_fold(detail, 120),
        project: ascii_fold(&d.project, 40),
    })
}

/// Fold text to ASCII for the board's fonts, collapse whitespace and cut to `limit` chars.
pub fn ascii_fold(s: &str, limit: usize) -> String {
    let mut folded = String::new();
    for c in s.chars() {
        fold_char(c, &mut folded);
    }
    folded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

/// Typographic punctuation and common Latin letters map to ASCII; other non-ASCII is dropped.
fn fold_char(c: char, out: &mut String) {
    let s = match c {
        '\u{2018}' | '\u{2019}' => "'",
        '\u{201C}' | '\u{201D}' => "\"",
        '\u{2013}' | '\u{2014}' | '\u{00B7}' | '\u{2022}' => "-",
        '\u{2026}' => "...",
        'à'..='å' => "a",
        'À'..='Å' => "A",
        'ç' => "c",
        'Ç' => "C",
        'è'..='ë' => "e",
        'È'..='Ë' => "E",
        'ì'..='ï' => "i",
        'Ì'..='Ï' => "I",
        'ñ' => "n",
        'Ñ' => "N",
        'ò'..='ö' | 'ø' => "o",
        'Ò'..='Ö' | 'Ø' => "O",
        'ù'..='ü' => "u",
        'Ù'..='Ü' => "U",
        'ý' | 'ÿ' => "y",
        'Ý' => "Y",
        'ß' => "ss",
        'æ' => "ae",
        'Æ' => "AE",
        'œ' => "oe",
        'Œ' => "OE",
        _ if c.is_ascii() => {
            out.push(c);
            return;
        }
        _ => return,
    };
    out.push_str(s);
}

/// Alert bookkeeping: what was dismissed, what the board shows. Pure; `run` feeds it.
#[derive(Debug, Default)]
pub struct Tracker {
    /// Session id -> `updated` of the alert that was dismissed.
    dismissed: HashMap<String, f64>,
    /// Sessions from the last poll (dismiss looks them up).
    sessions: HashMap<String, Session>,
    /// What the board was last told.
    shown: Option<Alert>,
}

impl Tracker {
    /// One poll over freshly loaded sessions. Returns the message to publish when the
    /// alert changed, else None. `fg_pid` is the process owning the front window (0 if none).
    pub fn poll(
        &mut self,
        sessions: HashMap<String, Session>,
        now: f64,
        fg_pid: u32,
        idle_after: f64,
    ) -> Option<Value> {
        self.sessions = sessions;
        self.mark_seen(fg_pid);
        let new = pick(&self.sessions, now, idle_after, &self.dismissed);
        if new == self.shown {
            return None;
        }
        let value = new.as_ref().map_or_else(clear_value, Alert::to_value);
        self.shown = new;
        Some(value)
    }

    /// Opening a finished session in its window fires no hook, so a finished session
    /// whose window is in front counts as seen. It alerts again only when its state changes.
    fn mark_seen(&mut self, fg_pid: u32) {
        if fg_pid == 0 {
            return;
        }
        for (sid, d) in &self.sessions {
            if d.state == "idle" && d.pids.contains(&fg_pid) {
                self.dismissed.insert(sid.clone(), d.updated);
            }
        }
    }

    /// Hide the current alert for this session until its state changes.
    pub fn dismiss(&mut self, sid: &str) {
        if let Some(d) = self.sessions.get(sid) {
            self.dismissed.insert(sid.to_string(), d.updated);
        }
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// ~/.deskhub/claude_sessions, with the home folder from USERPROFILE on Windows and HOME elsewhere.
fn sessions_dir() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }?;
    Some(PathBuf::from(home).join(".deskhub").join("claude_sessions"))
}

/// Read the session folder off the async runtime.
async fn load_async(root: Option<PathBuf>, now: f64) -> HashMap<String, Session> {
    match root {
        Some(root) => tokio::task::spawn_blocking(move || load_sessions(&root, now))
            .await
            .unwrap_or_default(),
        None => HashMap::new(),
    }
}

/// Focus the window of one session. Re-reads the folder so the pids are current.
async fn focus_async(root: Option<PathBuf>, sid: String, now: f64) -> String {
    tokio::task::spawn_blocking(move || {
        let pids = root
            .map(|r| load_sessions(&r, now))
            .and_then(|mut all| all.remove(&sid))
            .map(|s| s.pids);
        let result = match pids {
            None => "session not found".to_string(),
            Some(pids) => focus_pids(&pids),
        };
        log::info!("alert focus {sid}: {result}");
        result
    })
    .await
    .unwrap_or_else(|e| format!("failed ({e})"))
}

pub async fn run(ctx: Ctx) {
    let Ctx {
        out,
        mut settings,
        mut incoming,
    } = ctx;
    let root = sessions_dir();
    let mut tracker = Tracker::default();
    let mut off_published = false;
    let mut incoming_open = true;
    let mut ticker = tokio::time::interval(POLL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        // borrow_and_update marks the value seen, so changed() below waits for the next change.
        let (enabled, idle_after) = {
            let s = settings.borrow_and_update();
            (s.ai.alerts, s.ai.idle_alert_seconds.max(0) as f64)
        };
        if !enabled {
            if !off_published {
                out.publish(KEY, clear_value());
                off_published = true;
            }
            tracker = Tracker::default();
            if settings.changed().await.is_err() {
                return;
            }
            continue;
        }
        off_published = false;

        tokio::select! {
            _ = ticker.tick() => {
                let now = now_secs();
                let sessions = load_async(root.clone(), now).await;
                if let Some(msg) = tracker.poll(sessions, now, foreground_pid(), idle_after) {
                    out.publish(KEY, msg);
                }
            }
            msg = incoming.recv(), if incoming_open => match msg {
                Ok(Incoming::AlertAction { id, action }) => {
                    let sid = sanitize_id(&id);
                    match action.as_str() {
                        "dismiss" => tracker.dismiss(&sid),
                        "focus" => {
                            focus_async(root.clone(), sid.clone(), now_secs()).await;
                            // Focusing counts as seen.
                            tracker.dismiss(&sid);
                        }
                        _ => {}
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => incoming_open = false,
            },
            changed = settings.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(windows)]
fn foreground_pid() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    // SAFETY: plain queries; the out-pointer is a live local for the call.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return 0;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        pid
    }
}

#[cfg(not(windows))]
fn foreground_pid() -> u32 {
    0
}

/// Bring the window of the first pid that owns a visible top-level window to the front.
#[cfg(windows)]
fn focus_pids(pids: &[u32]) -> String {
    use std::ffi::c_void;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FlashWindowEx, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetForegroundWindow, ShowWindow, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, GW_OWNER,
        SW_RESTORE,
    };

    // Collects (pid, hwnd) for visible top-level windows. lparam is the &mut Vec below.
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: focus_pids passes a live Vec for the whole EnumWindows call.
        let found = unsafe { &mut *(lparam.0 as *mut Vec<(u32, isize)>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
            // Skip owned windows (dialogs and the like).
            let owned = unsafe { GetWindow(hwnd, GW_OWNER) }
                .map(|h| !h.0.is_null())
                .unwrap_or(false);
            if !owned {
                found.push((pid, hwnd.0 as isize));
            }
        }
        BOOL(1)
    }

    let mut found: Vec<(u32, isize)> = Vec::new();
    // SAFETY: visit only touches `found` through the lparam pointer, alive for this call.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut _ as isize)) };

    for &pid in pids {
        let Some(&(_, h)) = found.iter().find(|(p, _)| *p == pid) else {
            continue;
        };
        let hwnd = HWND(h as *mut c_void);
        // SAFETY: window and input calls on a window handle found above.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            // Windows refuses focus changes from background apps unless a key was just
            // pressed, so tap Alt first.
            keybd_event(VK_MENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            if SetForegroundWindow(hwnd).as_bool() {
                return format!("focused window of pid {pid}");
            }
            let info = FLASHWINFO {
                cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                hwnd,
                dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
                uCount: 3,
                dwTimeout: 0,
            };
            let _ = FlashWindowEx(&info);
        }
        return format!("could not focus window of pid {pid}, flashed it instead");
    }
    "no window found for the session".to_string()
}

#[cfg(not(windows))]
fn focus_pids(_pids: &[u32]) -> String {
    "not supported on this OS".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_000_000.0;

    fn sess(state: &str, age: f64, message: &str, project: &str, pids: &[u32]) -> Session {
        Session {
            state: state.into(),
            message: message.into(),
            project: project.into(),
            updated: NOW - age,
            pids: pids.to_vec(),
        }
    }

    fn map(items: Vec<(&str, Session)>) -> HashMap<String, Session> {
        items.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "deskhub-alerts-{}-{name}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_hook_file_and_rejects_bad_ones() {
        let s = parse_session(
            r#"{"state":"permission","cwd":"C:\\x","project":"p","message":"m","updated":5,"pids":[1,"x",4294967296,7]}"#,
        )
        .unwrap();
        assert_eq!(s.state, "permission");
        assert_eq!(s.project, "p");
        assert_eq!(s.updated, 5.0);
        assert_eq!(s.pids, vec![1, 7]);
        assert!(parse_session(r#"{"state":"idle"}"#).is_none(), "no updated");
        assert!(
            parse_session(r#"{"updated":"soon"}"#).is_none(),
            "updated not numeric"
        );
        assert!(parse_session("[1,2]").is_none(), "not an object");
        assert!(parse_session("not json").is_none());
    }

    #[test]
    fn sanitises_ids() {
        assert_eq!(sanitize_id("abc-DEF_09"), "abc-DEF_09");
        assert_eq!(sanitize_id("a b/../c.d"), "abcd");
        assert_eq!(sanitize_id("../.."), "");
    }

    #[test]
    fn folds_text_for_the_board() {
        assert_eq!(
            ascii_fold("Caf\u{e9}  \u{201C}ok\u{201D}\n\tnow\u{2026}", 120),
            "Cafe \"ok\" now..."
        );
        assert_eq!(
            ascii_fold("\u{4e2d}x", 120),
            "x",
            "unfoldable characters are dropped"
        );
        assert_eq!(ascii_fold("abcdef", 3), "abc");
        assert_eq!(ascii_fold("", 10), "");
    }

    #[test]
    fn permission_with_claude_message_is_the_title() {
        let m = pick(
            &map(vec![(
                "a",
                sess(
                    "permission",
                    20.0,
                    "Claude needs your permission to use Bash",
                    "alpha",
                    &[],
                ),
            )]),
            NOW,
            30.0,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            m,
            Alert {
                id: "a".into(),
                kind: "permission",
                title: "Claude needs your permission to use Bash".into(),
                detail: "".into(),
                project: "alpha".into(),
            }
        );
        assert_eq!(m.to_value()["agent"], "claude");
    }

    #[test]
    fn other_messages_go_under_the_generic_title() {
        let m = pick(
            &map(vec![("a", sess("input", 1.0, "Pick one", "p", &[]))]),
            NOW,
            30.0,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            (m.kind, m.title.as_str(), m.detail.as_str()),
            ("input", "Claude is waiting for you", "Pick one")
        );
        let idle = pick(
            &map(vec![("i", sess("idle", 40.0, "", "p", &[]))]),
            NOW,
            30.0,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            (idle.kind, idle.title.as_str(), idle.detail.as_str()),
            (
                "input",
                "Claude is waiting for you",
                "Finished. Waiting for your reply"
            )
        );
    }

    #[test]
    fn picks_newest_waiting_and_skips_the_rest() {
        let dismissed = HashMap::new();
        let sessions = map(vec![
            ("a", sess("permission", 20.0, "", "alpha", &[])),
            ("b", sess("input", 5.0, "", "beta", &[])),
            ("c", sess("working", 0.0, "", "gamma", &[])),
        ]);
        assert_eq!(pick(&sessions, NOW, 30.0, &dismissed).unwrap().id, "b");

        let old = map(vec![(
            "old",
            sess("permission", FRESH_S + 60.0, "", "p", &[]),
        )]);
        assert!(
            pick(&old, NOW, 30.0, &dismissed).is_none(),
            "older than 30 min ignored"
        );

        let fresh_idle = map(vec![("i", sess("idle", 5.0, "", "p", &[]))]);
        assert!(
            pick(&fresh_idle, NOW, 30.0, &dismissed).is_none(),
            "just finished does not alert"
        );
        let idle_40 = map(vec![("i", sess("idle", 40.0, "", "p", &[]))]);
        assert_eq!(pick(&idle_40, NOW, 30.0, &dismissed).unwrap().id, "i");
    }

    #[test]
    fn dismissed_alert_is_skipped_until_its_state_changes() {
        let mut t = Tracker::default();
        let first = map(vec![("a", sess("permission", 20.0, "", "alpha", &[]))]);
        assert_eq!(t.poll(first.clone(), NOW, 0, 30.0).unwrap()["id"], "a");
        assert!(
            t.poll(first.clone(), NOW, 0, 30.0).is_none(),
            "unchanged is not resent"
        );
        t.dismiss("a");
        assert_eq!(t.poll(first.clone(), NOW, 0, 30.0).unwrap(), clear_value());
        assert!(t.poll(first, NOW, 0, 30.0).is_none(), "clear sent once");

        let changed = map(vec![(
            "a",
            sess("input", 0.0, "Pick an option", "alpha", &[]),
        )]);
        let m = t.poll(changed, NOW, 0, 30.0).unwrap();
        assert_eq!(
            (m["id"].as_str(), m["detail"].as_str()),
            (Some("a"), Some("Pick an option"))
        );
    }

    #[test]
    fn dismiss_falls_back_to_older_session() {
        let mut t = Tracker::default();
        let both = map(vec![
            ("a", sess("permission", 20.0, "", "alpha", &[])),
            ("b", sess("input", 5.0, "", "beta", &[])),
        ]);
        assert_eq!(t.poll(both.clone(), NOW, 0, 30.0).unwrap()["id"], "b");
        t.dismiss("b");
        assert_eq!(t.poll(both, NOW, 0, 30.0).unwrap()["id"], "a");
    }

    #[test]
    fn front_window_marks_finished_session_seen() {
        let mut t = Tracker::default();
        let idle = map(vec![("v", sess("idle", 40.0, "", "viewed", &[1, 4242, 9]))]);
        assert!(
            t.poll(idle.clone(), NOW, 4242, 30.0).is_none(),
            "in front: treated as seen"
        );
        assert!(
            t.poll(idle, NOW, 0, 30.0).is_none(),
            "stays seen after switching away"
        );

        let perm = map(vec![(
            "v",
            sess("permission", 0.0, "", "viewed", &[1, 4242, 9]),
        )]);
        assert_eq!(
            t.poll(perm.clone(), NOW, 0, 30.0).unwrap()["id"],
            "v",
            "new state alerts again"
        );
        assert!(
            t.poll(perm, NOW, 4242, 30.0).is_none(),
            "permission stays while its window is in front"
        );
    }

    #[test]
    fn loads_fresh_sessions_and_deletes_stale_ones() {
        let root = temp_root("load");
        let hook = |updated: f64| {
            format!(
                r#"{{"state":"input","message":"m","project":"p","updated":{updated},"pids":[]}}"#
            )
        };
        std::fs::write(root.join("fresh.json"), hook(NOW - 60.0)).unwrap();
        std::fs::write(root.join("old.json"), hook(NOW - FRESH_S - 60.0)).unwrap();
        std::fs::write(root.join("ancient.json"), hook(NOW - STALE_S - 60.0)).unwrap();
        std::fs::write(root.join("bad.json"), "{").unwrap();
        std::fs::write(root.join("notes.txt"), hook(NOW)).unwrap();
        std::fs::write(root.join("we ird.json"), hook(NOW - 1.0)).unwrap();

        let loaded = load_sessions(&root, NOW);
        let mut ids: Vec<&str> = loaded.keys().map(String::as_str).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["fresh", "old", "weird"]);
        assert!(
            !root.join("ancient.json").exists(),
            "2+ hour old file deleted"
        );
        assert!(root.join("old.json").exists(), "30+ min old file kept");
        assert!(root.join("bad.json").exists(), "malformed file left alone");

        assert!(load_sessions(&root.join("missing"), NOW).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn publishes_only_changes() {
        let mut t = Tracker::default();
        assert!(
            t.poll(HashMap::new(), NOW, 0, 30.0).is_none(),
            "nothing waiting: nothing sent"
        );
        let waiting = map(vec![("a", sess("permission", 0.0, "", "p", &[]))]);
        assert!(t.poll(waiting.clone(), NOW, 0, 30.0).is_some());
        assert!(t.poll(waiting, NOW, 0, 30.0).is_none());
        assert_eq!(t.poll(HashMap::new(), NOW, 0, 30.0).unwrap(), clear_value());
    }
}
