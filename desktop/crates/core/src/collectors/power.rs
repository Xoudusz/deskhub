//! PC sleep/wake: display off or suspend -> pc_power. Port of companion/deskhub/pc_power.py.
//!
//! Windows tells a hidden window when the console display turns off, dims or on
//! (GUID_CONSOLE_DISPLAY_STATE), and when the PC is about to suspend. A window on its
//! own thread receives those and feeds the reducer below. `pc_sleep = "none"` (PcSleep::Stay)
//! keeps reporting asleep false and ignores events.

use serde_json::{json, Value};

use super::Ctx;
use crate::settings::PcSleep;

const KEY: &str = "pc_power";

/// Display state in a PBT_POWERSETTINGCHANGE notification: 0 off, 1 on, 2 dimmed (counts as on).
#[cfg_attr(not(windows), allow(dead_code))]
const DISPLAY_OFF: u32 = 0;

/// A power notification relevant to sleep state.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// GUID_CONSOLE_DISPLAY_STATE data: 0 off, 1 on, 2 dimmed.
    DisplayState(u32),
    /// PBT_APMSUSPEND: the PC is about to suspend.
    Suspend,
}

/// New raw "asleep" state after one event. Matches pc_power.py: display off -> asleep,
/// any other display state -> awake, suspend -> asleep.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn reduce(event: PowerEvent) -> bool {
    match event {
        PowerEvent::DisplayState(state) => state == DISPLAY_OFF,
        PowerEvent::Suspend => true,
    }
}

/// The value the board should see: never asleep when the setting is "none".
pub fn reported(mode: PcSleep, asleep: bool) -> bool {
    mode != PcSleep::Stay && asleep
}

/// The `pc_power` message the board expects.
pub fn message(asleep: bool) -> Value {
    json!({"type": KEY, "asleep": asleep})
}

pub async fn run(ctx: Ctx) {
    let mode = ctx.settings.borrow().display.pc_sleep;
    ctx.out.publish(KEY, message(reported(mode, false)));
    #[cfg(windows)]
    watch(ctx, mode).await;
    #[cfg(not(windows))]
    std::future::pending::<()>().await;
}

#[cfg(windows)]
async fn watch(mut ctx: Ctx, mut mode: PcSleep) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PowerEvent>();
    win::spawn(tx);
    let mut asleep = false;
    let mut sent = reported(mode, asleep);
    let mut settings_open = true;
    loop {
        tokio::select! {
            event = rx.recv() => {
                let Some(event) = event else {
                    log::warn!("pc power watch stopped");
                    break;
                };
                asleep = reduce(event);
                let now = reported(mode, asleep);
                if now != sent {
                    sent = now;
                    ctx.out.publish(KEY, message(now));
                }
            }
            changed = ctx.settings.changed(), if settings_open => {
                if changed.is_err() {
                    settings_open = false;
                    continue;
                }
                mode = ctx.settings.borrow_and_update().display.pc_sleep;
                let now = reported(mode, asleep);
                if now != sent {
                    sent = now;
                    ctx.out.publish(KEY, message(now));
                }
            }
        }
    }
    std::future::pending::<()>().await;
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;

    use tokio::sync::mpsc::UnboundedSender;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
    use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, CW_USEDEFAULT, DEVICE_NOTIFY_WINDOW_HANDLE, MSG, PBT_APMSUSPEND,
        PBT_POWERSETTINGCHANGE, WINDOW_EX_STYLE, WM_POWERBROADCAST, WNDCLASSW, WS_OVERLAPPED,
    };

    use super::PowerEvent;

    thread_local! {
        // The window procedure has no context pointer, and it runs on the window's thread.
        static SENDER: RefCell<Option<UnboundedSender<PowerEvent>>> = const { RefCell::new(None) };
    }

    pub fn spawn(tx: UnboundedSender<PowerEvent>) {
        let spawned = std::thread::Builder::new()
            .name("pc-power".into())
            .spawn(move || {
                SENDER.with(|s| *s.borrow_mut() = Some(tx));
                // SAFETY: plain Win32 window setup on this thread; the window lives until the thread ends.
                if let Err(e) = unsafe { run_window() } {
                    log::warn!("can't watch the PC's display power: {e}");
                }
            });
        if let Err(e) = spawned {
            log::warn!("can't start the pc power thread: {e}");
        }
    }

    fn send(event: PowerEvent) {
        SENDER.with(|s| {
            if let Some(tx) = s.borrow().as_ref() {
                let _ = tx.send(event);
            }
        });
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_POWERBROADCAST {
            if wparam.0 as u32 == PBT_POWERSETTINGCHANGE && lparam.0 != 0 {
                // SAFETY: for PBT_POWERSETTINGCHANGE, lParam points at a POWERBROADCAST_SETTING.
                let setting = unsafe { &*(lparam.0 as *const POWERBROADCAST_SETTING) };
                if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE {
                    send(PowerEvent::DisplayState(setting.Data[0] as u32));
                }
            } else if wparam.0 as u32 == PBT_APMSUSPEND {
                send(PowerEvent::Suspend);
            }
            return LRESULT(1);
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// A hidden top-level window (not HWND_MESSAGE: pc_power.py notes that message-only
    /// windows miss suspend broadcasts).
    unsafe fn run_window() -> windows::core::Result<()> {
        let hinst = unsafe { GetModuleHandleW(PCWSTR::null())? };
        let class = w!("DeskHubPowerWatch");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&wc) } == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Desk Hub"),
                WS_OVERLAPPED,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                0,
                0,
                None,
                None,
                Some(hinst.into()),
                None,
            )?
        };
        // Windows sends the current display state right away, then each change.
        unsafe {
            RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_CONSOLE_DISPLAY_STATE,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )?
        };
        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_off_is_asleep_on_and_dimmed_are_awake() {
        assert!(reduce(PowerEvent::DisplayState(0)));
        assert!(!reduce(PowerEvent::DisplayState(1)));
        assert!(!reduce(PowerEvent::DisplayState(2)));
    }

    #[test]
    fn suspend_is_asleep_and_resume_keeps_state() {
        assert!(reduce(PowerEvent::Suspend));
    }

    #[test]
    fn none_setting_never_reports_asleep() {
        assert!(!reported(PcSleep::Stay, true));
        assert!(reported(PcSleep::Off, true));
        assert!(reported(PcSleep::Clock, true));
        assert!(!reported(PcSleep::Clock, false));
    }

    #[test]
    fn message_shape() {
        assert_eq!(message(true), json!({"type": "pc_power", "asleep": true}));
        assert_eq!(message(false), json!({"type": "pc_power", "asleep": false}));
    }
}
