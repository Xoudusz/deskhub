//! Settings > Calendar & GitHub: the GitHub token and both collectors' status.
//! The collectors are deskhub_core::collectors::{calendar, github}.

use deskhub_core::collectors::{calendar, github};
use deskhub_core::secrets;
use serde::Serialize;

#[derive(Serialize)]
pub struct GithubStatus {
    token_saved: bool,
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
    let s = github::status().filter(|_| token_saved);
    GithubStatus {
        token_saved,
        pending: s.is_none(),
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
