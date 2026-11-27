//! deskhub-relay: serves a server's Claude Code sessions and hourly usage to the desktop
//! app (collectors/remote.rs). Runs on the server as a service; deskhub-cli's hooks write
//! the session files there as they do on the PC.
//!
//! GET /v1/sessions    -> {now, host, sessions: {id: {state, message, project, updated}}}
//! GET /v1/usage?rev=N -> {rev, unchanged: true} when N is current, else
//!                        {rev, hours: [[hour_s, model, total, output, n]], last_active}
//! POST /v1/decide?id=&request=&behavior=allow|deny -> the board's answer to a pending
//!                        permission request: written to ~/.deskhub/claude_decisions/<id>.json
//!                        for the waiting deskhub-cli hook. 409 unless that request is pending.
//!
//! Plain HTTP for the LAN with a Bearer token (compared in constant time). Minimal
//! HTTP/1.1: one GET per connection, headers capped at MAX_HEAD, IO_TIMEOUT per read and
//! write, at most MAX_CONNS connections at once. Never reads claude_status.json: the limits
//! come from the PC (same account). Paths, process ids and working folders are not sent.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use crate::collectors::ai::{Claude, Usage};
use crate::collectors::alerts::{load_sessions, parse_session, sanitize_id};

const MAX_HEAD: usize = 8 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNS: usize = 8;
/// Transcripts are read again this often.
const SCAN_EVERY: Duration = Duration::from_secs(10);
/// Shorter tokens are refused at start.
pub const MIN_TOKEN: usize = 16;

pub struct Config {
    /// The Bearer token the app sends (Settings > AI).
    pub token: String,
    /// The server's name in alert labels.
    pub host: String,
    /// The home folder of the user running Claude Code (.claude and .deskhub in it).
    pub home: PathBuf,
}

/// One hour of one model: (hour start in epoch seconds, model, total, output, messages).
pub type Hour = (i64, String, i64, i64, i64);

#[derive(Default)]
struct UsageState {
    rev: i64,
    hours: Vec<Hour>,
    last_active: i64,
}

struct Shared {
    cfg: Config,
    usage: Mutex<UsageState>,
}

/// Sums transcript records per hour and model. Also returns the last record's time (s).
pub fn hours(usage: &[Usage]) -> (Vec<Hour>, i64) {
    let mut sums: BTreeMap<(i64, String), (i64, i64, i64)> = BTreeMap::new();
    let mut last = 0;
    for u in usage {
        let e = sums
            .entry((u.ts.div_euclid(3_600_000) * 3600, u.model.clone()))
            .or_default();
        e.0 += u.total;
        e.1 += u.output;
        e.2 += u.n;
        last = last.max(u.ts.div_euclid(1000));
    }
    let list = sums
        .into_iter()
        .map(|((h, m), (t, o, n))| (h, m, t, o, n))
        .collect();
    (list, last)
}

/// Equal-length byte compare whose time doesn't depend on where they differ.
fn same_token(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Reads the transcripts once more (off the runtime) and bumps the revision on change.
async fn refresh(shared: &Arc<Shared>, claude: Claude) -> Claude {
    let home = shared.cfg.home.clone();
    let done = tokio::task::spawn_blocking(move || {
        let mut claude = claude;
        let (h, last) = hours(claude.scan(Some(&home), now_ms()));
        (claude, h, last)
    })
    .await;
    let Ok((claude, h, last)) = done else {
        return Claude::default();
    };
    let mut st = shared.usage.lock().unwrap_or_else(|e| e.into_inner());
    if st.hours != h || st.last_active != last {
        st.rev += 1;
        st.hours = h;
        st.last_active = last;
    }
    claude
}

/// Reads the request head (up to the blank line), at most MAX_HEAD bytes.
async fn read_head(s: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            bail!("closed early");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            buf.truncate(i);
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
        if buf.len() > MAX_HEAD {
            bail!("request head too long");
        }
    }
}

fn sessions_body(shared: &Shared) -> Value {
    let now = now_ms() as f64 / 1000.0;
    let root = shared.cfg.home.join(".deskhub").join("claude_sessions");
    let mut list = Map::new();
    for (id, s) in load_sessions(&root, now) {
        list.insert(
            id,
            json!({
                "state": s.state,
                "message": s.message,
                "project": s.project,
                "updated": s.updated,
                "request": s.request,
                "detail": s.detail,
                "cause": s.cause,
            }),
        );
    }
    json!({"now": now, "host": shared.cfg.host, "sessions": list})
}

fn usage_body(shared: &Shared, query: &str) -> Value {
    let asked = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("rev="))
        .and_then(|r| r.parse::<i64>().ok());
    let st = shared.usage.lock().unwrap_or_else(|e| e.into_inner());
    if asked == Some(st.rev) {
        return json!({"rev": st.rev, "unchanged": true});
    }
    let hours: Vec<Value> = st
        .hours
        .iter()
        .map(|(h, m, t, o, n)| json!([h, m, t, o, n]))
        .collect();
    json!({"rev": st.rev, "hours": hours, "last_active": st.last_active})
}

fn param<'a>(query: &'a str, key: &str) -> &'a str {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_default()
}

/// The board's Allow / Deny: written for the hook waiting on that permission request.
fn decide(shared: &Shared, query: &str) -> (&'static str, Value) {
    let (id, request, behavior) = (param(query, "id"), param(query, "request"), param(query, "behavior"));
    let ok_id = |s: &str| !s.is_empty() && s.len() <= 64 && sanitize_id(s) == s;
    if !ok_id(id) || !ok_id(request) || !matches!(behavior, "allow" | "deny") {
        return ("400 Bad Request", json!({"error": "id, request and behavior=allow|deny"}));
    }
    let dir = shared.cfg.home.join(".deskhub");
    let pending = std::fs::read_to_string(dir.join("claude_sessions").join(format!("{id}.json")))
        .ok()
        .and_then(|t| parse_session(&t))
        .is_some_and(|s| s.state == "permission" && s.request == request);
    if !pending {
        return ("409 Conflict", json!({"error": "not waiting for this answer"}));
    }
    let body = json!({"request": request, "behavior": behavior}).to_string();
    match write_private(&dir.join("claude_decisions"), &format!("{id}.json"), body.as_bytes()) {
        Ok(()) => ("200 OK", json!({"ok": true})),
        Err(e) => {
            log::warn!("relay decide: {e}");
            ("500 Internal Server Error", json!({"error": "could not save the answer"}))
        }
    }
}

/// Writes `name` in `dir` via a temp file and rename; owner-only on Unix.
fn write_private(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let result = opts.open(&tmp).and_then(|mut f| f.write_all(bytes));
    match result.and_then(|()| std::fs::rename(&tmp, dir.join(name))) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Status line and JSON body for a request head.
async fn respond(head: &str, shared: &Arc<Shared>) -> (&'static str, Value) {
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let (method, target) = (first.next().unwrap_or_default(), first.next().unwrap_or_default());
    let token = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("authorization"))
        .and_then(|(_, v)| v.trim().strip_prefix("Bearer "))
        .unwrap_or_default()
        .trim();
    if !same_token(token.as_bytes(), shared.cfg.token.as_bytes()) {
        return ("401 Unauthorized", json!({"error": "unauthorized"}));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let want = if path == "/v1/decide" { "POST" } else { "GET" };
    if method != want {
        return ("405 Method Not Allowed", json!({"error": format!("{want} only")}));
    }
    match path {
        "/v1/decide" => {
            let s = shared.clone();
            let query = query.to_string();
            match tokio::task::spawn_blocking(move || decide(&s, &query)).await {
                Ok(r) => r,
                Err(_) => ("500 Internal Server Error", json!({"error": "decide"})),
            }
        }
        "/v1/sessions" => {
            let s = shared.clone();
            match tokio::task::spawn_blocking(move || sessions_body(&s)).await {
                Ok(v) => ("200 OK", v),
                Err(_) => ("500 Internal Server Error", json!({"error": "sessions"})),
            }
        }
        "/v1/usage" => ("200 OK", usage_body(shared, query)),
        _ => ("404 Not Found", json!({"error": "not found"})),
    }
}

async fn handle(mut s: TcpStream, shared: Arc<Shared>) -> Result<()> {
    let head = tokio::time::timeout(IO_TIMEOUT, read_head(&mut s))
        .await
        .context("timed out")??;
    let (status, body) = respond(&head, &shared).await;
    let body = serde_json::to_vec(&body)?;
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tokio::time::timeout(IO_TIMEOUT, async {
        s.write_all(head.as_bytes()).await?;
        s.write_all(&body).await?;
        s.shutdown().await
    })
    .await
    .context("timed out")??;
    Ok(())
}

async fn serve(listener: TcpListener, shared: Arc<Shared>) -> Result<()> {
    let slots = Arc::new(Semaphore::new(MAX_CONNS));
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                log::warn!("relay accept: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        // Busy: drop the connection; the app asks again in two seconds.
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let _ = handle(stream, shared).await;
            drop(permit);
        });
    }
}

/// Reads the transcripts, then serves on `listener` and rescans every SCAN_EVERY.
pub async fn run_on(listener: TcpListener, cfg: Config) -> Result<()> {
    if cfg.token.len() < MIN_TOKEN {
        bail!("the token needs at least {MIN_TOKEN} characters");
    }
    let shared = Arc::new(Shared {
        cfg,
        usage: Mutex::new(UsageState::default()),
    });
    let mut claude = refresh(&shared, Claude::default()).await;
    let bg = shared.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SCAN_EVERY).await;
            claude = refresh(&bg, claude).await;
        }
    });
    serve(listener, shared).await
}

/// `run_on` a new listener at `bind` ("192.168.68.107:8791").
pub async fn run(bind: &str, cfg: Config) -> Result<()> {
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("listening on {bind}"))?;
    eprintln!("deskhub-relay: serving {} on {bind}", cfg.host);
    run_on(listener, cfg).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::remote::{parse_sessions, parse_usage};
    use std::fs;

    const TOKEN: &str = "0123456789abcdef-test";

    fn usage(ts: i64, model: &str, total: i64) -> Usage {
        Usage { ts, model: model.into(), total, output: 1, n: 1 }
    }

    #[test]
    fn records_are_summed_per_hour_and_model() {
        let h = 1_795_435_200_000; // on the hour
        let (list, last) = hours(&[
            usage(h + 10_000, "Opus", 5),
            usage(h + 20_000, "Opus", 7),
            usage(h + 30_000, "Sonnet", 1),
            usage(h + 3_600_000, "Opus", 2),
        ]);
        let s = h / 1000;
        assert_eq!(
            list,
            vec![
                (s, "Opus".into(), 12, 2, 2),
                (s, "Sonnet".into(), 1, 1, 1),
                (s + 3600, "Opus".into(), 2, 1, 1),
            ]
        );
        assert_eq!(last, s + 3600);
        assert_eq!(hours(&[]), (vec![], 0));
    }

    #[test]
    fn tokens_compare() {
        assert!(same_token(b"abc", b"abc"));
        assert!(!same_token(b"abc", b"abd"));
        assert!(!same_token(b"abc", b"abcd"));
        assert!(!same_token(b"", b"abc"));
    }

    async fn get(addr: std::net::SocketAddr, path: &str, token: &str) -> (String, Value) {
        call(addr, "GET", path, token).await
    }

    async fn call(addr: std::net::SocketAddr, method: &str, path: &str, token: &str) -> (String, Value) {
        let mut s = TcpStream::connect(addr).await.unwrap();
        let req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nauthorization: Bearer {token}\r\nContent-Length: 0\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        let status = head.lines().next().unwrap().to_string();
        (status, serde_json::from_str(body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn serves_sessions_and_usage_to_the_app() {
        let home = std::env::temp_dir().join(format!("deskhub-relay-{}", std::process::id()));
        let dir = home.join(".deskhub").join("claude_sessions");
        fs::create_dir_all(&dir).unwrap();
        let now = now_ms() as f64 / 1000.0;
        fs::write(
            dir.join("abc-1.json"),
            json!({"state": "permission", "message": "Claude needs your permission to use Bash",
                   "project": "deskhub", "cwd": "/root/secret", "updated": now - 5.0,
                   "pids": [1, 2], "request": "r1", "detail": "Bash: ls"})
            .to_string(),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cfg = Config { token: TOKEN.into(), host: "vibecode".into(), home: home.clone() };
        tokio::spawn(run_on(listener, cfg));

        let (status, _) = get(addr, "/v1/sessions", "wrong-token-wrong-token").await;
        assert!(status.contains("401"), "{status}");
        let (status, _) = get(addr, "/v1/nope", TOKEN).await;
        assert!(status.contains("404"), "{status}");

        let (status, v) = get(addr, "/v1/sessions", TOKEN).await;
        assert!(status.contains("200"), "{status}");
        assert!(!v.to_string().contains("/root/secret"));
        let (host, sessions) = parse_sessions(&v, now).unwrap();
        assert_eq!(host, "vibecode");
        assert_eq!(sessions["abc-1"].project, "deskhub");
        assert_eq!(sessions["abc-1"].detail, "Bash: ls");

        // The board's answer: only for the pending request, POST only.
        let decided = home.join(".deskhub/claude_decisions/abc-1.json");
        let (status, _) = get(addr, "/v1/decide?id=abc-1&request=r1&behavior=allow", TOKEN).await;
        assert!(status.contains("405"), "{status}");
        let (status, _) = call(addr, "POST", "/v1/decide?id=abc-1&request=r1&behavior=allow", "wrong-token-wrong-token").await;
        assert!(status.contains("401"), "{status}");
        let (status, _) = call(addr, "POST", "/v1/decide?id=abc-1&request=old&behavior=allow", TOKEN).await;
        assert!(status.contains("409"), "{status}");
        let (status, _) = call(addr, "POST", "/v1/decide?id=abc-1&request=r1&behavior=always", TOKEN).await;
        assert!(status.contains("400"), "{status}");
        let (status, _) = call(addr, "POST", "/v1/decide?id=../x&request=r1&behavior=allow", TOKEN).await;
        assert!(status.contains("400"), "{status}");
        assert!(!decided.exists());
        let (status, _) = call(addr, "POST", "/v1/decide?id=abc-1&request=r1&behavior=allow", TOKEN).await;
        assert!(status.contains("200"), "{status}");
        let d: Value = serde_json::from_str(&fs::read_to_string(&decided).unwrap()).unwrap();
        assert_eq!(d, json!({"request": "r1", "behavior": "allow"}));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&decided).unwrap().permissions().mode() & 0o777, 0o600);
        }

        let (_, v) = get(addr, "/v1/usage?rev=-1", TOKEN).await;
        let (rev, hours) = parse_usage(&v).unwrap();
        assert_eq!(hours, Some(vec![]));
        let (_, v) = get(addr, &format!("/v1/usage?rev={rev}"), TOKEN).await;
        assert_eq!(parse_usage(&v).unwrap(), (rev, None));
        let _ = fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn short_tokens_are_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = Config { token: "short".into(), host: "x".into(), home: PathBuf::from("/nonexistent") };
        assert!(run_on(listener, cfg).await.is_err());
    }
}
