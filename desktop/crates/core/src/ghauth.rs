//! "Sign in with GitHub" for the contribution chart: the device flow of the Desk Hub
//! GitHub App (no client secret, no redirect). The app shows a short code, the user enters
//! it on github.com/login/device, and the app polls for the token.
//!
//! User tokens of a GitHub App expire after 8 hours; the refresh token (6 months) rotates
//! on every use, so the newest pair is kept in Credential Manager ("github-oauth") as JSON.
//! The token can only read what the app may read (Contents, Metadata: read-only) on the
//! repositories it is installed on. Device codes, tokens and token replies are never logged.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::msgraph::{read_any, urlenc};

pub const BASE: &str = "https://github.com";
/// The Desk Hub GitHub App (public; device flow needs no secret).
pub const CLIENT_ID: &str = "Iv23liifCKcvhLwvK3NJ";

/// A started sign-in: show `user_code`, open `verification_uri`, poll with `device_code`.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds between polls.
    pub interval: u64,
    /// Seconds until the code stops working.
    pub expires_in: u64,
}

/// The stored sign-in (unix seconds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    pub expires_at: i64,
    pub refresh_expires_at: i64,
}

impl Tokens {
    /// True when the access token runs out within `margin` seconds.
    pub fn expiring(&self, now: i64, margin: i64) -> bool {
        self.expires_at - margin <= now
    }
}

#[derive(Debug, PartialEq)]
pub enum Poll {
    Done(Tokens),
    /// Not entered yet: ask again after the interval.
    Pending,
    /// Too fast: the new interval in seconds.
    SlowDown(u64),
    /// The user pressed Cancel on GitHub.
    Denied,
    /// The code ran out: start again.
    Expired,
    Error(String),
}

#[derive(Debug, PartialEq)]
pub enum RefreshError {
    /// The refresh token is no longer valid: sign in again.
    SignIn(String),
    Other(String),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefreshError::SignIn(m) | RefreshError::Other(m) => f.write_str(m),
        }
    }
}

fn json(status: u16, body: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(body).map_err(|_| format!("GitHub answered HTTP {status} without JSON"))
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn n(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(|e| e.as_u64().or_else(|| e.as_str()?.parse().ok()))
}

/// The first line of GitHub's error text, short.
fn describe(v: &Value) -> String {
    let d = s(v, "error_description");
    let first = d.lines().next().unwrap_or("").trim();
    if first.is_empty() { s(v, "error") } else { first.chars().take(200).collect() }
}

pub fn parse_device(status: u16, body: &[u8]) -> Result<Device, String> {
    let v = json(status, body)?;
    if v.get("error").is_some() {
        return Err(match s(&v, "error").as_str() {
            "device_flow_disabled" => "The Desk Hub GitHub App has the device flow switched off.".into(),
            _ => format!("GitHub refused the sign-in ({})", describe(&v)),
        });
    }
    if !(200..300).contains(&status) {
        return Err(format!("GitHub answered HTTP {status}"));
    }
    let d = Device {
        device_code: s(&v, "device_code"),
        user_code: s(&v, "user_code"),
        verification_uri: s(&v, "verification_uri"),
        interval: n(&v, "interval").unwrap_or(5).max(1),
        expires_in: n(&v, "expires_in").unwrap_or(900),
    };
    if d.device_code.is_empty() || d.user_code.is_empty() || !d.verification_uri.starts_with("https://github.com/") {
        return Err("GitHub sent an incomplete sign-in code".into());
    }
    Ok(d)
}

fn tokens_of(v: &Value, now: i64) -> Option<Tokens> {
    let access = s(v, "access_token");
    let refresh = s(v, "refresh_token");
    if access.is_empty() || refresh.is_empty() {
        return None;
    }
    let ttl = |k: &str, d: u64| i64::try_from(n(v, k).unwrap_or(d)).unwrap_or(0);
    Some(Tokens {
        access,
        refresh,
        expires_at: now + ttl("expires_in", 28_800),
        refresh_expires_at: now + ttl("refresh_token_expires_in", 15_897_600),
    })
}

const NO_REFRESH: &str = "GitHub sent no refresh token. Switch on \"Expire user authorization tokens\" in the GitHub App.";

pub fn parse_poll(status: u16, body: &[u8], now: i64) -> Poll {
    let v = match json(status, body) {
        Ok(v) => v,
        Err(e) => return Poll::Error(e),
    };
    if v.get("error").is_some() {
        return match s(&v, "error").as_str() {
            "authorization_pending" => Poll::Pending,
            "slow_down" => Poll::SlowDown(n(&v, "interval").unwrap_or(10)),
            "access_denied" => Poll::Denied,
            // The docs name both.
            "expired_token" | "token_expired" => Poll::Expired,
            _ => Poll::Error(format!("GitHub: {}", describe(&v))),
        };
    }
    if !(200..300).contains(&status) {
        return Poll::Error(format!("GitHub answered HTTP {status}"));
    }
    match tokens_of(&v, now) {
        Some(t) => Poll::Done(t),
        None if s(&v, "access_token").is_empty() => Poll::Error("GitHub sent no access token".into()),
        None => Poll::Error(NO_REFRESH.into()),
    }
}

pub fn parse_refresh(status: u16, body: &[u8], now: i64) -> Result<Tokens, RefreshError> {
    let v = json(status, body).map_err(RefreshError::Other)?;
    if v.get("error").is_some() {
        let e = s(&v, "error");
        return Err(if e == "bad_refresh_token" || e == "invalid_grant" {
            RefreshError::SignIn("The GitHub sign-in ran out. Sign in again.".into())
        } else {
            RefreshError::Other(format!("GitHub: {}", describe(&v)))
        });
    }
    if !(200..300).contains(&status) {
        return Err(RefreshError::Other(format!("GitHub answered HTTP {status}")));
    }
    tokens_of(&v, now).ok_or_else(|| RefreshError::Other(NO_REFRESH.into()))
}

async fn form_post(client: &reqwest::Client, url: String, form: String) -> Result<(u16, Vec<u8>)> {
    let resp = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(reqwest::header::ACCEPT, "application/json")
        .body(form)
        .send()
        .await
        .map_err(crate::plex::send_err)?;
    read_any(resp).await.context("GitHub's reply")
}

pub async fn device_code(client: &reqwest::Client, base: &str, client_id: &str) -> Result<Device> {
    let (status, body) = form_post(client, format!("{base}/login/device/code"), format!("client_id={}", urlenc(client_id))).await?;
    parse_device(status, &body).map_err(|e| anyhow!(e))
}

pub async fn poll(client: &reqwest::Client, base: &str, client_id: &str, device_code: &str, now: i64) -> Poll {
    let form = format!(
        "client_id={}&device_code={}&grant_type={}",
        urlenc(client_id),
        urlenc(device_code),
        urlenc("urn:ietf:params:oauth:grant-type:device_code")
    );
    match form_post(client, format!("{base}/login/oauth/access_token"), form).await {
        Ok((status, body)) => parse_poll(status, &body, now),
        Err(e) => Poll::Error(format!("{e:#}")),
    }
}

/// A new token pair; the old refresh token stops working. Device-flow tokens need no secret.
pub async fn refresh(client: &reqwest::Client, base: &str, client_id: &str, refresh_token: &str, now: i64) -> Result<Tokens, RefreshError> {
    let form = format!(
        "client_id={}&grant_type=refresh_token&refresh_token={}",
        urlenc(client_id),
        urlenc(refresh_token)
    );
    let (status, body) = form_post(client, format!("{base}/login/oauth/access_token"), form)
        .await
        .map_err(|e| RefreshError::Other(format!("{e:#}")))?;
    parse_refresh(status, &body, now)
}

pub fn encode(t: &Tokens) -> String {
    serde_json::to_string(t).unwrap_or_default()
}

pub fn decode(s: &str) -> Option<Tokens> {
    serde_json::from_str::<Tokens>(s).ok().filter(|t| !t.access.is_empty() && !t.refresh.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_reply() {
        let body = br#"{"device_code":"dc","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":899,"interval":5}"#;
        let d = parse_device(200, body).unwrap();
        assert_eq!(d.user_code, "WDJB-MJHT");
        assert_eq!((d.interval, d.expires_in), (5, 899));
        assert!(parse_device(200, br#"{"error":"device_flow_disabled"}"#).unwrap_err().contains("device flow"));
        // Only GitHub's own page is opened.
        let evil = br#"{"device_code":"dc","user_code":"X","verification_uri":"https://evil.example/","interval":5}"#;
        assert!(parse_device(200, evil).is_err());
        assert!(parse_device(500, b"oops").is_err());
    }

    #[test]
    fn poll_states() {
        let p = |b: &[u8]| parse_poll(200, b, 1000);
        assert_eq!(p(br#"{"error":"authorization_pending"}"#), Poll::Pending);
        assert_eq!(p(br#"{"error":"slow_down","interval":10}"#), Poll::SlowDown(10));
        assert_eq!(p(br#"{"error":"access_denied"}"#), Poll::Denied);
        assert_eq!(p(br#"{"error":"expired_token"}"#), Poll::Expired);
        assert_eq!(p(br#"{"error":"token_expired"}"#), Poll::Expired);
        assert!(matches!(p(br#"{"error":"incorrect_client_credentials","error_description":"bad id"}"#), Poll::Error(m) if m.contains("bad id")));
        let done = p(br#"{"access_token":"ghu_a","expires_in":28800,"refresh_token":"ghr_b","refresh_token_expires_in":15897600,"token_type":"bearer"}"#);
        assert_eq!(done, Poll::Done(Tokens { access: "ghu_a".into(), refresh: "ghr_b".into(), expires_at: 29_800, refresh_expires_at: 15_898_600 }));
        assert!(matches!(p(br#"{"access_token":"ghu_a","token_type":"bearer"}"#), Poll::Error(m) if m.contains("Expire user")));
    }

    #[test]
    fn refresh_reply() {
        let t = parse_refresh(200, br#"{"access_token":"ghu_n","expires_in":"28800","refresh_token":"ghr_n"}"#, 10).unwrap();
        assert_eq!((t.access.as_str(), t.refresh.as_str(), t.expires_at), ("ghu_n", "ghr_n", 28_810));
        assert!(matches!(parse_refresh(200, br#"{"error":"bad_refresh_token"}"#, 0), Err(RefreshError::SignIn(_))));
        assert!(matches!(parse_refresh(502, b"<html>", 0), Err(RefreshError::Other(_))));
    }

    #[test]
    fn stored_round_trip() {
        let t = Tokens { access: "a".into(), refresh: "r".into(), expires_at: 100, refresh_expires_at: 200 };
        assert_eq!(decode(&encode(&t)), Some(t.clone()));
        assert_eq!(decode("ghp_plain"), None);
        assert!(t.expiring(50, 60));
        assert!(!t.expiring(30, 60));
    }
}
