//! "Sign in with Microsoft" for an Outlook calendar (Settings > Calendar & GitHub): the
//! browser signs in, Microsoft sends it back to a one-time listener on 127.0.0.1, and the
//! refresh token goes into Credential Manager ("Desk Hub", "ms-refresh-token"), never into
//! settings.json. The calendar collector (deskhub_core::collectors::calendar) reads it.

use std::sync::atomic::{AtomicU64, Ordering};

use deskhub_core::collectors::calendar;
use deskhub_core::msgraph::{self, Redirect, TokenError};
use deskhub_core::secrets;
use deskhub_core::settings::Calendar;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::plex::open_browser;
use crate::App;

/// The running sign-in: a newer start, a cancel or a sign-out bumps the number and the old
/// listener stops.
#[derive(Default)]
pub struct SignIn(AtomicU64);

#[derive(Clone, Serialize)]
struct Outcome {
    /// "done", "error" or "expired".
    state: &'static str,
    message: String,
}

fn update_calendar(app: &AppHandle, f: impl FnOnce(&mut Calendar)) -> Result<(), String> {
    let state = app.state::<App>();
    let mut next = state.settings();
    f(&mut next.calendar);
    state.apply(app, next).map(|_| ())
}

/// Saves the client id and tenant, opens Microsoft's sign-in page and returns its address
/// (to show if no browser opened). The outcome arrives as an "ms-sign-in" event.
#[tauri::command]
pub async fn ms_sign_in(app: AppHandle, client_id: String, tenant: String) -> Result<String, String> {
    let client_id = client_id.trim().to_string();
    let tenant_input = tenant.trim().to_string();
    if client_id.is_empty() {
        return Err("Enter the Application (client) ID of your app registration first.".into());
    }
    let tenant = msgraph::tenant_of(&tenant_input);
    if tenant.is_empty() {
        return Err("Enter your work email address or your organisation's domain first.".into());
    }
    update_calendar(&app, |c| {
        c.ms_client_id = client_id.clone();
        c.ms_tenant = tenant_input.clone();
    })?;
    let pkce = msgraph::new_pkce().map_err(|e| format!("{e:#}"))?;
    let (listener, port) = msgraph::bind_loopback().await.map_err(|e| format!("{e:#}"))?;
    let redirect = format!("http://localhost:{port}");
    let url = msgraph::auth_url(msgraph::AUTH_BASE, &tenant, &client_id, &redirect, &pkce);
    open_browser(&url);
    let run = app.state::<SignIn>().0.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let current = || handle.state::<SignIn>().0.load(Ordering::SeqCst) == run;
        let got = msgraph::wait_redirect(listener, &pkce.state, msgraph::WAIT, || !current()).await;
        let outcome = match got {
            Err(_) if !current() => None,
            Err(e) if e.to_string().contains("timed out") => {
                Some(Outcome { state: "expired", message: "The sign-in timed out. Try again.".into() })
            }
            Err(e) => Some(Outcome { state: "error", message: format!("{e:#}") }),
            Ok(Redirect::Error { code, desc }) => {
                Some(Outcome { state: "error", message: msgraph::classify_error(&code, &desc).to_string() })
            }
            Ok(Redirect::Code(code)) => {
                Some(finish(&handle, &tenant, &client_id, &code, &redirect, &pkce.verifier).await)
            }
        };
        if let Some(o) = outcome.filter(|_| current()) {
            let _ = handle.emit("ms-sign-in", o);
        }
    });
    Ok(url)
}

/// Trades the code for tokens and keeps the refresh token.
async fn finish(app: &AppHandle, tenant: &str, client_id: &str, code: &str, redirect: &str, verifier: &str) -> Outcome {
    let err = |m: String| Outcome { state: "error", message: m };
    let client = match msgraph::client() {
        Ok(c) => c,
        Err(e) => return err(format!("{e:#}")),
    };
    let tokens = match msgraph::exchange_code(&client, msgraph::AUTH_BASE, tenant, client_id, code, redirect, verifier).await {
        Ok(t) => t,
        Err(TokenError::Consent(m)) | Err(TokenError::InvalidGrant(m)) | Err(TokenError::Other(m)) => return err(m),
    };
    let Some(refresh) = tokens.refresh else {
        return err("Microsoft sent no refresh token. Check that the app registration allows offline_access.".into());
    };
    let store = secrets::system_store_for(secrets::MS_REFRESH_TOKEN);
    if let Err(e) = secrets::store_verified(store.as_ref(), &refresh) {
        return err(format!("Could not save the Microsoft sign-in: {e:#}"));
    }
    if let Err(e) = update_calendar(app, |c| c.ms_signed_in = true) {
        return err(e);
    }
    calendar::signed_in();
    Outcome { state: "done", message: "Signed in. Your Outlook calendar shows within a minute.".into() }
}

/// Stops a sign-in that is waiting for the browser.
#[tauri::command]
pub fn ms_sign_in_cancel(app: AppHandle) {
    app.state::<SignIn>().0.fetch_add(1, Ordering::SeqCst);
}

/// Forgets the Microsoft sign-in (the client id and tenant stay for the next one).
#[tauri::command]
pub fn ms_sign_out(app: AppHandle) -> Result<(), String> {
    app.state::<SignIn>().0.fetch_add(1, Ordering::SeqCst);
    secrets::system_store_for(secrets::MS_REFRESH_TOKEN)
        .delete()
        .map_err(|e| format!("credential store: {e:#}"))?;
    update_calendar(&app, |c| c.ms_signed_in = false)
}
