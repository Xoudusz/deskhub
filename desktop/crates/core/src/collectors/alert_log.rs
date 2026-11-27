//! Claude alert log: every hook event the app sees (on this PC and on a server) and every
//! alert the board shows, in ~/.deskhub/alert_log.jsonl. Settings > AI lists the newest.
//! Stored: time, PC or server, folder, state, cause (the hook event) and the alert text.
//! Never prompts or tool input (the session files do not hold them either).

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::alerts::{Alert, Session};

pub const FILE: &str = "alert_log.jsonl";
/// Past this size the file keeps only its newest KEEP_LINES lines.
const MAX_BYTES: u64 = 1_000_000;
const KEEP_LINES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds.
    pub t: f64,
    /// "event" (a hook wrote the session), "shown" / "cleared" (the board's alert),
    /// "dismissed" / "show" / "allow" / "deny" (tapped on the board).
    pub kind: String,
    /// "PC" or the server's name.
    pub source: String,
    pub project: String,
    pub state: String,
    /// The hook event: "Stop", "Notification: idle_prompt", "PermissionRequest: Bash".
    /// Empty from an older deskhub-cli or relay.
    #[serde(default)]
    pub cause: String,
    #[serde(default)]
    pub text: String,
}

/// ~/.deskhub/alert_log.jsonl
pub fn path() -> Option<PathBuf> {
    Some(super::alerts::deskhub_dir()?.join(FILE))
}

fn source(s: &Session) -> String {
    if s.host.is_empty() {
        "PC".into()
    } else {
        s.host.clone()
    }
}

/// An entry for one session (an event, or a tap on its alert).
pub fn session_entry(kind: &str, s: &Session, t: f64) -> Entry {
    let text = if s.detail.is_empty() {
        s.message.clone()
    } else if s.message.is_empty() {
        s.detail.clone()
    } else {
        format!("{} ({})", s.message, s.detail)
    };
    Entry {
        t,
        kind: kind.into(),
        source: source(s),
        project: s.project.clone(),
        state: s.state.clone(),
        cause: s.cause.clone(),
        text,
    }
}

/// An entry for the board's alert: shown or cleared.
pub fn alert_entry(kind: &str, a: &Alert, s: Option<&Session>, t: f64) -> Entry {
    let text = if a.detail.is_empty() {
        a.title.clone()
    } else {
        format!("{} - {}", a.title, a.detail)
    };
    Entry {
        t,
        kind: kind.into(),
        source: s.map_or_else(|| "PC".into(), source),
        project: a.project.clone(),
        state: s.map_or_else(|| a.kind.to_string(), |s| s.state.clone()),
        cause: s.map(|s| s.cause.clone()).unwrap_or_default(),
        text,
    }
}

/// Turns each poll's sessions into "event" entries: one per session whose record changed.
/// A working session logs only when its state or cause changes (tool after tool is one
/// line); a waiting one logs every rewrite, since every rewrite can alert again.
#[derive(Debug, Default)]
pub struct Watch {
    /// Session id -> (state, cause, updated) from the last poll.
    last: HashMap<String, (String, String, f64)>,
    primed: bool,
}

impl Watch {
    pub fn events(&mut self, sessions: &HashMap<String, Session>) -> Vec<Entry> {
        let mut out = Vec::new();
        for (id, s) in sessions {
            let changed = match self.last.get(id) {
                // The first poll only learns what is there: those records are old news.
                None => self.primed,
                Some((state, cause, updated)) => {
                    *updated != s.updated
                        && !(s.state == "working" && *state == s.state && *cause == s.cause)
                }
            };
            if changed {
                out.push(session_entry("event", s, s.updated));
            }
        }
        // A working session's skipped rewrites still move `updated`; keep the latest.
        self.last = sessions
            .iter()
            .map(|(id, s)| (id.clone(), (s.state.clone(), s.cause.clone(), s.updated)))
            .collect();
        self.primed = true;
        out.sort_by(|a, b| a.t.total_cmp(&b.t));
        out
    }
}

/// Append entries; past MAX_BYTES keep the newest KEEP_LINES lines.
pub fn append(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut buf = String::new();
    for e in entries {
        buf.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
        buf.push('\n');
    }
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(buf.as_bytes())?;
    drop(f);
    if fs::metadata(path)?.len() > MAX_BYTES {
        let text = fs::read_to_string(path)?;
        let lines: Vec<&str> = text.lines().collect();
        let keep = &lines[lines.len().saturating_sub(KEEP_LINES)..];
        fs::write(path, keep.join("\n") + "\n")?;
    }
    Ok(())
}

/// Opens the folder that holds the log (~/.deskhub) in the file manager.
pub fn open_folder(path: &Path) {
    if let Some(dir) = path.parent() {
        super::shortcuts::open_target(&dir.to_string_lossy());
    }
}

/// The newest `limit` entries, newest first. Unreadable lines are skipped.
pub fn recent(path: &Path, limit: usize) -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str(l).ok())
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(state: &str, cause: &str, updated: f64) -> Session {
        Session {
            state: state.into(),
            message: String::new(),
            project: "my-proj".into(),
            updated,
            pids: Vec::new(),
            host: String::new(),
            request: String::new(),
            detail: String::new(),
            cause: cause.into(),
        }
    }

    fn map(v: &[(&str, Session)]) -> HashMap<String, Session> {
        v.iter().map(|(k, s)| (k.to_string(), s.clone())).collect()
    }

    #[test]
    fn logs_changes_but_not_what_was_there_at_start() {
        let mut w = Watch::default();
        assert!(w.events(&map(&[("a", sess("idle", "Stop", 1.0))])).is_empty());
        // Nothing changed.
        assert!(w.events(&map(&[("a", sess("idle", "Stop", 1.0))])).is_empty());
        // A new session and a rewrite of a waiting one.
        let e = w.events(&map(&[
            ("a", sess("input", "Notification: idle_prompt", 3.0)),
            ("b", sess("working", "UserPromptSubmit", 2.0)),
        ]));
        let causes: Vec<_> = e.iter().map(|e| e.cause.as_str()).collect();
        assert_eq!(causes, ["UserPromptSubmit", "Notification: idle_prompt"]);
        assert_eq!(e[1].source, "PC");
        assert_eq!(e[1].project, "my-proj");
    }

    #[test]
    fn tool_after_tool_is_one_line_and_waiting_rewrites_all_log() {
        let mut w = Watch::default();
        w.events(&map(&[("a", sess("working", "PostToolUse: Bash", 1.0))]));
        assert!(w.events(&map(&[("a", sess("working", "PostToolUse: Bash", 2.0))])).is_empty());
        assert_eq!(w.events(&map(&[("a", sess("working", "PostToolUse: Edit", 3.0))])).len(), 1);
        assert_eq!(w.events(&map(&[("a", sess("idle", "Stop", 4.0))])).len(), 1);
        assert_eq!(w.events(&map(&[("a", sess("idle", "Stop", 5.0))])).len(), 1);
    }

    #[test]
    fn server_sessions_name_their_host() {
        let mut s = sess("permission", "PermissionRequest: Bash", 1.0);
        s.host = "vibecode".into();
        s.message = "Claude wants to use Bash".into();
        s.detail = "Bash: ls".into();
        let e = session_entry("event", &s, 1.0);
        assert_eq!(e.source, "vibecode");
        assert_eq!(e.text, "Claude wants to use Bash (Bash: ls)");
    }

    #[test]
    fn appends_trims_and_reads_newest_first() {
        let dir = std::env::temp_dir().join(format!("deskhub-alertlog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let p = dir.join(FILE);
        assert!(recent(&p, 5).is_empty());
        let e = |t: f64| session_entry("event", &sess("idle", "Stop", t), t);
        append(&p, &[e(1.0), e(2.0)]).unwrap();
        append(&p, &[e(3.0)]).unwrap();
        fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"not json\n").unwrap();
        let r = recent(&p, 2);
        assert_eq!(r.iter().map(|e| e.t).collect::<Vec<_>>(), [3.0, 2.0]);
        // Past MAX_BYTES only the newest KEEP_LINES stay.
        let many: Vec<Entry> = (0..12000).map(|i| e(f64::from(i))).collect();
        append(&p, &many).unwrap();
        assert!(fs::metadata(&p).unwrap().len() <= MAX_BYTES);
        let text = fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), KEEP_LINES);
        assert_eq!(recent(&p, 1)[0].t, 11999.0);
        let _ = fs::remove_dir_all(&dir);
    }
}
