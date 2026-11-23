//! Tauri shell: window, tray, autostart, single instance. All board logic lives in
//! deskhub-core; this file wraps it in commands and events for the frontend.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use deskhub_core::http;
use deskhub_core::session::{Service, ServiceHandle};
use deskhub_core::settings::{find_config, import_config_file, Settings, State, Status};
use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_window_state::StateFlags;

/// Shared app state: the settings (saved to `settings.json` in the app config dir),
/// the board service and its last status.
struct App {
    settings: Mutex<Settings>,
    file: PathBuf,
    service: Option<ServiceHandle>,
    status: Mutex<Status>,
}

impl App {
    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Save, hand to the service, tell the frontend.
    fn apply(&self, app: &AppHandle, next: Settings) -> Result<Settings, String> {
        save(&self.file, &next)?;
        *self.settings.lock().unwrap() = next.clone();
        if let Some(svc) = &self.service {
            svc.update_settings(next.clone());
        }
        let _ = app.emit("settings", &next);
        Ok(next)
    }
}

fn save(file: &Path, s: &Settings) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, file).map_err(|e| e.to_string())
}

/// Load saved settings. First run: import the Python companion's config.toml if one
/// is found, else defaults. Returns the settings and whether this was the first run.
fn load(file: &Path) -> (Settings, bool) {
    if let Ok(text) = std::fs::read_to_string(file) {
        if let Ok(s) = serde_json::from_str::<Settings>(&text) {
            return (s, false);
        }
        log::warn!("settings.json unreadable, starting from defaults");
    }
    let s = find_config(None)
        .and_then(|p| match import_config_file(&p) {
            Ok(s) => {
                log::info!("imported {}", p.display());
                Some(s)
            }
            Err(e) => {
                log::warn!("config.toml import failed: {e:#}");
                None
            }
        })
        .unwrap_or_default();
    (s, true)
}

/// The Python companion holds this mutex while it runs. Holding it here too keeps
/// the two from fighting over the board (it accepts one companion only).
#[cfg(windows)]
fn claim_companion_mutex() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\DeskHubCompanion\0".encode_utf16().collect();
    // The handle is never closed: the mutex lives as long as the app.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    !handle.is_null() && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
}

#[cfg(not(windows))]
fn claim_companion_mutex() -> bool {
    true
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// ---- commands (contract: desktop/README.md) ----------------------------------

#[tauri::command]
fn get_settings(state: tauri::State<'_, App>) -> Settings {
    state.settings()
}

#[tauri::command]
fn set_settings(app: AppHandle, state: tauri::State<'_, App>, patch: Value) -> Result<Settings, String> {
    let mut next = state.settings();
    next.merge_patch(patch);
    state.apply(&app, next)
}

#[tauri::command]
fn get_status(state: tauri::State<'_, App>) -> Status {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
async fn board_screenshot(state: tauri::State<'_, App>) -> Result<String, String> {
    let s = state.settings();
    http::screenshot_data_url(&s.board.host, &s.board.token).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn board_tap(state: tauri::State<'_, App>, x: i64, y: i64) -> Result<(), String> {
    let s = state.settings();
    http::tap(&s.board.host, &s.board.token, x, y).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn import_config(app: AppHandle, state: tauri::State<'_, App>, path: Option<String>) -> Result<Settings, String> {
    let explicit = path.filter(|p| !p.trim().is_empty()).map(PathBuf::from);
    let file = find_config(explicit.as_deref()).ok_or("No config.toml found")?;
    let mut next = import_config_file(&file).map_err(|e| format!("{e:#}"))?;
    // The theme is an app setting, not part of config.toml: keep it.
    next.appearance = state.settings().appearance;
    state.apply(&app, next)
}

// ---- app ---------------------------------------------------------------------

pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch only brings the open window forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        // Size and position only: visibility is ours (--hidden starts in the tray).
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all() - StateFlags::VISIBLE)
                .build(),
        )
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .max_file_size(1_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(4))
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            let file = app.path().app_config_dir()?.join("settings.json");
            let (settings, first_run) = load(&file);
            if first_run {
                if let Err(e) = save(&file, &settings) {
                    log::warn!("could not save settings: {e}");
                }
                // Start with Windows by default, hidden in the tray.
                if let Err(e) = app.autolaunch().enable() {
                    log::warn!("autostart: {e}");
                }
            }

            let (service, status) = if claim_companion_mutex() {
                let svc = tauri::async_runtime::block_on(async { Service::start(settings.clone()) });
                let status = svc.status().borrow().clone();
                (Some(svc), status)
            } else {
                let status = Status {
                    state: State::Stopped,
                    host: settings.board.host.clone(),
                    detail: "The Python companion is running. Close it, then restart Desk Hub.".into(),
                    firmware: None,
                    since: None,
                };
                (None, status)
            };

            let status_rx = service.as_ref().map(|svc| svc.status());
            app.manage(App { settings: Mutex::new(settings), file, service, status: Mutex::new(status) });

            // Forward every status change to the frontend (App is managed by now).
            if let Some(mut rx) = status_rx {
                let h = handle.clone();
                tauri::async_runtime::spawn(async move {
                    while rx.changed().await.is_ok() {
                        let st = rx.borrow_and_update().clone();
                        *h.state::<App>().status.lock().unwrap() = st.clone();
                        let _ = h.emit("status", &st);
                    }
                });
            }


            let open = MenuItem::with_id(app, "open", "Open Desk Hub", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("tray")
                .tooltip("Desk Hub")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            if !std::env::args().any(|a| a == "--hidden") {
                show_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the app running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            get_status,
            board_screenshot,
            board_tap,
            import_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running Desk Hub");
}
