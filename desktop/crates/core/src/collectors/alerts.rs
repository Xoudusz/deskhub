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
use super::remote;
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
    /// The server it runs on ("" = this PC). Server sessions have no window to show.
    pub host: String,
    /// A pending permission request's id ("" = none); the board's Allow / Deny names it.
    pub request: String,
    /// What the permission request asks for ("Bash: npm test"), "" when unknown.
    pub detail: String,
}

/// The alert shown on the board.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub detail: String,
    pub project: String,
    /// The board may offer Show (the session has a window on this PC).
    pub focus: bool,
    /// The board may offer Allow / Deny (a server's pending permission request).
    pub decide: bool,
}

impl Alert {
    pub fn to_value(&self) -> Value {
        let mut v = json!({
            "type": "alert",
            "id": self.id,
            "kind": self.kind,
            "agent": "claude",
            "title": self.title,
            "detail": self.detail,
            "project": self.project,
        });
        if !self.focus {
            v["focus"] = json!(false);
        }
        if self.decide {
            v["decide"] = json!(true);
        }
        v
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
        host: String::new(),
        request: str_of("request"),
        detail: str_of("detail"),
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
    // What a permission request asks for ("Bash: npm test"), when the hook sent it.
    let detail = match d.detail.trim() {
        asked if permission && !asked.is_empty() => asked,
        _ => detail,
    };
    // A server's session says where it runs: "vibecode - deskhub".
    let project = if d.host.is_empty() {
        d.project.clone()
    } else if d.project.is_empty() {
        d.host.clone()
    } else {
        format!("{} - {}", d.host, d.project)
    };
    Some(Alert {
        id: sid.clone(),
        kind,
        title: ascii_fold(title, 60),
        detail: ascii_fold(detail, 120),
        project: ascii_fold(&project, 40),
        focus: d.host.is_empty(),
        decide: permission && !d.host.is_empty() && !d.request.is_empty(),
    })
}

/// Text the board can draw, at most `limit` bytes (crate::text::board_text): Latin-1
/// letters such as ä ö ü ß stay, other letters fold to their base, emoji are dropped.
pub fn ascii_fold(s: &str, limit: usize) -> String {
    crate::text::board_text(s, limit)
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
    /// Settings > AI "Show on PC opens" is set: a server's alerts get Show on PC, not
    /// Allow / Deny.
    pub open_remote: bool,
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
        let new = pick(&self.sessions, now, idle_after, &self.dismissed).map(|mut a| {
            if self.open_remote && self.is_remote(&a.id) {
                a.focus = true;
                a.decide = false;
            }
            a
        });
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

    /// Whether `sid` is a server's session (nothing to show on this PC).
    pub fn is_remote(&self, sid: &str) -> bool {
        self.sessions.get(sid).is_some_and(|d| !d.host.is_empty())
    }

    /// For a server's pending permission request: (the server's session id, request id).
    pub fn remote_request(&self, sid: &str) -> Option<(String, String)> {
        let d = self.sessions.get(sid)?;
        let id = sid.strip_prefix("r-")?;
        (!d.host.is_empty() && d.state == "permission" && !d.request.is_empty())
            .then(|| (id.to_string(), d.request.clone()))
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
        remote,
    } = ctx;
    let root = sessions_dir();
    let mut tracker = Tracker::default();
    let mut off_published = false;
    let mut incoming_open = true;
    let mut ticker = tokio::time::interval(POLL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        // borrow_and_update marks the value seen, so changed() below waits for the next change.
        let (enabled, idle_after, remote_open) = {
            let s = settings.borrow_and_update();
            (s.ai.alerts, s.ai.idle_alert_seconds.max(0) as f64, s.ai.remote_open.trim().to_string())
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
        tracker.open_remote = !remote_open.is_empty();

        tokio::select! {
            _ = ticker.tick() => {
                let now = now_secs();
                let mut sessions = load_async(root.clone(), now).await;
                // Claude Code on a server (Settings > AI), under ids of their own.
                for (id, s) in &remote.borrow().sessions {
                    sessions.insert(format!("r-{id}"), s.clone());
                }
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
                            // A server's session has no window here: Show brings up the app
                            // from Settings > AI (an SSH client, say), or only dismisses.
                            if !tracker.is_remote(&sid) {
                                focus_async(root.clone(), sid.clone(), now_secs()).await;
                            } else if !remote_open.is_empty() {
                                let (target, id) = (remote_open.clone(), sid.clone());
                                tokio::task::spawn_blocking(move || log::info!("alert {id}: {}", show_app(&target)));
                            }
                            // Focusing counts as seen.
                            tracker.dismiss(&sid);
                        }
                        // Allow / Deny on the board: a server's permission request.
                        "allow" | "deny" => {
                            if let Some((rid, request)) = tracker.remote_request(&sid) {
                                let url = settings.borrow().ai.remote_url.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = remote::decide(&url, &rid, &request, &action).await {
                                        log::warn!("board decision: {e:#}");
                                    }
                                });
                            }
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

/// Visible, unowned top-level windows as (pid, hwnd), in z-order.
#[cfg(windows)]
fn top_windows() -> Vec<(u32, isize)> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
    };

    // Collects (pid, hwnd) for visible top-level windows. lparam is the &mut Vec below.
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: top_windows passes a live Vec for the whole EnumWindows call.
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
    found
}

/// Restore and focus a window; flashes it when Windows refuses the focus change.
#[cfg(windows)]
fn bring_to_front(h: isize) -> bool {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, IsIconic, SetForegroundWindow, ShowWindow, FLASHWINFO, FLASHW_ALL,
        FLASHW_TIMERNOFG, SW_RESTORE,
    };

    let hwnd = HWND(h as *mut c_void);
    // SAFETY: window and input calls on a window handle from top_windows.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        // Windows refuses focus changes from background apps unless a key was just
        // pressed, so tap Alt first.
        keybd_event(VK_MENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        if SetForegroundWindow(hwnd).as_bool() {
            return true;
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
    false
}

/// Bring the window of the first pid that owns a visible top-level window to the front.
#[cfg(windows)]
fn focus_pids(pids: &[u32]) -> String {
    let found = top_windows();
    for &pid in pids {
        let Some(&(_, h)) = found.iter().find(|(p, _)| *p == pid) else {
            continue;
        };
        return if bring_to_front(h) {
            format!("focused window of pid {pid}")
        } else {
            format!("could not focus window of pid {pid}, flashed it instead")
        };
    }
    "no window found for the session".to_string()
}

#[cfg(not(windows))]
fn focus_pids(_pids: &[u32]) -> String {
    "not supported on this OS".to_string()
}

/// The lower-case program file name in `target` ("Termius.exe", "C:\...\Termius.exe"), if
/// it names an .exe.
pub fn exe_of(target: &str) -> Option<String> {
    let t = target.trim().trim_matches('"');
    let name = t.rsplit(['\\', '/']).next().unwrap_or(t).to_ascii_lowercase();
    (name.len() > 4 && name.ends_with(".exe")).then_some(name)
}

/// The full path of a process's program, if this user may ask.
#[cfg(windows)]
fn exe_path(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: the handle is closed below; the buffer outlives the call.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Settings > AI "Show on PC opens": focus the program's window when it runs (an .exe
/// target), else open the target like an "open" tile (program, path or URL).
#[cfg(windows)]
fn show_app(target: &str) -> String {
    if let Some(exe) = exe_of(target) {
        for (pid, h) in top_windows() {
            if exe_path(pid).and_then(|p| exe_of(&p)).as_deref() == Some(exe.as_str()) {
                return if bring_to_front(h) {
                    format!("focused {exe}")
                } else {
                    format!("could not focus {exe}, flashed it instead")
                };
            }
        }
    }
    super::shortcuts::open_target(target.trim().trim_matches('"'));
    format!("opened {target}")
}

#[cfg(not(windows))]
fn show_app(target: &str) -> String {
    super::shortcuts::open_target(target);
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
            host: String::new(),
            request: String::new(),
            detail: String::new(),
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
    fn server_sessions_are_labelled_and_have_no_show() {
        let mut s = sess("permission", 5.0, "Claude needs your permission to use Bash", "deskhub", &[]);
        s.host = "vibecode".into();
        let mut t = Tracker::default();
        let msg = t
            .poll(map(vec![("r-x", s.clone())]), NOW, 0, 30.0)
            .expect("an alert");
        assert_eq!(msg["project"], "vibecode - deskhub");
        assert_eq!(msg["focus"], false);
        assert!(t.is_remote("r-x"));
        assert!(!t.is_remote("nope"));
        s.project.clear();
        let a = pick(&map(vec![("r-x", s)]), NOW, 30.0, &HashMap::new()).unwrap();
        assert_eq!(a.project, "vibecode");
    }

    #[test]
    fn show_on_pc_opens_an_app_for_a_servers_alert() {
        let mut s = sess("permission", 5.0, "Claude wants to use Bash", "deskhub", &[]);
        s.host = "vibecode".into();
        s.request = "abc123".into();
        let mut t = Tracker { open_remote: true, ..Default::default() };
        let msg = t.poll(map(vec![("r-x1", s.clone())]), NOW, 0, 30.0).expect("an alert");
        // No Allow / Deny: the board shows Show on PC.
        assert!(msg.get("decide").is_none());
        assert!(msg.get("focus").is_none());
        // Turning it off brings Allow / Deny back on the next poll.
        t.open_remote = false;
        let msg = t.poll(map(vec![("r-x1", s)]), NOW, 0, 30.0).expect("changed");
        assert_eq!((msg["decide"].as_bool(), msg["focus"].as_bool()), (Some(true), Some(false)));
    }

    #[test]
    fn exe_names() {
        assert_eq!(exe_of("Termius.exe").as_deref(), Some("termius.exe"));
        assert_eq!(exe_of(r#""C:\Users\me\AppData\Local\Programs\Termius\Termius.exe""#).as_deref(), Some("termius.exe"));
        assert_eq!(exe_of("termius://"), None);
        assert_eq!(exe_of("C:/tools/"), None);
        assert_eq!(exe_of(".exe"), None);
    }

    #[test]
    fn a_servers_permission_request_can_be_answered_on_the_board() {
        let mut s = sess("permission", 5.0, "Claude wants to use Bash", "deskhub", &[]);
        s.host = "vibecode".into();
        s.request = "abc123".into();
        s.detail = "Bash: npm test".into();
        let mut t = Tracker::default();
        let msg = t.poll(map(vec![("r-x1", s.clone())]), NOW, 0, 30.0).expect("an alert");
        assert_eq!(msg["decide"], true);
        assert_eq!(msg["title"], "Claude wants to use Bash");
        assert_eq!(msg["detail"], "Bash: npm test");
        assert_eq!(t.remote_request("r-x1"), Some(("x1".into(), "abc123".into())));
        assert_eq!(t.remote_request("nope"), None);
        // This PC's sessions are answered in their window: no Allow / Deny.
        let mut local = s.clone();
        local.host.clear();
        let a = pick(&map(vec![("x2", local)]), NOW, 30.0, &HashMap::new()).unwrap();
        assert!(!a.decide);
        assert!(a.to_value().get("decide").is_none());
        // No request id (older hook): nothing to answer.
        s.request.clear();
        let a = pick(&map(vec![("r-x1", s)]), NOW, 30.0, &HashMap::new()).unwrap();
        assert!(!a.decide);
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
            "Caf\u{e9} \u{201C}ok\u{201D} now\u{2026}"
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
                focus: true,
                decide: false,
            }
        );
        assert_eq!(m.to_value()["agent"], "claude");
        assert!(m.to_value().get("focus").is_none());
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
