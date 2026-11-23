//! App updates from GitHub Releases (plugins.updater in tauri.conf.json). The check runs here,
//! not in the webview: the window lives hidden in the tray and WebView2 throttles its timers,
//! so a JS interval only fired after a restart. Checks 20 s after start, then every 10 minutes,
//! downloads a new version in the background and tells the UI ("update" event); the title bar
//! button then installs it (passive NSIS) and restarts.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const FIRST: Duration = Duration::from_secs(20);
const EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
struct Status {
    /// The newer version found (downloaded once `ready`).
    version: Option<String>,
    ready: bool,
    checking: bool,
    installing: bool,
    error: Option<String>,
    /// Unix seconds of the last finished check.
    checked: Option<u64>,
}

#[derive(Default)]
pub struct Updates {
    status: Mutex<Status>,
    /// The downloaded update and its signature-checked-at-install bytes.
    ready: Mutex<Option<(Update, Vec<u8>)>>,
    /// One check at a time (timer and "Check now" can overlap).
    busy: tokio::sync::Mutex<()>,
}

fn snapshot(app: &AppHandle) -> Value {
    let s = app.state::<Updates>();
    let st = s.status.lock().unwrap();
    json!({
        "current": app.package_info().version.to_string(),
        "version": st.version,
        "ready": st.ready,
        "checking": st.checking,
        "installing": st.installing,
        "error": st.error,
        "checked": st.checked,
    })
}

fn set(app: &AppHandle, f: impl FnOnce(&mut Status)) {
    f(&mut app.state::<Updates>().status.lock().unwrap());
    let _ = app.emit("update", snapshot(app));
}

async fn find_and_download(app: &AppHandle) -> Result<Option<(Update, Vec<u8>)>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    log::info!("app update {} found, downloading", update.version);
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some((update, bytes)))
}

/// Check now unless a check runs or an update already waits; returns the status.
pub async fn check_now(app: &AppHandle) -> Value {
    let state = app.state::<Updates>();
    let Ok(_guard) = state.busy.try_lock() else {
        return snapshot(app);
    };
    {
        let st = state.status.lock().unwrap();
        if st.ready || st.installing {
            drop(st);
            return snapshot(app);
        }
    }
    set(app, |s| s.checking = true);
    let result = find_and_download(app).await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok();
    match result {
        Ok(Some((update, bytes))) => {
            let version = update.version.clone();
            *state.ready.lock().unwrap() = Some((update, bytes));
            set(app, |s| {
                s.version = Some(version);
                s.ready = true;
                s.error = None;
            });
        }
        Ok(None) => set(app, |s| {
            s.version = None;
            s.error = None;
        }),
        Err(e) => {
            log::warn!("app update check failed: {e}");
            set(app, |s| s.error = Some(e));
        }
    }
    set(app, |s| {
        s.checking = false;
        s.checked = now;
    });
    snapshot(app)
}

/// The background check loop; call once from setup (after `manage(Updates::default())`).
pub fn spawn_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST).await;
        loop {
            check_now(&app).await;
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> Value {
    snapshot(&app)
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> Value {
    check_now(&app).await
}

/// Install the downloaded update and restart (on Windows the installer ends this process).
#[tauri::command]
pub fn update_install(app: AppHandle) -> Result<(), String> {
    let Some((update, bytes)) = app.state::<Updates>().ready.lock().unwrap().take() else {
        return Err("no update downloaded".into());
    };
    set(&app, |s| s.installing = true);
    if let Err(e) = update.install(&bytes) {
        let msg = e.to_string();
        set(&app, |s| {
            s.installing = false;
            s.ready = false;
            s.error = Some(msg.clone());
        });
        return Err(msg);
    }
    app.restart();
}
