//! Claude Code usage from ~/.claude/projects/**/*.jsonl (port of claude_usage.py).
//! Only usage counts, model family and timestamps are read. Message text is never looked at.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::tail::Tail;
use super::time::{floor_hour, parse_iso_ms};
use super::{contains, num, prune, py_str, summary, window, Usage, BLOCK_MS};

#[derive(Default)]
pub struct Claude {
    tail: Tail,
    log: ClaudeLog,
}

#[derive(Default)]
struct ClaudeLog {
    /// "message id:request id" pairs already counted (a reply is written again while streaming).
    /// Key -> timestamp, pruned with `usage`.
    seen: HashMap<String, i64>,
    usage: Vec<Usage>,
}

impl ClaudeLog {
    fn ingest(&mut self, line: &[u8]) {
        if !contains(line, b"\"usage\"") {
            return;
        }
        let Ok(d) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if d.get("type").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        let Some(msg) = d.get("message") else { return };
        let Some(u) = msg.get("usage").filter(|u| u.is_object()) else {
            return;
        };
        let Some(ts) = d
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso_ms)
        else {
            return;
        };
        let key = format!("{}:{}", py_str(msg.get("id")), py_str(d.get("requestId")));
        if self.seen.insert(key, ts).is_some() {
            return;
        }
        let input = num(u.get("input_tokens"));
        let output = num(u.get("output_tokens"));
        let cache_create = num(u.get("cache_creation_input_tokens"));
        let cache_read = num(u.get("cache_read_input_tokens"));
        let model = family(msg.get("model").and_then(Value::as_str).unwrap_or(""));
        self.usage.push(Usage {
            ts,
            model: model.into(),
            total: input + output + cache_create + cache_read,
            output,
        });
    }
}

/// "Opus", "Sonnet", "Haiku", "Fable" or "Other", from the model id.
fn family(model: &str) -> &'static str {
    let m = model.to_lowercase();
    for (needle, name) in [
        ("fable", "Fable"),
        ("opus", "Opus"),
        ("sonnet", "Sonnet"),
        ("haiku", "Haiku"),
    ] {
        if m.contains(needle) {
            return name;
        }
    }
    "Other"
}

impl Claude {
    /// The Claude part of the "ai" message. `home` is the user's home dir; `now` is epoch ms.
    pub fn snapshot(&mut self, home: Option<&Path>, now: i64, live: Option<&Value>) -> Value {
        if let Some(home) = home {
            let log = &mut self.log;
            self.tail
                .scan(&[home.join(".claude").join("projects")], |_, line| {
                    log.ingest(line)
                });
        }
        prune(&mut self.log.usage, now);
        let horizon = super::horizon(now);
        self.log.seen.retain(|_, ts| *ts >= horizon);
        let usage = &self.log.usage;
        let limits = claude_limits(home, now, live);
        let block = match current_block(usage, now) {
            Some(start) => {
                let (tokens, messages) = usage
                    .iter()
                    .filter(|u| u.ts >= start)
                    .fold((0_i64, 0_i64), |(t, m), u| (t + u.total, m + 1));
                json!({
                    "active": true,
                    "start": start / 1000,
                    "reset": (start + BLOCK_MS) / 1000,
                    "tokens": tokens,
                    "messages": messages,
                })
            }
            None => json!({"active": false}),
        };
        let mut msg = summary(usage, now);
        msg.insert("limits".into(), limits);
        msg.insert("type".into(), json!("claude"));
        msg.insert("block".into(), block);
        Value::Object(msg)
    }
}

/// Start (epoch ms) of the 5-hour usage window that is still open, if any. A window starts at
/// the hour of the first message after the previous window ended.
fn current_block(usage: &[Usage], now: i64) -> Option<i64> {
    let mut start: Option<i64> = None;
    for u in usage {
        match start {
            Some(s) if u.ts < s + BLOCK_MS => {}
            _ => start = Some(floor_hour(u.ts)),
        }
    }
    start.filter(|s| now < s + BLOCK_MS)
}

/// Status line readings, or the live ones (ai/live.rs) when given and newer
/// (Python ClaudeUsage._limits).
fn claude_limits(home: Option<&Path>, now: i64, live: Option<&Value>) -> Value {
    let limits = read_limits(home, now);
    match live {
        Some(l) if num(l.get("updated")) >= num(limits.get("updated")) => l.clone(),
        _ => limits,
    }
}

/// Plan limits saved by the status line bridge in ~/.deskhub/claude_status.json. Windows
/// whose reset time has passed are dropped.
fn read_limits(home: Option<&Path>, now: i64) -> Value {
    let Some(path) = home.map(|h| h.join(".deskhub").join("claude_status.json")) else {
        return json!({});
    };
    let Ok(text) = fs::read_to_string(path) else {
        return json!({});
    };
    let Ok(data) = serde_json::from_str::<Value>(&text) else {
        return json!({});
    };
    let now_s = now as f64 / 1000.0;
    let mut out = serde_json::Map::new();
    for name in ["five_hour", "seven_day"] {
        let w = data.get("rate_limits").and_then(|r| r.get(name));
        let pct = w
            .and_then(|w| w.get("used_percentage"))
            .and_then(Value::as_f64);
        let reset = w.and_then(|w| w.get("resets_at")).and_then(Value::as_f64);
        if let (Some(pct), Some(reset)) = (pct, reset) {
            if reset > now_s {
                out.insert(name.into(), window(pct, reset));
            }
        }
    }
    if out.is_empty() {
        return json!({});
    }
    out.insert("updated".into(), json!(num(data.get("updated"))));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::super::tail::temp_dir;
    use super::*;
    use std::path::PathBuf;

    /// 2026-11-23T12:00:00Z in epoch seconds and milliseconds.
    const NOW_S: i64 = 1_795_435_200;
    const NOW_MS: i64 = NOW_S * 1000;

    fn write(path: &PathBuf, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn assistant(ts: &str, req: &str, id: &str, model: &str, usage: Value) -> String {
        json!({
            "type": "assistant",
            "timestamp": ts,
            "requestId": req,
            "message": {"id": id, "model": model, "usage": usage},
        })
        .to_string()
    }

    #[test]
    fn families_match_by_substring() {
        assert_eq!(family("claude-opus-4-1"), "Opus");
        assert_eq!(family("claude-sonnet-4-5"), "Sonnet");
        assert_eq!(family("claude-haiku-4-5"), "Haiku");
        assert_eq!(family("claude-fable-1"), "Fable");
        assert_eq!(family("mystery"), "Other");
        assert_eq!(family(""), "Other");
    }

    #[test]
    fn golden_snapshot_over_two_days_and_a_block_boundary() {
        let dir = temp_dir("claude-golden");
        let home = dir.clone();
        let projects = home.join(".claude").join("projects");

        // Two sessions in two project folders, plus lines that must be skipped.
        let a = [
            assistant(
                "2026-11-17T09:00:00Z",
                "r0",
                "m0",
                "claude-haiku-4-5",
                json!({"input_tokens": 3, "output_tokens": 0}),
            ),
            assistant(
                "2026-11-22T20:00:00Z",
                "r4",
                "m4",
                "claude-haiku-4-5",
                json!({"input_tokens": 5, "output_tokens": 5}),
            ),
            assistant(
                "2026-11-23T10:30:00Z",
                "r1",
                "m1",
                "claude-opus-4-1",
                json!({"input_tokens": 100, "output_tokens": 50,
                       "cache_creation_input_tokens": 10, "cache_read_input_tokens": 20}),
            ),
            // Same message written again while streaming: counted once.
            assistant(
                "2026-11-23T10:31:00Z",
                "r1",
                "m1",
                "claude-opus-4-1",
                json!({"input_tokens": 100, "output_tokens": 50}),
            ),
            // Not an assistant turn, and no usage object.
            json!({"type": "user", "timestamp": "2026-11-23T10:00:00Z", "note": "usage"})
                .to_string(),
            json!({"type": "assistant", "timestamp": "2026-11-23T10:00:00Z",
                   "message": {"id": "m9", "usage": "usage"}})
            .to_string(),
        ]
        .join("\n");
        write(&projects.join("proj-a").join("s1.jsonl"), &(a + "\n"));
        let b = [
            assistant(
                "2026-11-23T11:15:00.5Z",
                "r2",
                "m2",
                "claude-sonnet-4-5",
                json!({"input_tokens": 10, "output_tokens": 5}),
            ),
            assistant(
                "2026-11-23T11:30:00Z",
                "r3",
                "m3",
                "claude-opus-4-1",
                json!({"input_tokens": 1, "output_tokens": 1}),
            ),
        ]
        .join("\n");
        write(&projects.join("proj-b").join("s2.jsonl"), &(b + "\n"));

        // Plan limits from the status line bridge.
        let status = json!({
            "updated": NOW_S - 100,
            "rate_limits": {
                "five_hour": {"used_percentage": 42.345, "resets_at": NOW_S + 3600},
                "seven_day": {"used_percentage": 10, "resets_at": NOW_S + 86400},
            },
        });
        write(
            &home.join(".deskhub").join("claude_status.json"),
            &status.to_string(),
        );

        let snap = Claude::default().snapshot(Some(home.as_path()), NOW_MS, None);
        let expected = json!({
            "limits": {
                "five_hour": {"pct": 42.3, "reset": NOW_S + 3600},
                "seven_day": {"pct": 10.0, "reset": NOW_S + 86400},
                "updated": NOW_S - 100,
            },
            "type": "claude",
            // Turns from 10:00 on: 10:30 (180), 11:15 (15), 11:30 (2).
            "today": {"tokens": 197, "output": 56, "messages": 3},
            "block": {
                "active": true,
                "start": NOW_S - 2 * 3600,
                "reset": NOW_S + 3 * 3600,
                "tokens": 197,
                "messages": 3,
            },
            "last_hour": 17,
            "days": [3, 0, 0, 0, 0, 10, 197],
            "models": [{"name": "Opus", "tokens": 182}, {"name": "Sonnet", "tokens": 15}],
            "last_active": NOW_S - 30 * 60,
        });
        assert_eq!(snap, expected);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_transcripts_gives_idle_snapshot() {
        let dir = temp_dir("claude-empty");
        let snap = Claude::default().snapshot(Some(dir.as_path()), NOW_MS, None);
        assert_eq!(snap["block"], json!({"active": false}));
        assert_eq!(snap["days"], json!([0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(snap["limits"], json!({}));
        assert_eq!(snap["last_active"], 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn block_closes_after_five_hours() {
        let usage = |ts: i64| Usage {
            ts,
            model: "Opus".into(),
            total: 1,
            output: 0,
        };
        let start = floor_hour(NOW_MS - 6 * 3600 * 1000);
        // Last message 6 h ago: the window opened then has closed.
        assert_eq!(current_block(&[usage(start)], NOW_MS), None);
        // Message 2 h ago: window still open, started at its own hour.
        let recent = NOW_MS - 2 * 3600 * 1000;
        assert_eq!(
            current_block(&[usage(recent)], NOW_MS),
            Some(floor_hour(recent))
        );
    }
}
