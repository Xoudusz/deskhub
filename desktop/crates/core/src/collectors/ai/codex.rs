//! Codex usage from <codex home>/sessions/**/*.jsonl and archived_sessions (port of
//! codex_usage.py). Only token counts, rate limit readings, model names and timestamps are
//! read. Message text is never looked at.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::tail::Tail;
use super::time::parse_iso_ms;
use super::{contains, prune, summary, window, Usage};

#[derive(Default)]
pub struct Codex {
    tail: Tail,
    log: CodexLog,
}

#[derive(Default)]
struct CodexLog {
    /// Model id per session file, from its turn_context lines.
    models: HashMap<PathBuf, String>,
    /// (file, timestamp) pairs already counted.
    /// (file, stamp) -> timestamp, pruned with `usage`.
    seen: HashMap<(String, String), i64>,
    usage: Vec<Usage>,
    /// Newest rate limit reading (epoch ms) and its payload.
    limit_ts: i64,
    rate_limits: Option<Value>,
}

impl CodexLog {
    fn ingest(&mut self, path: &Path, line: &[u8]) {
        let is_ctx = contains(line, b"\"turn_context\"");
        if !is_ctx && !contains(line, b"\"token_count\"") {
            return;
        }
        let Ok(d) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let Some(payload) = d.get("payload").filter(|p| p.is_object()) else {
            return;
        };
        let kind = d.get("type").and_then(Value::as_str);
        if is_ctx && kind == Some("turn_context") {
            if let Some(model) = payload.get("model").and_then(Value::as_str) {
                self.models.insert(path.to_path_buf(), model.into());
            }
            return;
        }
        if kind != Some("event_msg")
            || payload.get("type").and_then(Value::as_str) != Some("token_count")
        {
            return;
        }
        let Some(stamp) = d.get("timestamp").and_then(Value::as_str) else {
            return;
        };
        let Some(ts) = parse_iso_ms(stamp) else {
            return;
        };
        if let Some(rl) = payload.get("rate_limits").filter(|r| r.is_object()) {
            if ts >= self.limit_ts {
                self.limit_ts = ts;
                self.rate_limits = Some(rl.clone());
            }
        }
        let Some(last) = payload
            .get("info")
            .and_then(|i| i.get("last_token_usage"))
            .filter(|l| l.is_object())
        else {
            return;
        };
        if self
            .seen
            .insert((path.to_string_lossy().into_owned(), stamp.to_string()), ts)
            .is_some()
        {
            return;
        }
        let output = super::num(last.get("output_tokens"));
        let total = match last.get("total_tokens") {
            Some(Value::Number(_)) => super::num(last.get("total_tokens")),
            _ => super::num(last.get("input_tokens")) + output,
        };
        let model = self.models.get(path).map_or("", String::as_str);
        self.usage.push(Usage {
            ts,
            model: model_name(model),
            total,
            output,
        });
    }

    fn limits(&self, now: i64) -> Value {
        match &self.rate_limits {
            Some(rl) => limits_from(rl, self.limit_ts, now),
            None => json!({}),
        }
    }
}

/// Short display name: "gpt-6.1-sol" -> "GPT-6.1 Sol" (15 characters at most).
fn model_name(model: &str) -> String {
    let parts: Vec<&str> = model.split('-').filter(|p| !p.is_empty()).collect();
    let Some(&first) = parts.first() else {
        return "Other".into();
    };
    let (mut name, rest) = if first.eq_ignore_ascii_case("gpt") && parts.len() > 1 {
        (format!("GPT-{}", parts[1]), &parts[2..])
    } else {
        let name = if first.chars().count() <= 3 {
            first.to_uppercase()
        } else {
            title(first)
        };
        (name, &parts[1..])
    };
    if !rest.is_empty() {
        let words: Vec<String> = rest.iter().map(|p| title(p)).collect();
        name.push(' ');
        name.push_str(&words.join(" "));
    }
    name.chars().take(15).collect()
}

/// Python's str.title() for one word: first letter upper, the rest lower.
fn title(word: &str) -> String {
    let mut out = String::new();
    for (i, c) in word.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Maps Codex's two windows onto five_hour (up to 360 min) and seven_day (1440 min or more).
/// Windows whose reset time has passed are dropped.
fn limits_from(rate_limits: &Value, ts_ms: i64, now: i64) -> Value {
    let now_s = now as f64 / 1000.0;
    let mut out = Map::new();
    for key in ["primary", "secondary"] {
        let Some(w) = rate_limits.get(key).filter(|w| w.is_object()) else {
            continue;
        };
        let field = |k: &str| w.get(k).and_then(Value::as_f64);
        let (Some(pct), Some(reset), Some(mins)) = (
            field("used_percent"),
            field("resets_at"),
            field("window_minutes"),
        ) else {
            continue;
        };
        if reset <= now_s {
            continue;
        }
        let name = if mins <= 360.0 {
            "five_hour"
        } else if mins >= 1440.0 {
            "seven_day"
        } else {
            continue;
        };
        out.insert(name.into(), window(pct, reset));
    }
    if out.is_empty() {
        return json!({});
    }
    out.insert("updated".into(), json!(ts_ms / 1000));
    Value::Object(out)
}

impl Codex {
    /// The Codex part of the "ai" message. `dir` is the Codex home ($CODEX_HOME or ~/.codex).
    pub fn snapshot(&mut self, dir: Option<&Path>, now: i64) -> Value {
        if let Some(dir) = dir {
            let roots = [dir.join("sessions"), dir.join("archived_sessions")];
            let log = &mut self.log;
            self.tail.scan(&roots, |path, line| log.ingest(path, line));
        }
        prune(&mut self.log.usage, now);
        let horizon = super::horizon(now);
        self.log.seen.retain(|_, ts| *ts >= horizon);
        let mut msg = summary(&self.log.usage, now);
        msg.insert("limits".into(), self.log.limits(now));
        msg.insert("type".into(), json!("codex"));
        msg.insert("block".into(), json!({"active": false}));
        Value::Object(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tail::temp_dir;
    use super::*;
    use std::fs;

    /// 2026-11-23T12:00:00Z in epoch seconds and milliseconds.
    const NOW_S: i64 = 1_795_435_200;
    const NOW_MS: i64 = NOW_S * 1000;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn model_names_are_short() {
        assert_eq!(model_name("gpt-6.1-sol"), "GPT-6.1 Sol");
        assert_eq!(model_name("o4-mini"), "O4 Mini");
        assert_eq!(model_name(""), "Other");
        assert_eq!(
            model_name("gpt-5-codex-preview-long-name"),
            "GPT-5 Codex Pre"
        );
    }

    #[test]
    fn golden_snapshot_with_rate_limits_and_archive() {
        let dir = temp_dir("codex-golden");
        let codex = dir.join(".codex");
        let session = codex
            .join("sessions")
            .join("2026")
            .join("11")
            .join("23")
            .join("a.jsonl");
        let lines = [
            json!({"timestamp": "2026-11-23T09:00:00Z", "type": "turn_context",
                   "payload": {"model": "gpt-6.1-sol"}}),
            json!({"timestamp": "2026-11-23T09:00:01Z", "type": "event_msg", "payload": {
                "type": "token_count",
                "info": {"last_token_usage": {"input_tokens": 1000, "output_tokens": 100,
                                              "total_tokens": 1100}},
                "rate_limits": {
                    "primary": {"used_percent": 12.34, "window_minutes": 300,
                                "resets_at": NOW_S + 3600},
                    "secondary": {"used_percent": 40.0, "window_minutes": 10080,
                                  "resets_at": NOW_S + 86400},
                },
            }}),
            // Same event written twice: counted once.
            json!({"timestamp": "2026-11-23T09:00:01Z", "type": "event_msg", "payload": {
                "type": "token_count",
                "info": {"last_token_usage": {"input_tokens": 1000, "output_tokens": 100,
                                              "total_tokens": 1100}},
            }}),
            // No total_tokens: input plus output. No rate limits, so the older reading stays.
            json!({"timestamp": "2026-11-23T11:30:00Z", "type": "event_msg", "payload": {
                "type": "token_count",
                "info": {"last_token_usage": {"input_tokens": 200, "output_tokens": 20}},
            }}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        write(&session, &(lines + "\n"));

        let archived = codex.join("archived_sessions").join("old.jsonl");
        let old = [
            json!({"timestamp": "2026-11-22T10:00:00Z", "type": "turn_context",
                   "payload": {"model": "o4-mini"}}),
            json!({"timestamp": "2026-11-22T10:00:05Z", "type": "event_msg", "payload": {
                "type": "token_count",
                "info": {"last_token_usage": {"input_tokens": 50, "output_tokens": 10,
                                              "total_tokens": 60}},
            }}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        write(&archived, &(old + "\n"));

        let snap = Codex::default().snapshot(Some(codex.as_path()), NOW_MS);
        let expected = json!({
            "limits": {
                "five_hour": {"pct": 12.3, "reset": NOW_S + 3600},
                "seven_day": {"pct": 40.0, "reset": NOW_S + 86400},
                // 09:00:01, the newest reading that carried limits.
                "updated": NOW_S - 3 * 3600 + 1,
            },
            "type": "codex",
            "today": {"tokens": 1320, "output": 120, "messages": 2},
            "block": {"active": false},
            "last_hour": 220,
            "days": [0, 0, 0, 0, 0, 60, 1320],
            "models": [{"name": "GPT-6.1 Sol", "tokens": 1320}],
            "last_active": NOW_S - 30 * 60,
        });
        assert_eq!(snap, expected);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_sessions_gives_idle_snapshot() {
        let dir = temp_dir("codex-empty");
        let snap = Codex::default().snapshot(Some(dir.as_path()), NOW_MS);
        assert_eq!(snap["limits"], json!({}));
        assert_eq!(snap["last_active"], 0);
        assert_eq!(snap["models"], json!([]));
        let _ = fs::remove_dir_all(&dir);
    }
}
