//! Claude Code on a server (Settings > AI): the relay token and the poller's status.
//! The poller itself is deskhub_core::collectors::remote; its receiver is kept here.

use std::sync::OnceLock;

use deskhub_core::collectors::alert_log::{self, Entry};
use deskhub_core::collectors::remote::RemoteRx;
use deskhub_core::relay::MIN_TOKEN;
use deskhub_core::secrets;
use serde::Serialize;

static REMOTE: OnceLock<RemoteRx> = OnceLock::new();

/// Called once the collectors run.
pub fn keep(rx: RemoteRx) {
    let _ = REMOTE.set(rx);
}

#[derive(Serialize)]
pub struct RemoteStatus {
    configured: bool,
    token_saved: bool,
    ok: bool,
    error: String,
    host: String,
    sessions: usize,
}

/// Saves the relay token in the credential store ("" forgets it).
#[tauri::command]
pub fn remote_token_set(token: String) -> Result<(), String> {
    let store = secrets::system_store_for(secrets::REMOTE_TOKEN);
    let token = token.trim();
    if token.is_empty() {
        return store.delete().map_err(|e| format!("credential store: {e:#}"));
    }
    if token.len() < MIN_TOKEN {
        return Err(format!("The token needs at least {MIN_TOKEN} characters"));
    }
    secrets::store_verified(store.as_ref(), token).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn remote_status() -> RemoteStatus {
    let token_saved = secrets::system_store_for(secrets::REMOTE_TOKEN)
        .get()
        .ok()
        .flatten()
        .is_some_and(|t| !t.is_empty());
    let snap = REMOTE.get().map(|rx| rx.borrow().clone()).unwrap_or_default();
    RemoteStatus {
        configured: snap.configured,
        token_saved,
        ok: snap.ok,
        error: snap.error.clone(),
        host: snap.host.clone(),
        sessions: snap.sessions.len(),
    }
}

/// Settings > AI > Recent alerts: the newest entries of the alert log, newest first.
#[tauri::command]
pub fn alert_log(limit: usize) -> Vec<Entry> {
    alert_log::path().map_or_else(Vec::new, |p| alert_log::recent(&p, limit.min(500)))
}

/// Opens ~/.deskhub, where alert_log.jsonl is.
#[tauri::command]
pub fn alert_log_open() -> Result<(), String> {
    let p = alert_log::path().ok_or("no home folder")?;
    if !p.exists() {
        return Err("Nothing logged yet".into());
    }
    alert_log::open_folder(&p);
    Ok(())
}
