//! Claude Code on a server (Settings > AI): polls a deskhub-relay (crate::relay) for its
//! sessions and hourly usage. The alerts and ai collectors read the result from `Ctx.remote`:
//! server sessions become alerts labelled with the server's name, server usage is counted
//! with this PC's. Limits are not fetched: the server uses the same Claude account.
//!
//! Replies are untrusted LAN data: size-capped, at most MAX_SESSIONS sessions, text folded
//! to ASCII and cut, ids sanitised. Session times are moved onto this PC's clock.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::sync::watch;

use super::ai::Usage;
use super::alerts::{ascii_fold, sanitize_id, Session};
use crate::secrets;
use crate::settings::Settings;

const POLL: Duration = Duration::from_secs(2);
/// Usage is asked every this many polls (10 s).
const USAGE_EVERY: u32 = 5;
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: usize = 1024 * 1024;
const MAX_SESSIONS: usize = 32;
const MAX_HOURS: usize = 8 * 24 * 8;
/// Failed polls before the server's sessions are dropped (no stale alert for 30 minutes).
const FAILS_BEFORE_CLEAR: u32 = 3;

/// What the app knows about the server now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteSnap {
    /// A server URL is set.
    pub configured: bool,
    /// The last poll worked.
    pub ok: bool,
    /// Why the last poll failed ("" when it worked).
    pub error: String,
    /// The server's name, for alert labels.
    pub host: String,
    /// Session id -> session (`host` set, `updated` on this PC's clock).
    pub sessions: HashMap<String, Session>,
    /// Hourly records (`ts` = hour start) plus one zero record at the last activity.
    pub usage: Vec<Usage>,
}

pub type RemoteRx = watch::Receiver<Arc<RemoteSnap>>;

pub fn channel() -> (watch::Sender<Arc<RemoteSnap>>, RemoteRx) {
    watch::channel(Arc::new(RemoteSnap::default()))
}

/// A /v1/sessions reply: the server's name and its sessions, times moved by the clock gap.
pub fn parse_sessions(v: &Value, local_now: f64) -> Result<(String, HashMap<String, Session>)> {
    let server_now = v["now"].as_f64().context("reply without the server's time")?;
    let skew = local_now - server_now;
    let host = ascii_fold(v["host"].as_str().unwrap_or("server"), 20);
    let mut out = HashMap::new();
    let Some(list) = v["sessions"].as_object() else {
        bail!("reply without sessions");
    };
    for (id, s) in list.iter().take(MAX_SESSIONS) {
        let id = sanitize_id(id);
        let state = s["state"].as_str().unwrap_or_default();
        let Some(updated) = s["updated"].as_f64() else { continue };
        if id.is_empty() || !matches!(state, "permission" | "input" | "idle" | "working") {
            continue;
        }
        out.insert(
            id,
            Session {
                state: state.to_string(),
                message: ascii_fold(s["message"].as_str().unwrap_or_default(), 200),
                project: ascii_fold(s["project"].as_str().unwrap_or_default(), 40),
                updated: updated + skew,
                pids: Vec::new(),
                host: host.clone(),
                request: sanitize_id(s["request"].as_str().unwrap_or_default()),
                detail: ascii_fold(s["detail"].as_str().unwrap_or_default(), 120),
                cause: ascii_fold(s["cause"].as_str().unwrap_or_default(), 60),
            },
        );
    }
    Ok((host, out))
}

/// A /v1/usage reply: its revision, and the records unless it says `unchanged`.
pub fn parse_usage(v: &Value) -> Result<(i64, Option<Vec<Usage>>)> {
    let rev = v["rev"].as_i64().context("reply without a revision")?;
    if v["unchanged"].as_bool() == Some(true) {
        return Ok((rev, None));
    }
    let Some(hours) = v["hours"].as_array() else {
        bail!("reply without hours");
    };
    let mut out = Vec::new();
    for h in hours.iter().take(MAX_HOURS) {
        let f = |i: usize| h[i].as_i64().filter(|n| *n >= 0);
        let (Some(hour), Some(total), Some(output), Some(n)) = (f(0), f(2), f(3), f(4)) else {
            continue;
        };
        out.push(Usage {
            ts: hour * 1000,
            model: ascii_fold(h[1].as_str().unwrap_or("Other"), 16),
            total,
            output,
            n,
        });
    }
    // The last activity, to the second (hours start on the hour): counts nothing.
    if let Some(last) = v["last_active"].as_i64().filter(|s| *s > 0) {
        out.push(Usage { ts: last * 1000, model: String::new(), total: 0, output: 0, n: 0 });
    }
    out.sort_by_key(|u| u.ts);
    Ok((rev, Some(out)))
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

async fn get_json(client: &reqwest::Client, url: &str, token: &str) -> Result<Value> {
    let resp = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(crate::plex::send_err)?;
    if resp.status().as_u16() == 401 {
        bail!("the server refused the token");
    }
    let body = crate::plex::read_limited(resp, MAX_BODY).await?;
    serde_json::from_slice(&body).context("the reply isn't JSON")
}

/// Sends the board's Allow / Deny for permission request `request` of server session `id`.
pub async fn decide(url: &str, id: &str, request: &str, behavior: &str) -> Result<()> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() {
        bail!("no server set");
    }
    // Ids are [A-Za-z0-9_-] (sanitize_id), behavior allow|deny: nothing to escape.
    if !matches!(behavior, "allow" | "deny") {
        bail!("unknown answer {behavior:?}");
    }
    let client = reqwest::Client::builder().timeout(TIMEOUT).user_agent("deskhub").build()?;
    let resp = client
        .post(format!("{url}/v1/decide?id={id}&request={request}&behavior={behavior}"))
        .bearer_auth(token()?)
        .send()
        .await
        .map_err(crate::plex::send_err)?;
    match resp.status().as_u16() {
        200 => Ok(()),
        401 => bail!("the server refused the token"),
        409 => bail!("the request was already answered"),
        s => bail!("the server answered {s}"),
    }
}

fn token() -> Result<String> {
    secrets::system_store_for(secrets::REMOTE_TOKEN)
        .get()
        .map_err(|e| anyhow::anyhow!("credential store: {e:#}"))?
        .filter(|t| !t.is_empty())
        .context("no token saved for the server")
}

pub async fn run(tx: watch::Sender<Arc<RemoteSnap>>, mut settings: watch::Receiver<Settings>) {
    let client = match reqwest::Client::builder().timeout(TIMEOUT).user_agent("deskhub").build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("remote claude: {e}");
            return;
        }
    };
    let mut snap = RemoteSnap::default();
    let mut url = String::new();
    let (mut rev, mut tick, mut fails) = (-1_i64, 0_u32, 0_u32);
    loop {
        let want = settings.borrow().ai.remote_url.trim().trim_end_matches('/').to_string();
        if want != url {
            url = want;
            snap = RemoteSnap { configured: !url.is_empty(), ..Default::default() };
            (rev, tick, fails) = (-1, 0, 0);
            tx.send_replace(Arc::new(snap.clone()));
        }
        if url.is_empty() {
            if settings.changed().await.is_err() {
                return;
            }
            continue;
        }
        let result = async {
            let token = token()?;
            let v = get_json(&client, &format!("{url}/v1/sessions"), &token).await?;
            let (host, sessions) = parse_sessions(&v, now_secs())?;
            snap.host = host;
            snap.sessions = sessions;
            if tick % USAGE_EVERY == 0 {
                let v = get_json(&client, &format!("{url}/v1/usage?rev={rev}"), &token).await?;
                let (r, usage) = parse_usage(&v)?;
                rev = r;
                if let Some(u) = usage {
                    snap.usage = u;
                }
            }
            anyhow::Ok(())
        }
        .await;
        tick = tick.wrapping_add(1);
        match result {
            Ok(()) => {
                fails = 0;
                snap.ok = true;
                snap.error.clear();
            }
            Err(e) => {
                fails += 1;
                snap.ok = false;
                snap.error = format!("{e:#}");
                if fails >= FAILS_BEFORE_CLEAR {
                    snap.sessions.clear();
                }
                // Ask for everything again once it answers.
                rev = -1;
                tick = 0;
            }
        }
        tx.send_if_modified(|cur| {
            if **cur == snap {
                return false;
            }
            *cur = Arc::new(snap.clone());
            true
        });
        tokio::select! {
            _ = tokio::time::sleep(POLL) => {}
            r = settings.changed() => if r.is_err() { return },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sessions_are_cleaned_and_moved_to_this_clock() {
        let v = json!({"now": 1000.0, "host": "vibecode", "sessions": {
            "abc-1": {"state": "permission", "message": "Claude needs your permission to use Bash",
                      "project": "deskhub", "updated": 990.0, "request": "r1", "detail": "Bash: ls"},
            "bad/../id": {"state": "input", "updated": 995.0},
            "x": {"state": "nonsense", "updated": 995.0},
            "y": {"state": "idle"}
        }});
        let (host, s) = parse_sessions(&v, 1100.0).unwrap();
        assert_eq!(host, "vibecode");
        assert_eq!(s.len(), 2);
        let a = &s["abc-1"];
        assert_eq!((a.request.as_str(), a.detail.as_str()), ("r1", "Bash: ls"));
        assert_eq!(a.updated, 1090.0); // 10 s before the server's now, on this clock
        assert_eq!((a.host.as_str(), a.project.as_str()), ("vibecode", "deskhub"));
        assert!(a.pids.is_empty());
        assert!(s.contains_key("badid"));
        assert!(parse_sessions(&json!({"sessions": {}}), 0.0).is_err());
    }

    #[test]
    fn too_many_sessions_are_cut() {
        let many: serde_json::Map<String, Value> = (0..100)
            .map(|i| (format!("s{i}"), json!({"state": "working", "updated": 1.0})))
            .collect();
        let (_, s) = parse_sessions(&json!({"now": 1.0, "sessions": many}), 1.0).unwrap();
        assert_eq!(s.len(), MAX_SESSIONS);
    }

    #[test]
    fn usage_hours_and_last_activity() {
        let v = json!({"rev": 4, "last_active": 7300, "hours": [
            [7200, "Opus", 500, 50, 3], [3600, "Sonnet", 10, 1, 1], [3600, "Opus", -5, 0, 1]
        ]});
        let (rev, u) = parse_usage(&v).unwrap();
        let u = u.unwrap();
        assert_eq!(rev, 4);
        assert_eq!(u.len(), 3);
        assert_eq!((u[0].ts, u[0].model.as_str(), u[0].n), (3_600_000, "Sonnet", 1));
        assert_eq!((u[1].ts, u[1].total, u[1].output, u[1].n), (7_200_000, 500, 50, 3));
        assert_eq!((u[2].ts, u[2].total, u[2].n), (7_300_000, 0, 0));
        assert_eq!(parse_usage(&json!({"rev": 4, "unchanged": true})).unwrap(), (4, None));
        assert!(parse_usage(&json!({"hours": []})).is_err());
    }
}
