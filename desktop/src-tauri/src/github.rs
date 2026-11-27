//! Settings > Calendar & GitHub: "Sign in with GitHub" (device flow, deskhub_core::ghauth),
//! the GitHub token fallback and both collectors' status.
//! The collectors are deskhub_core::collectors::{calendar, github}.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use deskhub_core::collectors::{calendar, github};
use deskhub_core::ghauth::{self, Poll};
use deskhub_core::{msgraph, secrets};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::plex::open_browser;

#[derive(Serialize)]
pub struct GithubStatus {
    /// A personal access token is saved (the fallback).
    token_saved: bool,
    /// "Sign in with GitHub" is active (used before the token).
    signed_in: bool,
    /// Not tried yet (just saved, or no token).
    pending: bool,
    ok: bool,
    error: String,
    login: String,
    total: i64,
}

/// Saves the GitHub token in the credential store ("" forgets it) and asks again now.
#[tauri::command]
pub fn github_token_set(token: String) -> Result<(), String> {
    let store = secrets::system_store_for(secrets::GITHUB_TOKEN);
    let token = token.trim();
    let result = if token.is_empty() {
        store.delete().map_err(|e| format!("credential store: {e:#}"))
    } else if !(token.starts_with("github_pat_") || token.starts_with("ghp_")) || token.len() < 20 {
        Err("That doesn't look like a GitHub token (github_pat_... or ghp_...)".into())
    } else {
        secrets::store_verified(store.as_ref(), token).map_err(|e| format!("{e:#}"))
    };
    if result.is_ok() {
        github::WAKE.notify_one();
    }
    result
}

#[tauri::command]
pub fn github_status() -> GithubStatus {
    let token_saved = github::token().is_some();
    let signed_in = github::app_tokens().is_some();
    let s = github::status().filter(|_| token_saved || signed_in || github::sign_in_lost());
    GithubStatus {
        token_saved,
        signed_in,
        pending: s.is_none() && (token_saved || signed_in),
        ok: s.as_ref().is_some_and(|s| s.ok),
        error: s.as_ref().map(|s| s.error.clone()).unwrap_or_default(),
        login: s.as_ref().map(|s| s.login.clone()).unwrap_or_default(),
        total: s.map(|s| s.total).unwrap_or(0),
    }
}

/// The last read of each calendar feed (name, ok, error, events).
#[tauri::command]
pub fn calendar_status() -> Vec<calendar::FeedStatus> {
    calendar::status()
}

/// The running sign-in: a newer start, a cancel or a sign-out bumps the number and the
/// old poll loop stops.
#[derive(Default)]
pub struct GhSignIn(AtomicU64);

#[derive(Clone, Serialize)]
struct Outcome {
    /// "done", "error" or "expired".
    state: &'static str,
    message: String,
}

#[derive(Serialize)]
pub struct DeviceStart {
    user_code: String,
    uri: String,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Asks GitHub for a sign-in code and opens github.com/login/device; returns the code to
/// show. The outcome arrives as a "gh-sign-in" event.
#[tauri::command]
pub async fn gh_sign_in(app: AppHandle) -> Result<DeviceStart, String> {
    let client = msgraph::client().map_err(|e| format!("{e:#}"))?;
    let d = ghauth::device_code(&client, ghauth::BASE, ghauth::CLIENT_ID).await.map_err(|e| format!("{e:#}"))?;
    open_browser(&d.verification_uri);
    let run = app.state::<GhSignIn>().0.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = app.clone();
    let device_code = d.device_code;
    let (mut interval, expires_in) = (d.interval, d.expires_in);
    tauri::async_runtime::spawn(async move {
        let current = || handle.state::<GhSignIn>().0.load(Ordering::SeqCst) == run;
        let deadline = Instant::now() + Duration::from_secs(expires_in);
        let expired = || Outcome { state: "expired", message: "The code ran out. Try again.".into() };
        let error = |m: String| Outcome { state: "error", message: m };
        let outcome = loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            if !current() {
                break None;
            }
            if Instant::now() >= deadline {
                break Some(expired());
            }
            match ghauth::poll(&client, ghauth::BASE, ghauth::CLIENT_ID, &device_code, unix_now()).await {
                Poll::Pending => {}
                Poll::SlowDown(i) => interval = i.max(interval + 5),
                Poll::Denied => break Some(error("The sign-in was cancelled on GitHub.".into())),
                Poll::Expired => break Some(expired()),
                Poll::Error(m) => break Some(error(m)),
                Poll::Done(t) => break keep_sign_in(&t, current),
            }
        };
        if let Some(o) = outcome.filter(|_| current()) {
            let _ = handle.emit("gh-sign-in", o);
        }
    });
    Ok(DeviceStart { user_code: d.user_code, uri: d.verification_uri })
}

/// Saves a finished sign-in under the lock, unless it was cancelled or replaced while
/// GitHub answered (then nothing is kept and nothing is reported).
fn keep_sign_in(t: &ghauth::Tokens, current: impl Fn() -> bool) -> Option<Outcome> {
    let error = |m: String| Outcome { state: "error", message: m };
    let outcome = {
        let Ok(_g) = github::AUTH_LOCK.lock() else { return Some(error("sign-in lock poisoned".into())) };
        if !current() {
            return None;
        }
        let store = secrets::system_store_for(secrets::GITHUB_OAUTH);
        match secrets::store_verified(store.as_ref(), &ghauth::encode(t)) {
            Ok(()) => Outcome { state: "done", message: "Signed in. Your contributions show within a minute.".into() },
            Err(e) => return Some(error(format!("Could not save the GitHub sign-in: {e:#}"))),
        }
    };
    github::auth_changed();
    Some(outcome)
}

/// Stops a sign-in that is waiting for the code.
#[tauri::command]
pub fn gh_sign_in_cancel(app: AppHandle) {
    app.state::<GhSignIn>().0.fetch_add(1, Ordering::SeqCst);
}

/// Forgets the GitHub sign-in (a saved token is used again, if any).
#[tauri::command]
pub fn gh_sign_out(app: AppHandle) -> Result<(), String> {
    app.state::<GhSignIn>().0.fetch_add(1, Ordering::SeqCst);
    let r = {
        let _g = github::AUTH_LOCK.lock().map_err(|_| "sign-in lock poisoned".to_string())?;
        secrets::system_store_for(secrets::GITHUB_OAUTH).delete().map_err(|e| format!("credential store: {e:#}"))
    };
    github::auth_changed();
    r
}
