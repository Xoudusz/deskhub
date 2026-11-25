//! Opt-in live Claude plan limits from Anthropic (port of companion/deskhub/claude_live.py).
//!
//! Off unless `ai.live_limits` is on. Reads the access token Claude Code saved in
//! ~/.claude/.credentials.json and asks https://api.anthropic.com/api/oauth/usage for the
//! 5-hour and weekly percentages, the lookup Claude's own apps use for their usage bars.
//!
//! - No message is sent to Claude, so this uses no tokens and no API credits.
//! - The token stays in memory. It is never logged, saved elsewhere, or sent to the board.
//! - The token is never refreshed here (that could sign Claude Code out). When it has expired
//!   or was rejected, `claude auth status` has Claude Code renew its own sign-in, at most
//!   every 10 minutes. Its output (which names the account) is discarded.
//! - The endpoint is undocumented and may change: failures fall back quietly to the status
//!   line readings.
//! - A poll can take up to 15 s (a renew up to 60 s), so it runs on its own blocking thread
//!   (`LivePoll`) and the usage ticks never wait for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use tokio::task::JoinHandle;

use super::time::parse_iso_ms;
use super::window;

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Between attempts to have Claude Code renew its sign-in.
const RENEW_EVERY_MS: i64 = 10 * 60 * 1000;
/// A reading is shown at most this long after it was fetched.
const FRESH_MS: i64 = 15 * 60 * 1000;
/// Response bodies larger than this are refused.
const MAX_BODY: usize = 64 * 1024;

/// How one poll went (the result of the HTTP request).
enum Fetch {
    Ok(Value),
    Unauthorized,
    RateLimited,
    Failed(String),
}

/// Runs `LiveLimits` polls on a blocking thread of their own. `tick` is called every usage
/// tick; it starts a poll when none is running and returns the latest reading (at most
/// 15 minutes old).
#[derive(Default)]
pub struct LivePoll {
    idle: Option<LiveLimits>,
    task: Option<JoinHandle<(LiveLimits, Option<Value>)>>,
    value: Option<Value>,
}

impl LivePoll {
    pub async fn tick(&mut self, home: Option<PathBuf>, now: i64, active: bool) -> Option<Value> {
        if self.task.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(task) = self.task.take() {
                match task.await {
                    Ok((limits, value)) => {
                        self.idle = Some(limits);
                        self.value = value;
                    }
                    Err(e) => {
                        log::warn!("live Claude limits failed: {e}");
                        // Its state is lost: wait before the next poll and the next renew.
                        self.idle = Some(LiveLimits {
                            next: now + 300_000,
                            next_renew: now + RENEW_EVERY_MS,
                            ..Default::default()
                        });
                    }
                }
            }
        }
        if self.task.is_none() {
            let mut limits = self.idle.take().unwrap_or_default();
            self.task = Some(tokio::task::spawn_blocking(move || {
                let value = limits.get(home.as_deref(), now, active);
                (limits, value)
            }));
        }
        fresh(self.value.as_ref(), now)
    }
}

/// `v` while its `updated` (epoch s) is less than 15 minutes before `now` (epoch ms).
fn fresh(v: Option<&Value>, now: i64) -> Option<Value> {
    let v = v?;
    let updated = v.get("updated").and_then(Value::as_i64).unwrap_or(0) * 1000;
    (now - updated < FRESH_MS).then(|| v.clone())
}

pub struct LiveLimits {
    /// Epoch ms of the next poll.
    next: i64,
    /// Last result: {five_hour, seven_day, updated}.
    last: Option<Value>,
    /// Credentials file mtime that got a 401; the token is stale until the file changes.
    bad_mtime: Option<SystemTime>,
    /// Last logged problem, so each is logged once.
    state: String,
    /// Epoch ms before which no renew is attempted.
    next_renew: i64,
    /// The HTTP request and the sign-in renew (replaced in tests).
    fetch: fn(&str) -> Fetch,
    renew: fn() -> bool,
}

impl Default for LiveLimits {
    fn default() -> Self {
        Self {
            next: 0,
            last: None,
            bad_mtime: None,
            state: String::new(),
            next_renew: 0,
            fetch,
            renew: renew_signin,
        }
    }
}

impl LiveLimits {
    /// Latest limits, or None. Polls every minute while Claude Code is in use (`active`),
    /// every 5 minutes otherwise. `now` is epoch ms. Blocks for the request.
    pub fn get(&mut self, home: Option<&Path>, now: i64, active: bool) -> Option<Value> {
        if now < self.next {
            return fresh(self.last.as_ref(), now);
        }
        self.next = now + if active { 60_000 } else { 300_000 };
        let path = home?.join(".claude").join(".credentials.json");
        let Some(token) = self.token(&path, now, false) else {
            return fresh(self.last.as_ref(), now);
        };
        match (self.fetch)(&token) {
            Fetch::Ok(data) => {
                let out = parse_usage(&data, now);
                if out.is_some() {
                    self.note("ok", "receiving live limits");
                }
                self.last = out.clone();
                out
            }
            Fetch::Unauthorized => {
                self.bad_mtime = mtime(&path);
                self.note(
                    "401",
                    "sign-in was rejected; paused until Claude Code renews it",
                );
                self.last = None;
                None
            }
            Fetch::RateLimited => {
                self.next = now + 600_000;
                self.note("429", "rate limited; trying again in 10 minutes");
                fresh(self.last.as_ref(), now)
            }
            Fetch::Failed(why) => {
                self.note(&format!("fail:{why}"), &format!("request failed ({why})"));
                fresh(self.last.as_ref(), now)
            }
        }
    }

    fn note(&mut self, state: &str, msg: &str) {
        if self.state != state {
            self.state = state.to_string();
            log::info!("live Claude limits: {msg}");
        }
    }

    /// The access token, or None when there is none or it is stale (then a renew is tried).
    fn token(&mut self, path: &Path, now: i64, renewed: bool) -> Option<String> {
        let Ok(text) = std::fs::read_to_string(path) else {
            self.note(
                "nofile",
                "no Claude Code sign-in found; run `claude` once to sign in",
            );
            return None;
        };
        let Some((token, expires_ms)) = parse_credentials(&text) else {
            self.note("notoken", "no access token in the Claude Code sign-in");
            return None;
        };
        let rejected = self.bad_mtime.is_some() && self.bad_mtime == mtime(path);
        let expired = expires_ms > 0 && expires_ms < now + 30_000;
        if !rejected && !expired {
            return Some(token);
        }
        if !renewed && now >= self.next_renew {
            self.next_renew = now + RENEW_EVERY_MS;
            if (self.renew)() {
                log::info!("live Claude limits: asked Claude Code to renew its sign-in");
                return self.token(path, now, true);
            }
        }
        self.note(
            "expired",
            "sign-in expired; paused until Claude Code renews it (use `claude` once)",
        );
        None
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// (access token, expiry in epoch ms; 0 = unknown) from Claude Code's credentials file.
fn parse_credentials(text: &str) -> Option<(String, i64)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let oauth = v.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str()?;
    if token.is_empty() {
        return None;
    }
    let expires = oauth
        .get("expiresAt")
        .and_then(|e| e.as_i64().or_else(|| e.as_f64().map(|f| f as i64)))
        .unwrap_or(0);
    Some((token.to_string(), expires))
}

/// The usage response as {five_hour: {pct, reset}, seven_day: {...}, updated}. Windows whose
/// reset has passed are dropped; None when none is left.
fn parse_usage(data: &Value, now: i64) -> Option<Value> {
    let mut out = Map::new();
    for name in ["five_hour", "seven_day"] {
        let w = data.get(name);
        let pct = w.and_then(|w| w.get("utilization")).and_then(Value::as_f64);
        let reset = w
            .and_then(|w| w.get("resets_at"))
            .and_then(Value::as_str)
            .and_then(parse_iso_ms);
        if let (Some(pct), Some(reset)) = (pct, reset) {
            if reset > now {
                out.insert(name.into(), window(pct, (reset / 1000) as f64));
            }
        }
    }
    if out.is_empty() {
        return None;
    }
    out.insert("updated".into(), json!(now / 1000));
    Some(Value::Object(out))
}

/// One GET of the usage endpoint. Runs on a blocking thread, so it drives the request on
/// the tokio runtime; without a runtime nothing is fetched.
fn fetch(token: &str) -> Fetch {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return Fetch::Failed("no runtime".into());
    };
    rt.block_on(async {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
        else {
            return Fetch::Failed("http client".into());
        };
        let resp = match client
            .get(URL)
            .header("Authorization", format!("Bearer {token}"))
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("User-Agent", "deskhub")
            .send()
            .await
        {
            Ok(r) => r,
            // Only the error kind: the message could include request details.
            Err(e) => return Fetch::Failed(error_kind(&e).into()),
        };
        match resp.status().as_u16() {
            200 => {}
            401 => return Fetch::Unauthorized,
            429 => return Fetch::RateLimited,
            code => return Fetch::Failed(format!("HTTP {code}")),
        }
        // Stream, so a response without Content-Length can't grow past MAX_BODY.
        let mut resp = resp;
        let mut body = Vec::new();
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    body.extend_from_slice(&chunk);
                    if body.len() > MAX_BODY {
                        return Fetch::Failed("response too large".into());
                    }
                }
                Ok(None) => break,
                Err(e) => return Fetch::Failed(error_kind(&e).into()),
            }
        }
        match serde_json::from_slice(&body) {
            Ok(v) => Fetch::Ok(v),
            Err(_) => Fetch::Failed("unexpected response".into()),
        }
    })
}

fn error_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else {
        "network"
    }
}

/// The `claude` CLI: on PATH, or where its installer puts it.
fn find_claude() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for n in names {
                let p = dir.join(n);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    names
        .iter()
        .map(|n| PathBuf::from(&home).join(".local").join("bin").join(n))
        .find(|p| p.is_file())
}

/// Has Claude Code renew its sign-in: `claude auth status` refreshes an expired token and
/// writes it back to the credentials file; it sends no message to Claude. Output is discarded.
/// True when it ran and exited 0 within 60 s.
fn renew_signin() -> bool {
    let Some(claude) = find_claude() else {
        return false;
    };
    let mut cmd = std::process::Command::new(claude);
    cmd.args(["auth", "status"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    for _ in 0..600 {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    const NOW: i64 = 1_790_000_000_000; // 2026-09-21T14:13:20Z
    const VALID: &str = r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":1790009999000}}"#;
    const EXPIRED: &str = r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":1789999000000}}"#;

    /// A home dir with ~/.claude/.credentials.json holding `creds`.
    fn home(name: &str, creds: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("deskhub-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        std::fs::write(h.join(".claude").join(".credentials.json"), creds).unwrap();
        h
    }

    fn usage_ok(_: &str) -> Fetch {
        Fetch::Ok(json!({"five_hour": {"utilization": 50, "resets_at": "2026-09-21T16:00:00Z"}}))
    }

    fn limits(fetch: fn(&str) -> Fetch, renew: fn() -> bool) -> LiveLimits {
        LiveLimits {
            fetch,
            renew,
            ..Default::default()
        }
    }

    #[test]
    fn credentials_token_and_expiry() {
        assert_eq!(
            parse_credentials(VALID),
            Some(("abc".into(), 1_790_009_999_000))
        );
        let no_expiry = r#"{"claudeAiOauth":{"accessToken":"abc"}}"#;
        assert_eq!(parse_credentials(no_expiry), Some(("abc".into(), 0)));
        assert_eq!(
            parse_credentials(r#"{"claudeAiOauth":{"accessToken":""}}"#),
            None
        );
        assert_eq!(parse_credentials(r#"{"other":{}}"#), None);
        assert_eq!(parse_credentials("not json"), None);
    }

    #[test]
    fn usage_windows_and_expired_reset() {
        let data = json!({
            "five_hour": {"utilization": 42.37, "resets_at": "2026-09-21T16:00:00.123456+00:00"},
            "seven_day": {"utilization": 12, "resets_at": "2026-09-20T00:00:00Z"},
        });
        let v = parse_usage(&data, NOW).unwrap();
        assert_eq!(v["five_hour"]["pct"], json!(42.4));
        assert_eq!(v["five_hour"]["reset"], json!(1_790_006_400));
        assert!(v.get("seven_day").is_none(), "reset in the past is dropped");
        assert_eq!(v["updated"], json!(NOW / 1000));
    }

    #[test]
    fn usage_empty_or_odd_shapes() {
        assert_eq!(parse_usage(&json!({}), NOW), None);
        let odd = json!({"five_hour": null, "seven_day": {"utilization": "x"}});
        assert_eq!(parse_usage(&odd, NOW), None);
    }

    #[test]
    fn no_home_or_no_file_gives_none_and_waits() {
        let mut l = limits(usage_ok, || false);
        assert_eq!(l.get(None, NOW, true), None);
        assert_eq!(l.next, NOW + 60_000);
        let dir = std::env::temp_dir().join(format!("deskhub-live-none-{}", std::process::id()));
        assert_eq!(l.get(Some(&dir), NOW + 60_000, false), None);
        assert_eq!(l.next, NOW + 60_000 + 300_000);
        assert_eq!(l.state, "nofile");
    }

    #[test]
    fn ok_then_cached_between_polls_until_stale() {
        let h = home("ok", VALID);
        let mut l = limits(usage_ok, || false);
        let v = l.get(Some(&h), NOW, true).unwrap();
        assert_eq!(v["five_hour"]["pct"], json!(50.0));
        assert_eq!(l.state, "ok");
        // Between polls: the reading, while fresh.
        assert_eq!(l.get(Some(&h), NOW + 30_000, true), Some(v));
        l.next = NOW + FRESH_MS + 1;
        assert_eq!(
            l.get(Some(&h), NOW + FRESH_MS, true),
            None,
            "stale between polls"
        );
    }

    #[test]
    fn unauthorized_pauses_until_the_file_changes() {
        static RENEWS: AtomicUsize = AtomicUsize::new(0);
        let h = home("401", VALID);
        let mut l = limits(
            |_| Fetch::Unauthorized,
            || {
                RENEWS.fetch_add(1, Ordering::SeqCst);
                false
            },
        );
        assert_eq!(l.get(Some(&h), NOW, true), None);
        assert_eq!(l.state, "401");
        assert!(l.bad_mtime.is_some());
        // Same file: rejected token, a renew is tried (throttled), no request.
        l.fetch = |_| panic!("no request with a rejected token");
        assert_eq!(l.get(Some(&h), NOW + 60_000, true), None);
        assert_eq!(RENEWS.load(Ordering::SeqCst), 1);
        assert_eq!(l.state, "expired");
        assert_eq!(l.get(Some(&h), NOW + 120_000, true), None);
        assert_eq!(
            RENEWS.load(Ordering::SeqCst),
            1,
            "renew at most every 10 minutes"
        );
    }

    #[test]
    fn rate_limited_waits_10_minutes_and_keeps_fresh_reading() {
        let h = home("429", VALID);
        let mut l = limits(usage_ok, || false);
        let v = l.get(Some(&h), NOW, true);
        l.fetch = |_| Fetch::RateLimited;
        assert_eq!(l.get(Some(&h), NOW + 60_000, true), v);
        assert_eq!(l.next, NOW + 60_000 + 600_000);
        assert_eq!(l.state, "429");
    }

    #[test]
    fn failed_request_keeps_fresh_reading_and_logs_once() {
        let h = home("fail", VALID);
        let mut l = limits(usage_ok, || false);
        let v = l.get(Some(&h), NOW, true);
        l.fetch = |_| Fetch::Failed("timeout".into());
        assert_eq!(l.get(Some(&h), NOW + 60_000, true), v);
        assert_eq!(l.state, "fail:timeout");
        assert_eq!(
            l.get(Some(&h), NOW + FRESH_MS + 60_000, true),
            None,
            "too old now"
        );
    }

    #[test]
    fn expired_token_renews_then_reads_again() {
        static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
        let h = home("renew", EXPIRED);
        *PATH.lock().unwrap() = Some(h.join(".claude").join(".credentials.json"));
        // The renew writes a fresh token, as `claude auth status` does.
        let mut l = limits(usage_ok, || {
            let p = PATH.lock().unwrap().clone().unwrap();
            std::fs::write(p, VALID).unwrap();
            true
        });
        assert!(l.get(Some(&h), NOW, true).is_some());
        assert_eq!(l.state, "ok");
    }

    #[test]
    fn expired_token_without_renew_pauses() {
        let h = home("expired", EXPIRED);
        let mut l = limits(|_| panic!("no request with an expired token"), || false);
        assert_eq!(l.get(Some(&h), NOW, true), None);
        assert_eq!(l.state, "expired");
    }

    #[test]
    fn fresh_window() {
        let v = json!({"updated": NOW / 1000});
        assert!(fresh(Some(&v), NOW + FRESH_MS - 1000).is_some());
        assert!(fresh(Some(&v), NOW + FRESH_MS).is_none());
        assert!(fresh(None, NOW).is_none());
    }

    #[tokio::test]
    async fn poll_runs_on_its_own_thread_and_returns_next_tick() {
        let mut p = LivePoll {
            idle: Some(limits(usage_ok, || false)),
            ..Default::default()
        };
        let h = home("poll", r#"{"claudeAiOauth":{"accessToken":"abc"}}"#);
        let now = super::super::time::now_ms();
        // Reset times must be in the future for this clock: fake a response that is.
        p.idle.as_mut().unwrap().fetch = |_| {
            let reset = chrono::Utc::now() + chrono::Duration::hours(1);
            Fetch::Ok(json!({"seven_day": {"utilization": 7, "resets_at": reset.to_rfc3339()}}))
        };
        assert_eq!(
            p.tick(Some(h.clone()), now, true).await,
            None,
            "first tick only starts it"
        );
        while !p.task.as_ref().unwrap().is_finished() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let v = p.tick(Some(h), now, true).await.unwrap();
        assert_eq!(v["seven_day"]["pct"], json!(7.0));
    }
}
