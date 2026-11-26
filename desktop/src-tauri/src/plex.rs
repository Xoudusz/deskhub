//! "Sign in with Plex" for the media-server section: a plex.tv PIN the user approves in
//! the browser, then a server from the account. The account token lives in Credential
//! Manager ("Desk Hub", "plex-token"), never in settings.json. The collector is
//! deskhub_core::collectors::plex.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use deskhub_core::plex;
use deskhub_core::secrets;
use deskhub_core::settings::MediaServer;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::App;

const POLL: Duration = Duration::from_secs(2);
/// plex.tv PINs expire after a few minutes; stop waiting after five.
const WAIT: Duration = Duration::from_secs(5 * 60);

/// The running sign-in: a newer start or a cancel bumps the number and the old poll stops.
#[derive(Default)]
pub struct SignIn(AtomicU64);

#[derive(Clone, Serialize)]
struct Outcome {
    /// "done", "error" or "expired".
    state: &'static str,
    message: String,
}

#[derive(Serialize)]
pub struct Server {
    id: String,
    name: String,
    owned: bool,
}

fn token() -> Result<String, String> {
    secrets::system_store_for(secrets::PLEX_TOKEN)
        .get()
        .map_err(|e| format!("credential store: {e:#}"))?
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "Not signed in to Plex".to_string())
}

/// Opens `url` in the default browser.
fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if let Err(e) = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
        {
            log::warn!("could not open the browser: {e}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = url;
    }
}

fn update_media_server(app: &AppHandle, f: impl FnOnce(&mut MediaServer)) -> Result<(), String> {
    let state = app.state::<App>();
    let mut next = state.settings();
    f(&mut next.media_server);
    state.apply(app, next).map(|_| ())
}

/// Starts a sign-in: opens the Plex page and returns its address (to show if no browser
/// opened). The outcome arrives as a "plex-sign-in" event.
#[tauri::command]
pub async fn plex_sign_in(app: AppHandle, state: tauri::State<'_, App>) -> Result<String, String> {
    let client_id = state.settings().media_server.client_id;
    let pin = plex::pin_start(plex::TV, &client_id)
        .await
        .map_err(|e| format!("plex.tv: {e:#}"))?;
    let url = plex::auth_url(&client_id, &pin.code);
    open_browser(&url);
    let run = app.state::<SignIn>().0.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = wait_for_token(&handle, run, &client_id, pin.id).await;
        if let Some(o) = outcome {
            let _ = handle.emit("plex-sign-in", o);
        }
    });
    Ok(url)
}

/// Polls the PIN until approved, expired, cancelled (None) or out of time.
async fn wait_for_token(app: &AppHandle, run: u64, client_id: &str, pin: i64) -> Option<Outcome> {
    let current = || app.state::<SignIn>().0.load(Ordering::SeqCst) == run;
    let err = |m: String| Some(Outcome { state: "error", message: m });
    let start = std::time::Instant::now();
    loop {
        tokio::time::sleep(POLL).await;
        if !current() {
            return None;
        }
        if start.elapsed() > WAIT {
            return Some(Outcome { state: "expired", message: "The sign-in timed out. Try again.".into() });
        }
        let token = match plex::pin_poll(plex::TV, client_id, pin).await {
            Ok(Some(t)) => t,
            Ok(None) => continue,
            Err(e) if e.to_string().contains("expired") => {
                return Some(Outcome { state: "expired", message: "The sign-in code expired. Try again.".into() })
            }
            Err(e) => {
                // A network blip: keep waiting.
                log::info!("plex sign-in: {e:#}");
                continue;
            }
        };
        if !current() {
            return None;
        }
        let store = secrets::system_store_for(secrets::PLEX_TOKEN);
        if let Err(e) = secrets::store_verified(store.as_ref(), &token) {
            return err(format!("Could not save the Plex sign-in: {e:#}"));
        }
        // A single server on the account is picked right away.
        let only = plex::resources(plex::TV, client_id, &token)
            .await
            .ok()
            .filter(|r| r.len() == 1)
            .and_then(|mut r| r.pop());
        let saved = update_media_server(app, |m| {
            m.signed_in = true;
            m.server_id = only.as_ref().map(|r| r.id.clone()).unwrap_or_default();
            m.server_name = only.as_ref().map(|r| r.name.clone()).unwrap_or_default();
        });
        return match saved {
            Ok(()) => Some(Outcome { state: "done", message: String::new() }),
            Err(e) => err(e),
        };
    }
}

#[tauri::command]
pub fn plex_sign_in_cancel(app: AppHandle) {
    app.state::<SignIn>().0.fetch_add(1, Ordering::SeqCst);
}

/// The servers on the signed-in account.
#[tauri::command]
pub async fn plex_servers(state: tauri::State<'_, App>) -> Result<Vec<Server>, String> {
    let client_id = state.settings().media_server.client_id;
    let token = token()?;
    let list = plex::resources(plex::TV, &client_id, &token)
        .await
        .map_err(|e| format!("plex.tv: {e:#}"))?;
    Ok(list
        .into_iter()
        .map(|r| Server { id: r.id, name: r.name, owned: r.owned })
        .collect())
}

#[tauri::command]
pub fn plex_select_server(app: AppHandle, id: String, name: String) -> Result<(), String> {
    token()?;
    update_media_server(&app, |m| {
        m.server_id = id;
        m.server_name = name;
    })
}

/// Forgets the account token and the server (the client id stays: it names this install).
#[tauri::command]
pub fn plex_sign_out(app: AppHandle) -> Result<(), String> {
    app.state::<SignIn>().0.fetch_add(1, Ordering::SeqCst);
    secrets::system_store_for(secrets::PLEX_TOKEN)
        .delete()
        .map_err(|e| format!("credential store: {e:#}"))?;
    update_media_server(&app, |m| {
        m.signed_in = false;
        m.server_id.clear();
        m.server_name.clear();
    })
}
