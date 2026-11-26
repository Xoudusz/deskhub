//! Shortcut tiles: "tiles" + "tile_state" messages, board tile presses -> actions.
//! Port of companion/deskhub/shortcuts.py and audio_state.py.
//!
//! Parsing, planning and the messages are platform-independent and unit-tested; only
//! `sys` (SendInput, ShellExecute, Core Audio) is Windows-specific. Elsewhere the actions
//! are logged no-ops and the mute states read as off.
#![cfg_attr(not(windows), allow(dead_code))]

use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use super::media::ascii_fold;
use super::Ctx;
use crate::protocol::Incoming;
use crate::settings::{Tile, MAX_TILES, TILE_ACTIONS};

/// Seconds between mute-state polls.
const POLL: Duration = Duration::from_secs(1);
/// Windows applies a volume-mute key a moment after the key press.
const SETTLE: Duration = Duration::from_millis(150);

// Virtual-key codes (winuser.h), kept here so the parser builds everywhere.
const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;
const VK_PAUSE: u16 = 0x13;
const VK_CAPITAL: u16 = 0x14;
const VK_ESCAPE: u16 = 0x1B;
const VK_SPACE: u16 = 0x20;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_SNAPSHOT: u16 = 0x2C;
const VK_INSERT: u16 = 0x2D;
const VK_DELETE: u16 = 0x2E;
const VK_LWIN: u16 = 0x5B;
const VK_APPS: u16 = 0x5D;
const VK_F1: u16 = 0x70;
const VK_NUMLOCK: u16 = 0x90;
const VK_SCROLL: u16 = 0x91;
const VK_VOLUME_MUTE: u16 = 0xAD;
const VK_VOLUME_DOWN: u16 = 0xAE;
const VK_VOLUME_UP: u16 = 0xAF;
const VK_MEDIA_NEXT_TRACK: u16 = 0xB0;
const VK_MEDIA_PREV_TRACK: u16 = 0xB1;
const VK_MEDIA_PLAY_PAUSE: u16 = 0xB3;

/// What a tile does once its target is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Press these keys in order, release in reverse.
    Keys(Vec<u16>),
    Run(String),
    Open(String),
    /// A single media/volume key tap.
    Tap(u16),
    Lock,
    Mic,
}

impl Action {
    /// Whether the action can change a mute state the board shows.
    fn touches_audio(&self) -> bool {
        matches!(self, Action::Mic | Action::Tap(VK_VOLUME_MUTE))
    }
}

/// Mute state of the default speaker and microphone (audio_state.py read_states).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioStates {
    pub mic_mute: bool,
    pub speaker_mute: bool,
}

impl AudioStates {
    fn is_on(&self, state: &str) -> bool {
        match state {
            "mic_mute" => self.mic_mute,
            "speaker_mute" => self.speaker_mute,
            _ => false,
        }
    }
}

/// Alias -> canonical key name (shortcuts.py _KEY_ALIASES; pynput's "cmd" is the Win key).
fn alias(part: &str) -> &str {
    match part {
        "win" | "windows" | "super" => "cmd",
        "control" => "ctrl",
        "escape" => "esc",
        "return" => "enter",
        "del" => "delete",
        "pgup" => "page_up",
        "pgdn" => "page_down",
        other => other,
    }
}

/// Virtual-key code of a named key, or None.
fn named_key(name: &str) -> Option<u16> {
    Some(match name {
        "ctrl" | "ctrl_l" | "ctrl_r" => VK_CONTROL,
        "shift" | "shift_l" | "shift_r" => VK_SHIFT,
        "alt" | "alt_l" | "alt_r" => VK_MENU,
        "cmd" | "cmd_l" | "cmd_r" => VK_LWIN,
        "esc" => VK_ESCAPE,
        "enter" => VK_RETURN,
        "tab" => VK_TAB,
        "space" => VK_SPACE,
        "backspace" => VK_BACK,
        "delete" => VK_DELETE,
        "insert" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "page_up" => VK_PRIOR,
        "page_down" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "caps_lock" => VK_CAPITAL,
        "num_lock" => VK_NUMLOCK,
        "scroll_lock" => VK_SCROLL,
        "pause" => VK_PAUSE,
        "print_screen" => VK_SNAPSHOT,
        "menu" => VK_APPS,
        "media_play_pause" => VK_MEDIA_PLAY_PAUSE,
        "media_next" => VK_MEDIA_NEXT_TRACK,
        "media_previous" => VK_MEDIA_PREV_TRACK,
        "media_volume_up" => VK_VOLUME_UP,
        "media_volume_down" => VK_VOLUME_DOWN,
        "media_volume_mute" => VK_VOLUME_MUTE,
        _ => {
            let n: u16 = name.strip_prefix('f')?.parse().ok()?;
            if (1..=24).contains(&n) {
                VK_F1 + n - 1
            } else {
                return None;
            }
        }
    })
}

/// Virtual-key code of a single character (US layout for punctuation).
fn char_key(c: char) -> Option<u16> {
    Some(match c {
        'a'..='z' => 0x41 + (c as u16 - 'a' as u16),
        '0'..='9' => 0x30 + (c as u16 - '0' as u16),
        ';' => 0xBA,
        '=' => 0xBB,
        ',' => 0xBC,
        '-' => 0xBD,
        '.' => 0xBE,
        '/' => 0xBF,
        '`' => 0xC0,
        '[' => 0xDB,
        '\\' => 0xDC,
        ']' => 0xDD,
        '\'' => 0xDE,
        _ => return None,
    })
}

/// The virtual keys in "ctrl+shift+m"; Err names the key it doesn't know
/// (shortcuts.py parse_combo).
pub fn parse_combo(combo: &str) -> Result<Vec<u16>, String> {
    let squeezed: String = combo.to_lowercase().chars().filter(|c| *c != ' ').collect();
    let mut keys = Vec::new();
    for part in squeezed.split('+') {
        let part = alias(part);
        let mut chars = part.chars();
        let vk = match (chars.next(), chars.next()) {
            (Some(c), None) => char_key(c),
            (Some(_), Some(_)) => named_key(part),
            (None, _) => None,
        };
        match vk {
            Some(vk) => keys.push(vk),
            None => return Err(format!("unknown key {part:?} in {combo:?}")),
        }
    }
    Ok(keys)
}

/// Replace a leading `~` with the home directory and `%VAR%` with its value.
fn expand_path(target: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut s = target.to_string();
    if s == "~" || s.starts_with("~/") || s.starts_with("~\\") {
        if let Some(home) = lookup("USERPROFILE") {
            s = format!("{home}{}", &s[1..]);
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after
            .find('%')
            .and_then(|end| lookup(&after[..end]).map(|v| (end, v)))
        {
            Some((end, v)) => {
                out.push_str(&v);
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What pressing this tile does, or why it can't (bad combo, unknown action or target).
pub fn plan(tile: &Tile, lookup: impl Fn(&str) -> Option<String>) -> Result<Action, String> {
    let target = tile.target.as_str();
    match tile.action.as_str() {
        "keys" => parse_combo(target).map(Action::Keys),
        "run" if !target.trim().is_empty() => Ok(Action::Run(target.to_string())),
        "open" if !target.trim().is_empty() => Ok(Action::Open(expand_path(target, lookup))),
        "run" | "open" => Err(format!("{} tile needs a target", tile.action)),
        "media" => match target {
            "toggle" => Ok(Action::Tap(VK_MEDIA_PLAY_PAUSE)),
            "next" => Ok(Action::Tap(VK_MEDIA_NEXT_TRACK)),
            "prev" => Ok(Action::Tap(VK_MEDIA_PREV_TRACK)),
            _ => Err(format!("unknown media target {target:?}")),
        },
        "volume" => match target {
            "up" => Ok(Action::Tap(VK_VOLUME_UP)),
            "down" => Ok(Action::Tap(VK_VOLUME_DOWN)),
            "mute" => Ok(Action::Tap(VK_VOLUME_MUTE)),
            _ => Err(format!("unknown volume target {target:?}")),
        },
        "lock" => Ok(Action::Lock),
        "mic" => Ok(Action::Mic),
        other => Err(format!(
            "unknown action {other:?} (known: {})",
            TILE_ACTIONS.join(", ")
        )),
    }
}

/// A board press: None when the id is out of range (ignored), else the plan for that tile.
pub fn dispatch(tiles: &[Tile], id: i64) -> Option<Result<Action, String>> {
    let tile = usize::try_from(id)
        .ok()
        .filter(|i| *i < MAX_TILES)
        .and_then(|i| tiles.get(i))?;
    Some(plan(tile, |k| std::env::var(k).ok()))
}

/// The hub view: {"type":"tiles","tiles":[{label,icon,color}]} (shortcuts.py hub_view).
pub fn hub_view(tiles: &[Tile]) -> Value {
    let list: Vec<Value> = tiles
        .iter()
        .take(MAX_TILES)
        .map(|t| json!({"label": ascii_fold(&t.label, 30), "icon": t.icon, "color": t.color}))
        .collect();
    json!({"type": "tiles", "tiles": list})
}

/// Indices of the tiles whose Windows state is on.
pub fn lit_indices(tiles: &[Tile], states: AudioStates) -> Vec<usize> {
    tiles
        .iter()
        .take(MAX_TILES)
        .enumerate()
        .filter(|(_, t)| states.is_on(t.effective_state()))
        .map(|(i, _)| i)
        .collect()
}

/// {"type":"tile_state","on":[index, ...]} (shortcuts.py lit_message).
pub fn tile_state_message(on: &[usize]) -> Value {
    json!({"type": "tile_state", "on": on})
}

pub async fn run(mut ctx: Ctx) {
    let mut tiles = ctx.settings.borrow().tiles.clone();
    let mut last: Option<Vec<usize>> = None;
    ctx.out.publish("tiles", hub_view(&tiles));
    refresh(&ctx, &tiles, &mut last).await;
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            changed = ctx.settings.changed() => {
                if changed.is_err() {
                    return;
                }
                let new = ctx.settings.borrow_and_update().tiles.clone();
                if new != tiles {
                    tiles = new;
                    ctx.out.publish("tiles", hub_view(&tiles));
                    last = None;
                    refresh(&ctx, &tiles, &mut last).await;
                }
            }
            _ = tick.tick() => refresh(&ctx, &tiles, &mut last).await,
            msg = ctx.incoming.recv() => match msg {
                Ok(Incoming::Shortcut { id }) => {
                    if press(&tiles, id).await {
                        refresh(&ctx, &tiles, &mut last).await;
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
        }
    }
}

/// Re-read the mute states and publish tile_state when the lit set changed.
async fn refresh(ctx: &Ctx, tiles: &[Tile], last: &mut Option<Vec<usize>>) {
    let watched = tiles.iter().any(|t| !t.effective_state().is_empty());
    let states = if watched {
        tokio::task::spawn_blocking(sys::read_states)
            .await
            .unwrap_or_default()
    } else {
        AudioStates::default()
    };
    let on = lit_indices(tiles, states);
    if last.as_ref() != Some(&on) {
        ctx.out.publish("tile_state", tile_state_message(&on));
        *last = Some(on);
    }
}

/// Run the pressed tile. True when a mute state may have changed.
async fn press(tiles: &[Tile], id: i64) -> bool {
    let action = match dispatch(tiles, id) {
        None => {
            log::debug!("shortcut {id}: no such tile");
            return false;
        }
        Some(Err(e)) => {
            log::warn!("shortcut {id} failed: {e}");
            return false;
        }
        Some(Ok(a)) => a,
    };
    log::info!("shortcut {id}: {action:?}");
    let audio = action.touches_audio();
    if let Err(e) = tokio::task::spawn_blocking(move || sys::perform(&action)).await {
        log::warn!("shortcut {id} failed: {e}");
        return false;
    }
    if audio {
        tokio::time::sleep(SETTLE).await;
    }
    audio
}

#[cfg(windows)]
mod sys {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, eRender, EDataFlow, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Shutdown::LockWorkStation;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::*;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn perform(action: &Action) {
        match action {
            Action::Keys(keys) => send_keys(keys),
            Action::Tap(vk) => send_keys(&[*vk]),
            Action::Run(cmd) => run(cmd),
            Action::Open(path) => open(path),
            Action::Lock => {
                if let Err(e) = unsafe { LockWorkStation() } {
                    log::warn!("lock failed: {e}");
                }
            }
            Action::Mic => match with_com(toggle_mic) {
                Some(Ok(())) => {}
                Some(Err(e)) => log::warn!("mic toggle failed: {e}"),
                None => log::warn!("mic toggle failed: COM unavailable"),
            },
        }
    }

    /// Down in order, up in reverse, as one SendInput batch.
    fn send_keys(keys: &[u16]) {
        let key = |vk: u16, up: bool| {
            let mut flags = KEYBD_EVENT_FLAGS(0);
            if is_extended(vk) {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if up {
                flags |= KEYEVENTF_KEYUP;
            }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(vk),
                        wScan: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        };
        let inputs: Vec<INPUT> = keys
            .iter()
            .map(|k| key(*k, false))
            .chain(keys.iter().rev().map(|k| key(*k, true)))
            .collect();
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            log::warn!("SendInput sent {sent} of {} events", inputs.len());
        }
    }

    /// Keys the keyboard driver reports with the extended flag.
    fn is_extended(vk: u16) -> bool {
        matches!(
            vk,
            VK_PRIOR
                | VK_NEXT
                | VK_END
                | VK_HOME
                | VK_LEFT
                | VK_UP
                | VK_RIGHT
                | VK_DOWN
                | VK_INSERT
                | VK_DELETE
                | VK_LWIN
                | VK_APPS
                | VK_SNAPSHOT
                // Volume and media keys (0xAD-0xB3) send E0-prefixed scancodes too.
                | 0xAD..=0xB3
        )
    }

    /// `cmd /c start "" <command line>`: no console window, not tied to this process.
    fn run(cmd: &str) {
        let spawned = Command::new("cmd")
            .arg("/c")
            .raw_arg(format!("start \"\" {cmd}"))
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .spawn();
        if let Err(e) = spawned {
            log::warn!("run {cmd:?} failed: {e}");
        }
    }

    fn open(path: &str) {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        let h = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecute reports success with a value above 32.
        if (h.0 as isize) <= 32 {
            log::warn!("open {path:?} failed (code {})", h.0 as isize);
        }
    }

    /// Run `f` on this thread with COM initialised (audio_state.py com_thread).
    fn with_com<T>(f: impl FnOnce() -> T) -> Option<T> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let owned = hr.is_ok();
            if !owned && hr != RPC_E_CHANGED_MODE {
                return None;
            }
            let out = f();
            if owned {
                CoUninitialize();
            }
            Some(out)
        }
    }

    /// IAudioEndpointVolume of the default device; Err when there is none (no microphone).
    fn endpoint(flow: EDataFlow) -> windows::core::Result<IAudioEndpointVolume> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(flow, eConsole)?;
            device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
        }
    }

    fn muted(flow: EDataFlow) -> Option<bool> {
        let vol = endpoint(flow).ok()?;
        unsafe { vol.GetMute().ok().map(|b| b.as_bool()) }
    }

    fn toggle_mic() -> windows::core::Result<()> {
        let vol = endpoint(eCapture)?;
        unsafe {
            let now = vol.GetMute()?.as_bool();
            vol.SetMute(!now, std::ptr::null())
        }
    }

    pub fn read_states() -> AudioStates {
        with_com(|| AudioStates {
            mic_mute: muted(eCapture).unwrap_or(false),
            speaker_mute: muted(eRender).unwrap_or(false),
        })
        .unwrap_or_default()
    }
}

#[cfg(not(windows))]
mod sys {
    use super::*;

    pub fn perform(action: &Action) {
        log::debug!("shortcut {action:?}: not supported on this platform");
    }

    pub fn read_states() -> AudioStates {
        AudioStates::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(action: &str, target: &str) -> Tile {
        Tile {
            action: action.into(),
            target: target.into(),
            ..Tile::default()
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn combos_parse() {
        assert_eq!(
            parse_combo("ctrl+shift+m"),
            Ok(vec![VK_CONTROL, VK_SHIFT, 0x4D])
        );
        assert_eq!(
            parse_combo("Win + Shift + S"),
            Ok(vec![VK_LWIN, VK_SHIFT, 0x53])
        );
        assert_eq!(parse_combo("f13"), Ok(vec![VK_F1 + 12]));
        assert_eq!(
            parse_combo("control+escape"),
            Ok(vec![VK_CONTROL, VK_ESCAPE])
        );
        assert_eq!(parse_combo("alt+pgdn"), Ok(vec![VK_MENU, VK_NEXT]));
        assert_eq!(parse_combo("ctrl+del"), Ok(vec![VK_CONTROL, VK_DELETE]));
        assert_eq!(parse_combo("ctrl+5"), Ok(vec![VK_CONTROL, 0x35]));
    }

    #[test]
    fn bad_combos_name_the_key() {
        assert!(parse_combo("ctrl+bogus").unwrap_err().contains("bogus"));
        assert!(parse_combo("").is_err());
        assert!(parse_combo("ctrl+").is_err());
        assert!(parse_combo("+m").is_err());
        assert!(parse_combo("f25").is_err());
        assert!(parse_combo("f0").is_err());
        assert!(parse_combo("ctrl+\u{e9}").is_err());
    }

    #[test]
    fn plan_per_action() {
        assert_eq!(
            plan(&tile("keys", "ctrl+m"), no_env),
            Ok(Action::Keys(vec![VK_CONTROL, 0x4D]))
        );
        assert!(plan(&tile("keys", "ctrl+nope"), no_env).is_err());
        assert_eq!(
            plan(&tile("run", "wt.exe"), no_env),
            Ok(Action::Run("wt.exe".into()))
        );
        assert!(plan(&tile("run", " "), no_env).is_err());
        assert_eq!(
            plan(&tile("media", "toggle"), no_env),
            Ok(Action::Tap(VK_MEDIA_PLAY_PAUSE))
        );
        assert_eq!(
            plan(&tile("media", "next"), no_env),
            Ok(Action::Tap(VK_MEDIA_NEXT_TRACK))
        );
        assert_eq!(
            plan(&tile("media", "prev"), no_env),
            Ok(Action::Tap(VK_MEDIA_PREV_TRACK))
        );
        assert!(plan(&tile("media", "seek"), no_env).is_err());
        assert_eq!(
            plan(&tile("volume", "up"), no_env),
            Ok(Action::Tap(VK_VOLUME_UP))
        );
        assert_eq!(
            plan(&tile("volume", "down"), no_env),
            Ok(Action::Tap(VK_VOLUME_DOWN))
        );
        assert_eq!(
            plan(&tile("volume", "mute"), no_env),
            Ok(Action::Tap(VK_VOLUME_MUTE))
        );
        assert!(plan(&tile("volume", "loud"), no_env).is_err());
        assert_eq!(plan(&tile("lock", ""), no_env), Ok(Action::Lock));
        assert_eq!(plan(&tile("mic", ""), no_env), Ok(Action::Mic));
        assert!(plan(&tile("teleport", ""), no_env)
            .unwrap_err()
            .contains("teleport"));
    }

    #[test]
    fn open_expands_home_and_env() {
        let env = |k: &str| match k {
            "USERPROFILE" => Some("C:\\Users\\me".to_string()),
            "APPDATA" => Some("C:\\Roaming".to_string()),
            _ => None,
        };
        assert_eq!(
            plan(&tile("open", "~/Downloads"), env),
            Ok(Action::Open("C:\\Users\\me/Downloads".into()))
        );
        assert_eq!(
            plan(&tile("open", "%APPDATA%\\x"), env),
            Ok(Action::Open("C:\\Roaming\\x".into()))
        );
        assert_eq!(
            plan(&tile("open", "https://a.b/~x"), env),
            Ok(Action::Open("https://a.b/~x".into()))
        );
        assert_eq!(
            plan(&tile("open", "50%off"), env),
            Ok(Action::Open("50%off".into()))
        );
        assert_eq!(
            plan(&tile("open", "~"), no_env),
            Ok(Action::Open("~".into()))
        );
    }

    #[test]
    fn effective_state_picks_lit_tiles() {
        let mut mic = tile("mic", "");
        mic.icon = "mic_off".into();
        let mute = tile("volume", "mute");
        let plain = tile("lock", "");
        let mut off = tile("mic", "");
        off.state = Some(String::new());
        let tiles = vec![plain, mic, mute, off];
        let both = AudioStates {
            mic_mute: true,
            speaker_mute: true,
        };
        assert_eq!(lit_indices(&tiles, both), vec![1, 2]);
        let mic_only = AudioStates {
            mic_mute: true,
            speaker_mute: false,
        };
        assert_eq!(lit_indices(&tiles, mic_only), vec![1]);
        assert!(lit_indices(&tiles, AudioStates::default()).is_empty());
        assert_eq!(
            tile_state_message(&[1, 2]),
            json!({"type": "tile_state", "on": [1, 2]})
        );
        assert_eq!(
            tile_state_message(&[]),
            json!({"type": "tile_state", "on": []})
        );
    }

    #[test]
    fn hub_view_folds_and_limits() {
        let mut t = tile("lock", "");
        t.label = format!("Caf\u{e9} \u{2014} {}", "x".repeat(40));
        t.icon = "lock".into();
        t.color = "#112233".into();
        let v = hub_view(std::slice::from_ref(&t));
        assert_eq!(v["type"], "tiles");
        let label = v["tiles"][0]["label"].as_str().unwrap();
        assert_eq!(label.len(), 30); // bytes: what the board's buffer holds
        assert!(label.starts_with("Caf\u{e9} \u{2014} xxx"));
        assert_eq!(v["tiles"][0]["icon"], "lock");
        assert_eq!(v["tiles"][0]["color"], "#112233");
        assert_eq!(v["tiles"][0].as_object().unwrap().len(), 3);

        let many = vec![t; MAX_TILES + 4];
        assert_eq!(
            hub_view(&many)["tiles"].as_array().unwrap().len(),
            MAX_TILES
        );
        assert_eq!(hub_view(&[])["tiles"], json!([]));
    }

    #[test]
    fn press_out_of_range_is_ignored() {
        let tiles = vec![tile("lock", ""), tile("keys", "bad+bad")];
        assert_eq!(dispatch(&tiles, 0), Some(Ok(Action::Lock)));
        assert!(matches!(dispatch(&tiles, 1), Some(Err(_))));
        assert_eq!(dispatch(&tiles, 2), None);
        assert_eq!(dispatch(&tiles, -1), None);
        assert_eq!(dispatch(&tiles, i64::MAX), None);
        assert_eq!(dispatch(&[], 0), None);
    }

    #[test]
    fn audio_actions_trigger_refresh() {
        assert!(Action::Mic.touches_audio());
        assert!(Action::Tap(VK_VOLUME_MUTE).touches_audio());
        assert!(!Action::Tap(VK_VOLUME_UP).touches_audio());
        assert!(!Action::Lock.touches_audio());
    }
}
