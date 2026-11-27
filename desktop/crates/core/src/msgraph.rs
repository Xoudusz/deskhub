//! "Sign in with Microsoft" for an Outlook (Exchange Online) calendar that can't be
//! published as an .ics link: OAuth 2.0 authorization code + PKCE with a loopback redirect
//! (a public client, no secret), then Microsoft Graph's calendarView.
//!
//! The user registers the app in their own Entra tenant (README, Calendars): platform
//! "Mobile and desktop", redirect `http://localhost`, public client flows on, delegated
//! Calendars.Read. A single-tenant app needs the tenant in the URL (`common` fails).
//! Codes, state, tokens and token replies are never logged or put into errors.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use chrono::{NaiveDate, NaiveDateTime, TimeZone};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const AUTH_BASE: &str = "https://login.microsoftonline.com";
pub const GRAPH: &str = "https://graph.microsoft.com/v1.0";
pub const SCOPE: &str = "offline_access Calendars.Read User.Read";
/// How long the browser has to come back.
pub const WAIT: Duration = Duration::from_secs(5 * 60);
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_PAGES: usize = 5;
/// The request line and headers of the browser's redirect.
const MAX_HEAD: usize = 8 * 1024;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

// ---- PKCE and the sign-in URL ---------------------------------------------------

/// One sign-in's secrets: the PKCE verifier (kept here), its S256 challenge (sent in the
/// URL) and the `state` the redirect must echo.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

fn random_b64() -> Result<String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| anyhow!("no random numbers: {e}"))?;
    Ok(B64.encode(b))
}

pub fn new_pkce() -> Result<Pkce> {
    let verifier = random_b64()?;
    Ok(Pkce { challenge: challenge_of(&verifier), verifier, state: random_b64()? })
}

/// RFC 7636 S256: base64url(sha256(verifier)) without padding.
pub fn challenge_of(verifier: &str) -> String {
    B64.encode(Sha256::digest(verifier.as_bytes()))
}

/// The tenant from what the user typed: an email address gives its domain; a domain or a
/// tenant id stays as it is.
pub fn tenant_of(input: &str) -> String {
    let t = input.trim();
    t.rsplit_once('@').map(|(_, d)| d).unwrap_or(t).trim().to_ascii_lowercase()
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
pub fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes a query value (`+` is a space).
pub fn urldec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn auth_url(base: &str, tenant: &str, client_id: &str, redirect: &str, p: &Pkce) -> String {
    format!(
        "{base}/{}/oauth2/v2.0/authorize?client_id={}&response_type=code&redirect_uri={}&response_mode=query\
         &scope={}&state={}&code_challenge={}&code_challenge_method=S256&prompt=select_account",
        urlenc(tenant),
        urlenc(client_id),
        urlenc(redirect),
        urlenc(SCOPE),
        urlenc(&p.state),
        urlenc(&p.challenge),
    )
}

// ---- the loopback redirect --------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum Redirect {
    Code(String),
    /// The sign-in page sent an error back (e.g. consent refused).
    Error { code: String, desc: String },
}

/// Reads the browser's request line (`GET /?code=..&state=.. HTTP/1.1`). Err for anything
/// that isn't the redirect (a favicon, a wrong state), which the caller ignores or reports.
pub fn parse_redirect(head: &str, state: &str) -> Result<Redirect, String> {
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    if parts.next() != Some("GET") {
        return Err("not a GET".into());
    }
    let target = parts.next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/" {
        return Err("not the redirect".into());
    }
    let get = |k: &str| {
        query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(key, _)| *key == k)
            .map(|(_, v)| urldec(v))
    };
    if let Some(code) = get("error") {
        return Ok(Redirect::Error { code, desc: get("error_description").unwrap_or_default() });
    }
    if get("state").as_deref() != Some(state) {
        return Err("the sign-in reply doesn't match this sign-in".into());
    }
    match get("code") {
        Some(c) if !c.is_empty() => Ok(Redirect::Code(c)),
        _ => Err("the sign-in reply has no code".into()),
    }
}

/// A listener for the redirect on 127.0.0.1 and an unused port. The redirect URI is
/// `http://localhost:{port}` (what the app registration allows, any port).
pub async fn bind_loopback() -> Result<(TcpListener, u16)> {
    let l = TcpListener::bind("127.0.0.1:0").await.context("no local port for the sign-in")?;
    let port = l.local_addr()?.port();
    Ok((l, port))
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>Desk Hub</title>\
<body style=\"font-family:sans-serif;padding:40px\"><h2>Desk Hub</h2><p>You can close this tab and go back to Desk Hub.</p>";

/// Waits for the browser to come back with a code or an error. Requests that aren't the
/// redirect (a favicon) are answered and skipped. Err on timeout or cancel (`cancelled()`
/// is checked twice a second).
pub async fn wait_redirect(
    listener: TcpListener,
    state: &str,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> Result<Redirect> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cancelled() {
            bail!("cancelled");
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("timed out");
        }
        let accepted = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await;
        let Ok(Ok((mut sock, _))) = accepted else { continue };
        let mut head = Vec::new();
        let mut buf = [0u8; 1024];
        let read = tokio::time::timeout(Duration::from_secs(5), async {
            while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < MAX_HEAD {
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
            }
        })
        .await;
        if read.is_err() {
            continue;
        }
        let parsed = parse_redirect(&String::from_utf8_lossy(&head), state);
        let (status, body) = match &parsed {
            Ok(_) => ("200 OK", PAGE_OK),
            Err(_) => ("404 Not Found", ""),
        };
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        sock.write_all(reply.as_bytes()).await.ok();
        sock.shutdown().await.ok();
        match parsed {
            Ok(r) => return Ok(r),
            Err(e) if e.contains("doesn't match") => bail!("{e}"),
            Err(_) => continue,
        }
    }
}

// ---- tokens --------------------------------------------------------------------

/// The HTTP client for sign-in and Graph (30 s per request).
pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("deskhub")
        .build()
        .context("no HTTP client")
}

#[derive(Debug, PartialEq)]
pub struct Tokens {
    pub access: String,
    /// A new refresh token (they rotate); None keeps the old one.
    pub refresh: Option<String>,
    pub expires_in: u64,
}

#[derive(Debug, PartialEq)]
pub enum TokenError {
    /// The refresh token is no longer valid: sign in again.
    InvalidGrant(String),
    /// The tenant needs an administrator to approve the app.
    Consent(String),
    Other(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::InvalidGrant(m) | TokenError::Consent(m) | TokenError::Other(m) => f.write_str(m),
        }
    }
}

pub const CONSENT_MESSAGE: &str = "Your organisation needs an administrator to approve this app. \
Ask your IT admin to grant consent for Calendars.Read and User.Read, then sign in again.";

/// Sorts an OAuth error: consent needed, sign-in no longer valid, or anything else.
pub fn classify_error(error: &str, desc: &str) -> TokenError {
    let d = desc.to_ascii_lowercase();
    // AADSTS65001: no consent; 90094/90095: admin consent required; 65004: user declined.
    if ["AADSTS65001", "AADSTS90094", "AADSTS90095"].iter().any(|c| desc.contains(c))
        || d.contains("admin approval")
        || d.contains("admin consent")
    {
        return TokenError::Consent(CONSENT_MESSAGE.into());
    }
    let first = desc.lines().next().unwrap_or("").trim();
    let short = if first.is_empty() { error.to_string() } else { first.chars().take(200).collect() };
    if error == "invalid_grant" || error == "interaction_required" {
        return TokenError::InvalidGrant(format!("Sign in again ({short})"));
    }
    if error == "access_denied" {
        return TokenError::Other(format!("The sign-in was refused ({short})"));
    }
    TokenError::Other(short)
}

pub fn parse_token_response(status: u16, body: &[u8]) -> Result<Tokens, TokenError> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|_| TokenError::Other(format!("Microsoft answered HTTP {status} without JSON")))?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    if !(200..300).contains(&status) || v.get("error").is_some() {
        let err = s("error");
        return Err(classify_error(if err.is_empty() { "http_error" } else { &err }, &s("error_description")));
    }
    let access = s("access_token");
    if access.is_empty() {
        return Err(TokenError::Other("Microsoft sent no access token".into()));
    }
    let refresh = Some(s("refresh_token")).filter(|r| !r.is_empty());
    let expires_in = v.get("expires_in").and_then(|e| e.as_u64().or_else(|| e.as_str()?.parse().ok())).unwrap_or(3600);
    Ok(Tokens { access, refresh, expires_in })
}

/// Any reply's status and body (capped), so an error reply's JSON can be read.
pub(crate) async fn read_any(mut resp: reqwest::Response) -> Result<(u16, Vec<u8>)> {
    let status = resp.status().as_u16();
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| anyhow!("reading the reply: {}", e.without_url()))? {
        if body.len() + chunk.len() > MAX_BODY {
            bail!("reply too large");
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

async fn token_request(client: &reqwest::Client, base: &str, tenant: &str, form: String) -> Result<Tokens, TokenError> {
    let url = format!("{base}/{}/oauth2/v2.0/token", urlenc(tenant));
    let resp = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .map_err(|e| TokenError::Other(format!("{:#}", crate::plex::send_err(e))))?;
    let (status, body) = read_any(resp).await.map_err(|e| TokenError::Other(format!("{e:#}")))?;
    parse_token_response(status, &body)
}

pub async fn exchange_code(
    client: &reqwest::Client,
    base: &str,
    tenant: &str,
    client_id: &str,
    code: &str,
    redirect: &str,
    verifier: &str,
) -> Result<Tokens, TokenError> {
    let form = format!(
        "client_id={}&grant_type=authorization_code&code={}&redirect_uri={}&code_verifier={}&scope={}",
        urlenc(client_id),
        urlenc(code),
        urlenc(redirect),
        urlenc(verifier),
        urlenc(SCOPE)
    );
    token_request(client, base, tenant, form).await
}

pub async fn refresh(
    client: &reqwest::Client,
    base: &str,
    tenant: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<Tokens, TokenError> {
    let form = format!(
        "client_id={}&grant_type=refresh_token&refresh_token={}&scope={}",
        urlenc(client_id),
        urlenc(refresh_token),
        urlenc(SCOPE)
    );
    token_request(client, base, tenant, form).await
}

// ---- events ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct GraphEvent {
    pub title: String,
    pub loc: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
}

#[derive(Debug)]
pub enum GraphError {
    /// 401: the access token is no longer accepted.
    Unauthorized,
    Other(anyhow::Error),
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphError::Unauthorized => f.write_str("Microsoft refused the sign-in (HTTP 401)"),
            GraphError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

fn local_midnight(d: NaiveDate) -> Option<i64> {
    let n = d.and_hms_opt(0, 0, 0)?;
    chrono::Local
        .from_local_datetime(&n)
        .earliest()
        .or_else(|| chrono::Local.from_local_datetime(&(n + chrono::Duration::hours(1))).earliest())
        .map(|t| t.timestamp())
}

/// A Graph time in UTC (`2026-11-27T09:00:00.0000000`, the reply's time zone is UTC) as
/// unix seconds. All-day events are midnight in the calendar's zone, so in UTC they are up
/// to 12 hours off a midnight: rounded to the nearest one, that day starts at local midnight
/// (as .ics all-day events do).
pub fn graph_time(dt: &str, all_day: bool) -> Option<i64> {
    let s = dt.trim().trim_end_matches('Z');
    let s = s.split('.').next()?;
    let n = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()?;
    let utc = n.and_utc().timestamp();
    if !all_day {
        return Some(utc);
    }
    let day = (utc + 43_200).div_euclid(86_400);
    let date = chrono::DateTime::from_timestamp(day * 86_400, 0)?.date_naive();
    local_midnight(date)
}

/// One calendarView page: its events (cancelled ones dropped) and the next page's link.
pub fn parse_events_page(body: &[u8]) -> Result<(Vec<GraphEvent>, Option<String>)> {
    let v: Value = serde_json::from_slice(body).context("Microsoft's reply isn't JSON")?;
    let list = v.get("value").and_then(Value::as_array).context("Microsoft's reply has no events list")?;
    let mut out = Vec::new();
    for e in list {
        if e.get("isCancelled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let all_day = e.get("isAllDay").and_then(Value::as_bool).unwrap_or(false);
        let time = |k: &str| e.get(k)?.get("dateTime")?.as_str().and_then(|t| graph_time(t, all_day));
        let (Some(start), Some(end)) = (time("start"), time("end")) else { continue };
        let title = e.get("subject").and_then(Value::as_str).unwrap_or("").trim();
        let loc = e.get("location").and_then(|l| l.get("displayName")).and_then(Value::as_str).unwrap_or("");
        out.push(GraphEvent {
            title: if title.is_empty() { "(No title)".into() } else { title.to_string() },
            loc: loc.trim().to_string(),
            start,
            end: end.max(start),
            all_day,
        });
    }
    let next = v.get("@odata.nextLink").and_then(Value::as_str).map(str::to_string);
    Ok((out, next))
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split('/').next().unwrap_or("")
}

fn iso(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// The signed-in user's events between `from` and `to` (unix seconds), recurring ones
/// expanded by Graph. Follows next-page links on the same host only (the token never goes
/// elsewhere), at most MAX_PAGES pages.
pub async fn fetch_events(
    client: &reqwest::Client,
    graph: &str,
    token: &str,
    from: i64,
    to: i64,
) -> Result<Vec<GraphEvent>, GraphError> {
    let mut url = format!(
        "{graph}/me/calendarView?startDateTime={}&endDateTime={}\
         &$select=subject,start,end,isAllDay,location,isCancelled&$orderby=start/dateTime&$top=100",
        urlenc(&iso(from)),
        urlenc(&iso(to))
    );
    let host = host_of(graph).to_string();
    let mut all = Vec::new();
    for _ in 0..MAX_PAGES {
        let resp = client
            .get(&url)
            .bearer_auth(token)
            .header("Prefer", "outlook.timezone=\"UTC\"")
            .send()
            .await
            .map_err(|e| GraphError::Other(crate::plex::send_err(e)))?;
        if resp.status().as_u16() == 401 {
            return Err(GraphError::Unauthorized);
        }
        let body = crate::plex::read_limited(resp, MAX_BODY).await.map_err(GraphError::Other)?;
        let (events, next) = parse_events_page(&body).map_err(GraphError::Other)?;
        all.extend(events);
        match next {
            Some(n) if host_of(&n) == host => url = n,
            Some(_) => return Err(GraphError::Other(anyhow!("Microsoft sent a next page on another host"))),
            None => break,
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plex::tests::fake;

    #[test]
    fn pkce_rfc7636_vector() {
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn new_pkce_is_random_and_url_safe() {
        let a = new_pkce().unwrap();
        let b = new_pkce().unwrap();
        assert_eq!(a.verifier.len(), 43);
        assert!(a.verifier.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.state, b.state);
        assert_eq!(a.challenge, challenge_of(&a.verifier));
    }

    #[test]
    fn tenant_from_email_domain_or_id() {
        assert_eq!(tenant_of(" Anna@Example.ORG "), "example.org");
        assert_eq!(tenant_of("contoso.com"), "contoso.com");
        assert_eq!(tenant_of("72f988bf-86f1-41af-91ab-2d7cd011db47"), "72f988bf-86f1-41af-91ab-2d7cd011db47");
    }

    #[test]
    fn url_encoding_round_trip() {
        assert_eq!(urlenc("a b/c:d"), "a%20b%2Fc%3Ad");
        assert_eq!(urldec("a+b%20c%2F%zz"), "a b c/%zz");
        assert_eq!(urldec("%"), "%");
        assert_eq!(urldec("%4"), "%4");
    }

    #[test]
    fn auth_url_has_tenant_pkce_and_redirect() {
        let p = Pkce { verifier: "v".into(), challenge: "ch".into(), state: "st".into() };
        let u = auth_url(AUTH_BASE, "example.org", "cid", "http://localhost:5000", &p);
        assert!(u.starts_with("https://login.microsoftonline.com/example.org/oauth2/v2.0/authorize?"));
        assert!(u.contains("client_id=cid"));
        assert!(u.contains("redirect_uri=http%3A%2F%2Flocalhost%3A5000"));
        assert!(u.contains("scope=offline_access%20Calendars.Read%20User.Read"));
        assert!(u.contains("code_challenge=ch&code_challenge_method=S256"));
        assert!(u.contains("state=st"));
        assert!(u.contains("response_mode=query"));
        assert!(!u.contains("&v&") && !u.contains("verifier"));
    }

    #[test]
    fn redirect_parsing() {
        let ok = "GET /?code=abc%2Fd&state=s1&session_state=x HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(parse_redirect(ok, "s1"), Ok(Redirect::Code("abc/d".into())));
        assert!(parse_redirect(ok, "other").unwrap_err().contains("doesn't match"));
        let err = "GET /?error=access_denied&error_description=AADSTS65004%3A+User+declined&state=s1 HTTP/1.1\r\n\r\n";
        assert_eq!(
            parse_redirect(err, "s1"),
            Ok(Redirect::Error { code: "access_denied".into(), desc: "AADSTS65004: User declined".into() })
        );
        assert!(parse_redirect("GET /favicon.ico HTTP/1.1\r\n\r\n", "s1").is_err());
        assert!(parse_redirect("POST /?code=a&state=s1 HTTP/1.1\r\n\r\n", "s1").is_err());
        assert!(parse_redirect("GET /?state=s1 HTTP/1.1\r\n\r\n", "s1").unwrap_err().contains("no code"));
    }

    #[test]
    fn token_replies() {
        let ok = br#"{"token_type":"Bearer","access_token":"AT","refresh_token":"RT2","expires_in":3599}"#;
        assert_eq!(
            parse_token_response(200, ok),
            Ok(Tokens { access: "AT".into(), refresh: Some("RT2".into()), expires_in: 3599 })
        );
        let no_rt = br#"{"access_token":"AT","expires_in":"60"}"#;
        assert_eq!(parse_token_response(200, no_rt).unwrap().refresh, None);
        assert_eq!(parse_token_response(200, no_rt).unwrap().expires_in, 60);
        let grant = br#"{"error":"invalid_grant","error_description":"AADSTS70008: The refresh token has expired.\r\nTrace ID: x"}"#;
        assert!(matches!(parse_token_response(400, grant), Err(TokenError::InvalidGrant(m)) if m.contains("AADSTS70008") && !m.contains("Trace")));
        let consent = br#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented"}"#;
        assert_eq!(parse_token_response(400, consent), Err(TokenError::Consent(CONSENT_MESSAGE.into())));
        let admin = br#"{"error":"access_denied","error_description":"AADSTS90094: Admin approval required"}"#;
        assert!(matches!(parse_token_response(400, admin), Err(TokenError::Consent(_))));
        assert!(matches!(parse_token_response(500, b"<html>"), Err(TokenError::Other(m)) if m.contains("500")));
        assert!(matches!(parse_token_response(200, b"{}"), Err(TokenError::Other(_))));
    }

    #[test]
    fn graph_times() {
        use chrono::TimeZone;
        let t = graph_time("2026-11-27T09:30:00.0000000", false).unwrap();
        assert_eq!(t, chrono::Utc.with_ymd_and_hms(2026, 11, 27, 9, 30, 0).unwrap().timestamp());
        assert_eq!(graph_time("2026-11-27T09:30:00Z", false), Some(t));
        assert_eq!(graph_time("nope", false), None);
        // An all-day event of a calendar in UTC+1 starts 23:00 UTC the day before.
        let day = |d| local_midnight(NaiveDate::from_ymd_opt(2026, 11, d).unwrap()).unwrap();
        assert_eq!(graph_time("2026-11-26T23:00:00.0000000", true), Some(day(27)));
        // ... and in UTC-5 at 05:00 UTC the same day.
        assert_eq!(graph_time("2026-11-27T05:00:00.0000000", true), Some(day(27)));
    }

    #[test]
    fn events_page() {
        let body = br#"{"value":[
            {"subject":"Stand-up","isAllDay":false,"isCancelled":false,"start":{"dateTime":"2026-11-27T08:00:00.0000000","timeZone":"UTC"},"end":{"dateTime":"2026-11-27T08:15:00.0000000","timeZone":"UTC"},"location":{"displayName":"Room 1"}},
            {"subject":"Dropped","isCancelled":true,"start":{"dateTime":"2026-11-27T09:00:00"},"end":{"dateTime":"2026-11-27T10:00:00"}},
            {"subject":null,"isAllDay":true,"start":{"dateTime":"2026-11-27T00:00:00.0000000"},"end":{"dateTime":"2026-11-28T00:00:00.0000000"},"location":null},
            {"subject":"Bad","start":{"dateTime":"x"},"end":{"dateTime":"y"}}
        ],"@odata.nextLink":"https://graph.microsoft.com/v1.0/me/calendarView?$skip=100"}"#;
        let (ev, next) = parse_events_page(body).unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].title, "Stand-up");
        assert_eq!(ev[0].loc, "Room 1");
        assert_eq!(ev[0].end - ev[0].start, 900);
        assert_eq!(ev[1].title, "(No title)");
        assert!(ev[1].all_day);
        assert_eq!(next.as_deref(), Some("https://graph.microsoft.com/v1.0/me/calendarView?$skip=100"));
        assert!(parse_events_page(b"{}").is_err());
    }

    #[tokio::test]
    async fn token_request_sends_form_without_secrets_in_errors() {
        let ok = br#"{"access_token":"AT","refresh_token":"RT2","expires_in":3600}"#.to_vec();
        let srv = fake(vec![("/example.org/oauth2/v2.0/token", 200, ok)]).await;
        let base = format!("http://127.0.0.1:{}", srv.port);
        let c = reqwest::Client::new();
        let t = refresh(&c, &base, "example.org", "cid", "RT1").await.unwrap();
        assert_eq!(t.refresh.as_deref(), Some("RT2"));
        let head = srv.requests.lock().unwrap()[0].to_ascii_lowercase();
        assert!(head.starts_with("post /example.org/oauth2/v2.0/token"));
        assert!(head.contains("content-type: application/x-www-form-urlencoded"));
        assert!(!head.contains("origin:"));

        let bad = br#"{"error":"invalid_grant","error_description":"AADSTS50173: grant revoked"}"#.to_vec();
        let srv = fake(vec![("/t/oauth2/v2.0/token", 400, bad)]).await;
        let base = format!("http://127.0.0.1:{}", srv.port);
        let e = exchange_code(&c, &base, "t", "cid", "CODE", "http://localhost:1", "VER").await.unwrap_err();
        assert!(matches!(&e, TokenError::InvalidGrant(m) if !m.contains("CODE") && !m.contains("VER")));
    }

    #[tokio::test]
    async fn fetch_follows_pages_on_the_same_host_only() {
        // A server whose page 1 links to its own page 2 (its port is known before serving).
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let graph = format!("http://127.0.0.1:{port}/v1.0");
        let ev = |t: &str, h: u32| format!(r#"{{"subject":"{t}","start":{{"dateTime":"2026-11-27T{h:02}:00:00"}},"end":{{"dateTime":"2026-11-27T{h:02}:30:00"}}}}"#);
        let page1 = format!(r#"{{"value":[{}],"@odata.nextLink":"{graph}/p2"}}"#, ev("A", 8));
        let page2 = format!(r#"{{"value":[{}]}}"#, ev("B", 10));
        let heads = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = heads.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { break };
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if head.starts_with("GET /v1.0/p2") { page2.clone() } else { page1.clone() };
                log.lock().unwrap().push(head);
                let r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                s.write_all(r.as_bytes()).await.ok();
                s.shutdown().await.ok();
            }
        });
        let c = reqwest::Client::new();
        let got = fetch_events(&c, &graph, "AT", 0, 1).await.unwrap();
        assert_eq!(got.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        let first = heads.lock().unwrap()[0].to_ascii_lowercase();
        assert!(first.starts_with("get /v1.0/me/calendarview?startdatetime=1970-01-01t00%3a00%3a00z"));
        assert!(first.contains("authorization: bearer at"));
        assert!(first.contains("prefer: outlook.timezone=\"utc\""));

        // A next page on another host (here: another port) is refused; the token stays put.
        let other = format!(r#"{{"value":[],"@odata.nextLink":"http://127.0.0.1:{}/v1.0/p2"}}"#, port.wrapping_add(1));
        let srv = fake(vec![("/v1.0/me/calendarView", 200, other.into_bytes())]).await;
        let g = format!("http://127.0.0.1:{}/v1.0", srv.port);
        let e = fetch_events(&c, &g, "AT", 0, 1).await.unwrap_err();
        assert!(e.to_string().contains("another host"));
    }

    #[tokio::test]
    async fn fetch_maps_401() {
        let srv = fake(vec![("/v1.0/me/calendarView", 401, b"{}".to_vec())]).await;
        let g = format!("http://127.0.0.1:{}/v1.0", srv.port);
        let e = fetch_events(&reqwest::Client::new(), &g, "AT", 0, 1).await.unwrap_err();
        assert!(matches!(e, GraphError::Unauthorized));
    }

    #[tokio::test]
    async fn loopback_serves_the_redirect_and_skips_strays() {
        let (l, port) = bind_loopback().await.unwrap();
        let task = tokio::spawn(async move { wait_redirect(l, "s1", Duration::from_secs(10), || false).await });
        let send = |req: &'static str| async move {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            s.write_all(req.as_bytes()).await.unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.ok();
            out
        };
        let stray = send("GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert!(stray.starts_with("HTTP/1.1 404"));
        let page = send("GET /?code=C1&state=s1 HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert!(page.contains("close this tab"));
        assert_eq!(task.await.unwrap().unwrap(), Redirect::Code("C1".into()));
    }

    #[tokio::test]
    async fn loopback_cancel_and_wrong_state() {
        let (l, _) = bind_loopback().await.unwrap();
        let e = wait_redirect(l, "s1", Duration::from_secs(10), || true).await.unwrap_err();
        assert!(e.to_string().contains("cancelled"));

        let (l, port) = bind_loopback().await.unwrap();
        let task = tokio::spawn(async move { wait_redirect(l, "s1", Duration::from_secs(10), || false).await });
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(b"GET /?code=C&state=evil HTTP/1.1\r\n\r\n").await.unwrap();
        let e = task.await.unwrap().unwrap_err();
        assert!(e.to_string().contains("doesn't match"));
    }
}
