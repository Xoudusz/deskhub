//! Claude Code and Codex usage -> "ai" message (key "ai", every 5 s). Port of
//! companion/deskhub/claude_usage.py, codex_usage.py, pick_ai() and claude_loop in
//! __main__.py. File reads run on a blocking thread.
//!
//! Differences from the Python:
//! - Days (`days`, `today`, today's `models`) are local days via chrono, like Python.
//! - claude_live.py lives in ai/live.rs (opt-in `ai.live_limits`).
//! - Codex is enabled when its home dir exists, checked every tick (Python checks once at start).
//! - `block.start` is included, as in the Python output.

mod claude;
mod codex;
mod live;
mod tail;
mod time;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};

use super::Ctx;
pub use claude::Claude;
use codex::Codex;
use time::{day_of, now_ms, DAY_MS, HOUR_MS};

const HISTORY_DAYS: i64 = 7;
const BLOCK_MS: i64 = 5 * HOUR_MS;
/// Claude Code used within this counts as active (live limits poll every minute).
const LIVE_RECENT_MS: i64 = 15 * 60 * 1000;

/// One token-usage record from either provider. Only counts, model name and time are kept.
/// A record from a server (collectors/remote.rs) sums one hour: `ts` is the hour start and
/// `n` its message count; local records are single messages (`n` = 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub ts: i64,
    pub model: String,
    pub total: i64,
    pub output: i64,
    pub n: i64,
}

/// Collector state, moved into the blocking thread and back on every tick.
struct State {
    home: Option<PathBuf>,
    claude: Claude,
    codex: Codex,
}

impl State {
    fn new() -> Self {
        Self {
            home: home_dir(),
            claude: Claude::default(),
            codex: Codex::default(),
        }
    }
}

pub async fn run(ctx: Ctx) {
    let mut state: Option<State> = None;
    let mut live_poll = live::LivePoll::default();
    // Epoch s of Claude Code's last use: live limits poll every minute while it is recent.
    let mut claude_last = 0_i64;
    loop {
        // Copy the AI settings out of the watch borrow before any await.
        let ai = ctx.settings.borrow().ai.clone();
        let agent: &'static str = match ai.agent.as_str() {
            "claude" => "claude",
            "codex" => "codex",
            _ => "auto",
        };
        let st = state.take().unwrap_or_else(State::new);
        // Claude Code on a server (Settings > AI): hourly records merged into the numbers.
        let remote = ctx.remote.borrow().usage.clone();
        let live = if ai.live_limits && agent != "codex" {
            let now = now_ms();
            let active = now - claude_last * 1000 < LIVE_RECENT_MS;
            live_poll.tick(st.home.clone(), now, active).await
        } else {
            None
        };
        let work = tokio::task::spawn_blocking(move || {
            let mut st = st;
            let now = now_ms();
            let cdir = codex_dir(st.home.as_deref());
            let codex_on =
                agent == "codex" || (agent == "auto" && cdir.as_deref().is_some_and(Path::is_dir));
            let claude = (agent != "codex")
                .then(|| st.claude.snapshot(st.home.as_deref(), now, live.as_ref(), &remote));
            let codex = codex_on.then(|| st.codex.snapshot(cdir.as_deref(), now));
            (st, claude, codex)
        });
        match work.await {
            Ok((st, claude, codex)) => {
                state = Some(st);
                if let Some(t) = claude.as_ref().and_then(|c| c.get("last_active")) {
                    claude_last = num(Some(t));
                }
                if let Some(msg) = pick_ai(agent, claude.as_ref(), codex.as_ref()) {
                    ctx.out.publish("ai", msg);
                }
            }
            Err(e) => log::warn!("AI usage failed: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Which agent the board shows (pick_ai in __main__.py). "auto" shows whichever was used
/// last (Claude on a tie). A forced agent is shown when it has data.
fn pick_ai(agent: &str, claude: Option<&Value>, codex: Option<&Value>) -> Option<Value> {
    let last = |v: Option<&Value>| {
        v.and_then(|v| v.get("last_active"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    let codex_forced = agent == "codex" && codex.is_some();
    let codex_newer =
        agent == "auto" && codex.is_some() && claude.is_some() && last(codex) > last(claude);
    let provider = if codex_forced || codex_newer {
        "codex"
    } else if claude.is_some() {
        "claude"
    } else {
        "codex"
    };
    let snap = if provider == "codex" { codex } else { claude }?;
    let mut msg = snap.as_object()?.clone();
    msg.insert("type".into(), json!("ai"));
    msg.insert("provider".into(), json!(provider));
    Some(Value::Object(msg))
}

/// Oldest timestamp kept: the start of the history window.
fn horizon(now: i64) -> i64 {
    now - HISTORY_DAYS * DAY_MS
}

/// Sorts by time and drops records older than the history window.
fn prune(usage: &mut Vec<Usage>, now: i64) {
    let horizon = horizon(now);
    usage.retain(|u| u.ts >= horizon);
    usage.sort_by_key(|u| u.ts);
}

/// The fields both providers share: today, last hour, the 7-day series, today's models
/// (top 4) and the last activity time. `usage` must be sorted.
fn summary(usage: &[Usage], now: i64) -> Map<String, Value> {
    let today = day_of(now);
    let first = today - (HISTORY_DAYS - 1);
    let mut days = vec![0_i64; HISTORY_DAYS as usize];
    let (mut tokens, mut output, mut messages, mut last_hour) = (0_i64, 0_i64, 0_i64, 0_i64);
    let mut models: Vec<(String, i64)> = Vec::new();
    for u in usage {
        let day = day_of(u.ts);
        if (first..=today).contains(&day) {
            days[(day - first) as usize] += u.total;
        }
        if day == today {
            tokens += u.total;
            output += u.output;
            messages += u.n;
            match models.iter_mut().find(|(name, _)| *name == u.model) {
                Some((_, total)) => *total += u.total,
                None => models.push((u.model.clone(), u.total)),
            }
        }
        if now - u.ts <= HOUR_MS {
            last_hour += u.total;
        }
    }
    models.sort_by_key(|m| std::cmp::Reverse(m.1));
    let models: Vec<Value> = models
        .into_iter()
        .filter(|(_, total)| *total > 0)
        .take(4)
        .map(|(name, total)| json!({"name": name, "tokens": total}))
        .collect();
    let mut m = Map::new();
    m.insert(
        "today".into(),
        json!({"tokens": tokens, "output": output, "messages": messages}),
    );
    m.insert("last_hour".into(), json!(last_hour));
    m.insert("days".into(), json!(days));
    m.insert("models".into(), json!(models));
    m.insert(
        "last_active".into(),
        json!(usage.last().map_or(0, |u| u.ts / 1000)),
    );
    m
}

/// `{pct, reset}` for one plan window; pct is rounded to one decimal like Python's round(x, 1).
fn window(pct: f64, reset: f64) -> Value {
    json!({"pct": (pct * 10.0).round() / 10.0, "reset": reset as i64})
}

/// Whether a JSON number field is an integer or float, read as i64 (Python's `int(x or 0)`).
fn num(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

/// Python's str() for a JSON value, used for dedupe keys ("None" when missing).
fn py_str(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The user's home: USERPROFILE on Windows, HOME elsewhere.
fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// $CODEX_HOME, or ~/.codex.
fn codex_dir(home: Option<&Path>) -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => home.map(|h| h.join(".codex")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(last: i64) -> Value {
        json!({"type": "x", "last_active": last, "limits": {}})
    }

    #[test]
    fn auto_shows_most_recently_used() {
        let (c, x) = (agent(100), agent(200));
        let msg = pick_ai("auto", Some(&c), Some(&x)).unwrap();
        assert_eq!(msg["provider"], "codex");
        assert_eq!(msg["type"], "ai");
        let msg = pick_ai("auto", Some(&x), Some(&c)).unwrap();
        assert_eq!(msg["provider"], "claude");
    }

    #[test]
    fn auto_ties_go_to_claude() {
        let (c, x) = (agent(100), agent(100));
        assert_eq!(
            pick_ai("auto", Some(&c), Some(&x)).unwrap()["provider"],
            "claude"
        );
    }

    #[test]
    fn forced_agent_wins_over_recency() {
        let (c, x) = (agent(100), agent(200));
        assert_eq!(
            pick_ai("claude", Some(&c), Some(&x)).unwrap()["provider"],
            "claude"
        );
        assert_eq!(
            pick_ai("codex", Some(&c), Some(&x)).unwrap()["provider"],
            "codex"
        );
    }

    #[test]
    fn falls_back_to_whichever_exists() {
        let (c, x) = (agent(100), agent(200));
        assert_eq!(
            pick_ai("auto", Some(&c), None).unwrap()["provider"],
            "claude"
        );
        assert_eq!(
            pick_ai("auto", None, Some(&x)).unwrap()["provider"],
            "codex"
        );
        assert!(pick_ai("auto", None, None).is_none());
    }

    #[test]
    fn picked_message_keeps_the_snapshot_fields() {
        let c = agent(100);
        let msg = pick_ai("claude", Some(&c), None).unwrap();
        assert_eq!(msg["last_active"], 100);
        assert_eq!(msg["limits"], json!({}));
    }
}
