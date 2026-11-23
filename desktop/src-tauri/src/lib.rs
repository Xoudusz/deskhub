//! Tauri shell: window, tray, autostart, single instance. All board logic lives in
//! deskhub-core; this file wraps it in commands and events for the frontend.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use deskhub_core::claude_install;
use deskhub_core::http;
use deskhub_core::session::{Service, ServiceHandle};
use deskhub_core::settings::{find_config, import_config_file, Settings, State, Status};
use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
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
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\DeskHubCompanion\0".encode_utf16().collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        return false;
    }
    // Held until exit (release_companion_mutex), so a restart can claim it again.
    *COMPANION_MUTEX.lock().unwrap() = Some(handle as usize);
    true
}

/// Our handle to the companion mutex, while we hold it.
#[cfg(windows)]
static COMPANION_MUTEX: Mutex<Option<usize>> = Mutex::new(None);

#[cfg(windows)]
fn release_companion_mutex() {
    if let Some(h) = COMPANION_MUTEX.lock().unwrap().take() {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(h as windows_sys::Win32::Foundation::HANDLE)
        };
    }
}

#[cfg(not(windows))]
fn claim_companion_mutex() -> bool {
    true
}

#[cfg(not(windows))]
fn release_companion_mutex() {}

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
fn set_settings(
    app: AppHandle,
    state: tauri::State<'_, App>,
    patch: Value,
) -> Result<Settings, String> {
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
    http::screenshot_data_url(&s.board.host, &s.board.token)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Install a firmware image on the board: `path` or a file picker. The board asks first.
#[tauri::command]
async fn board_firmware(
    app: tauri::AppHandle,
    state: tauri::State<'_, App>,
    path: Option<String>,
) -> Result<String, String> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => {
            use tauri_plugin_dialog::DialogExt;
            let picked = app
                .dialog()
                .file()
                .set_title("Firmware image (deskhub.bin)")
                .add_filter("Firmware", &["bin"])
                .blocking_pick_file();
            match picked.and_then(|f| f.into_path().ok()) {
                Some(p) => p,
                None => return Err("No file chosen".into()),
            }
        }
    };
    let image = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let s = state.settings();
    http::firmware(&s.board.host, &s.board.token, image)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn board_tap(state: tauri::State<'_, App>, x: i64, y: i64) -> Result<(), String> {
    let s = state.settings();
    http::tap(&s.board.host, &s.board.token, x, y)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn import_config(
    app: AppHandle,
    state: tauri::State<'_, App>,
    path: Option<String>,
) -> Result<Settings, String> {
    let explicit = path.filter(|p| !p.trim().is_empty()).map(PathBuf::from);
    let file = find_config(explicit.as_deref()).ok_or("No config.toml found")?;
    let mut next = import_config_file(&file).map_err(|e| format!("{e:#}"))?;
    // The theme is an app setting, not part of config.toml: keep it.
    next.appearance = state.settings().appearance;
    state.apply(&app, next)
}

// ---- Claude Code status line + hooks (AI section) -----------------------------

/// Claude Code's settings.json (CLAUDE_CONFIG_DIR wins, like Claude Code itself).
fn claude_settings_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// deskhub-cli's state folder: DESKHUB_HOME or the user's home, then .deskhub (same rule
/// as deskhub-cli's store::deskhub_dir, so it finds statusline_previous.json).
fn deskhub_state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("DESKHUB_HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }))?;
    Some(PathBuf::from(home).join(".deskhub"))
}

/// deskhub-cli next to the app (the installer puts it there).
fn cli_path() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "deskhub-cli.exe"
    } else {
        "deskhub-cli"
    };
    Some(std::env::current_exe().ok()?.parent()?.join(name))
}

fn claude_state() -> Value {
    let (Some(settings), Some(cli)) = (claude_settings_path(), cli_path()) else {
        return serde_json::json!({ "statusline": false, "hooks": false, "cli": false });
    };
    let st = claude_install::status(&settings, &cli);
    serde_json::json!({ "statusline": st.statusline, "hooks": st.hooks, "cli": cli.is_file() })
}

#[tauri::command]
fn claude_status() -> Value {
    claude_state()
}

/// Connect (on) or disconnect Claude Code: status line + hooks pointing at deskhub-cli.
#[tauri::command]
fn claude_connect(on: bool) -> Result<Value, String> {
    let settings = claude_settings_path().ok_or("No home folder")?;
    let dir = deskhub_state_dir().ok_or("No home folder")?;
    if on {
        let cli = cli_path()
            .filter(|p| p.is_file())
            .ok_or("deskhub-cli is missing next to the app")?;
        claude_install::install_in(&settings, &dir, &cli, claude_install::What::All)
            .map_err(|e| format!("{e:#}"))?;
    } else {
        claude_install::remove_in(&settings, &dir, claude_install::What::All)
            .map_err(|e| format!("{e:#}"))?;
    }
    Ok(claude_state())
}

/// After an update or a move the exe path changes: if Claude Code points at some
/// deskhub-cli but not this one, point it here again.
fn heal_claude_hooks() {
    let (Some(settings), Some(cli), Some(dir)) =
        (claude_settings_path(), cli_path(), deskhub_state_dir())
    else {
        return;
    };
    let mentions = std::fs::read_to_string(&settings).is_ok_and(|t| t.contains("deskhub-cli"));
    let st = claude_install::status(&settings, &cli);
    if mentions && cli.is_file() && !(st.statusline && st.hooks) {
        match claude_install::install_in(&settings, &dir, &cli, claude_install::What::All) {
            Ok(()) => log::info!("Claude Code hooks re-pointed to {}", cli.display()),
            Err(e) => log::warn!("Claude Code hooks: {e:#}"),
        }
    }
}

// ---- app ---------------------------------------------------------------------

pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch only brings the open window forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
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
            heal_claude_hooks();
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
                let svc = tauri::async_runtime::block_on(async {
                    let svc = Service::start(settings.clone());
                    deskhub_core::collectors::start_all(&svc);
                    svc
                });
                let status = svc.status().borrow().clone();
                (Some(svc), status)
            } else {
                let status = Status {
                    state: State::Stopped,
                    host: settings.board.host.clone(),
                    detail: "The Python companion is running. Close it, then restart Desk Hub."
                        .into(),
                    firmware: None,
                    since: None,
                };
                (None, status)
            };

            let status_rx = service.as_ref().map(|svc| svc.status());
            app.manage(App {
                settings: Mutex::new(settings),
                file,
                service,
                status: Mutex::new(status),
            });

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
            let restart = MenuItem::with_id(app, "restart", "Restart", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &restart, &quit])?;
            let mut tray = TrayIconBuilder::with_id("tray")
                .tooltip("Desk Hub")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_main(app),
                    // From the main thread restart() skips RunEvent::Exit, so free the
                    // companion mutex first or the new process sees it taken. The
                    // single-instance lock is freed by Tauri's cleanup_before_exit.
                    "restart" => {
                        release_companion_mutex();
                        app.restart()
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
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
            board_firmware,
            import_config,
            claude_status,
            claude_connect
        ])
        .build(tauri::generate_context!())
        .expect("error while building Desk Hub")
        .run(|_app, event| {
            if let RunEvent::Exit = event {
                release_companion_mutex();
            }
        });
}
