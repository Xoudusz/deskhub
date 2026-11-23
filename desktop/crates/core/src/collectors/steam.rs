//! Steam + custom/preset games: "steam" message, STM1 art, launch.
//! Port of companion/deskhub/steam.py (without the opt-in Web API friends), custom_games.py
//! and game_presets.py.
//!
//! Only local files and the registry are read. Cover art comes from Steam's own library
//! cache, then from the public CDN; names of games that are no longer installed come from
//! the public Store API. Both are looked up by appid alone and cached under ~/.deskhub.
//!
//! The "steam" JSON is published first and the STM1 art frames after it, one latest-frame
//! key per art slot (header, thumb, portrait 0-4). The board gives a portrait a slot by
//! checking the games of the last "steam" message, so the JSON has to come first.
//!
//! Deviations from Python: no Web API friends/playtime (needs the credential store, M5),
//! `friends` is always `[]`. https downloads (CDN, Store API, image URLs, the League
//! client) work on Windows only; the Linux build has no TLS.

pub mod art;
pub mod games;
pub mod vdf;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tokio::sync::{broadcast, watch};

use self::art::{KIND_HEADER, KIND_PORTRAIT, KIND_THUMB};
use self::games::{
    crc32, expand_path, is_custom, league_mode_from_session, load_games, plan_launch, Game,
    LaunchAction, Playing, Probe, ProcInfo, Recent, Tracker, WinInfo,
};
use self::vdf::parse_vdf;
use super::stats::ascii_fold;
use super::Ctx;
use crate::protocol::Incoming;
use crate::settings::Games;

/// Publish key of the "steam" message.
const KEY: &str = "steam";
const TICK: Duration = Duration::from_secs(2);
const KEEPALIVE_S: u64 = 60;
const CACHE_S: f64 = 60.0;
const RECENT_MAX: usize = 5;
const RETRY_ART: Duration = Duration::from_secs(300);
const CDN: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
const STORE_API: &str = "https://store.steampowered.com/api/appdetails";
const SKIP_IDS: [u32; 1] = [228_980];
const SKIP_WORDS: [&str; 4] = [
    "redistributable",
    "proton",
    "steam linux runtime",
    "steamworks",
];
/// Names looked up per tick, so a first run with many unknown games stays responsive.
const NAMES_PER_TICK: usize = 3;
/// https works only where reqwest has TLS (Windows, schannel).
const HTTPS: bool = cfg!(windows);

// ---- paths -------------------------------------------------------------------

fn deskhub_dir() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }?;
    Some(PathBuf::from(home).join(".deskhub"))
}

fn art_dir() -> Option<PathBuf> {
    deskhub_dir().map(|d| d.join("steam_art"))
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ---- Steam's local files ------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AppInfo {
    pub last: i64,
    pub total_min: i64,
}

#[derive(Debug, Default)]
pub struct SteamLocal {
    pub installed: HashMap<u32, String>,
    pub apps: HashMap<u32, AppInfo>,
    /// Steam account of the newest userdata folder.
    pub account_id: u32,
}

fn read_text(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// The Steam folder plus the libraries named in libraryfolders.vdf, without repeats.
pub fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![root.to_path_buf()];
    if let Some(text) = read_text(&root.join("steamapps").join("libraryfolders.vdf")) {
        let vdf = parse_vdf(&text);
        if let Some(folders) = vdf.get("libraryfolders") {
            for (_, entry) in folders.entries() {
                match entry {
                    vdf::Vdf::Map(_) => {
                        let p = entry.str_of("path");
                        if !p.is_empty() {
                            libs.push(PathBuf::from(p));
                        }
                    }
                    vdf::Vdf::Str(s) if !s.trim().is_empty() => libs.push(PathBuf::from(s)),
                    vdf::Vdf::Str(_) => {}
                }
            }
        }
    }
    let mut seen = HashSet::new();
    libs.retain(|p| seen.insert(path_key(p)));
    libs
}

/// Installed games, last played / playtime of the newest user, and that user's id.
pub fn load_local(root: &Path) -> SteamLocal {
    let mut local = SteamLocal::default();
    for lib in libraries(root) {
        let Ok(dir) = std::fs::read_dir(lib.join("steamapps")) else {
            continue;
        };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Some(text) = read_text(&entry.path()) else {
                continue;
            };
            let vdf = parse_vdf(&text);
            let Some(state) = vdf.get("AppState") else {
                continue;
            };
            let appid = state.int_of("appid", 0);
            let game = state.str_of("name");
            if let (Ok(appid), false) = (u32::try_from(appid), game.is_empty()) {
                if appid != 0 {
                    local.installed.insert(appid, game.to_string());
                }
            }
        }
    }

    let mut configs: Vec<(SystemTime, PathBuf)> = Vec::new();
    if let Ok(dir) = std::fs::read_dir(root.join("userdata")) {
        for entry in dir.flatten() {
            let p = entry.path().join("config").join("localconfig.vdf");
            if let Ok(mtime) = p.metadata().and_then(|m| m.modified()) {
                configs.push((mtime, p));
            }
        }
    }
    if let Some((_, newest)) = configs.into_iter().max_by_key(|(t, _)| *t) {
        let folder = newest
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        local.account_id = folder.parse().unwrap_or(0);
        if let Some(text) = read_text(&newest) {
            let cfg = parse_vdf(&text);
            let apps = cfg.path(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]);
            for (k, v) in apps.map_or(&[][..], |a| a.entries()) {
                if let (Ok(id), vdf::Vdf::Map(_)) = (k.parse::<u32>(), v) {
                    local.apps.insert(
                        id,
                        AppInfo {
                            last: v.int_of("LastPlayed", 0),
                            total_min: v.int_of("Playtime", 0),
                        },
                    );
                }
            }
        }
    }
    local
}

/// Steam's own cached cover: old `librarycache/<appid>_header.jpg` names, then the newer
/// `librarycache/<appid>/header.jpg` and `librarycache/<appid>/<hash>/header.jpg`.
pub fn find_local_art(root: &Path, appid: u32, portrait: bool) -> Option<PathBuf> {
    let name = if portrait {
        "library_600x900"
    } else {
        "header"
    };
    let lib = root.join("appcache").join("librarycache");
    let dir = lib.join(appid.to_string());
    let mut candidates = Vec::new();
    for ext in ["jpg", "png"] {
        candidates.push(lib.join(format!("{appid}_{name}.{ext}")));
        candidates.push(dir.join(format!("{name}.{ext}")));
    }
    if let Some(hit) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(hit);
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subs.sort();
    for sub in subs {
        for ext in ["jpg", "png"] {
            let p = sub.join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

// ---- registry ----------------------------------------------------------------

/// (SteamPath, RunningAppID) from HKCU; (None, 0) when Steam isn't installed.
#[cfg(windows)]
fn read_registry() -> (Option<PathBuf>, u32) {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };

    let key = w!("Software\\Valve\\Steam");
    let value = |name: PCWSTR| -> Option<String> {
        let mut len = 0u32;
        // SAFETY: size query with a live out-pointer; no data buffer.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key,
                name,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut len),
            )
        };
        if r != ERROR_SUCCESS || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
        let mut got = (buf.len() * 2) as u32;
        // SAFETY: `buf` is live and `got` is its size in bytes.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key,
                name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut got),
            )
        };
        if r != ERROR_SUCCESS {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    };
    let path = value(w!("SteamPath"))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let mut running = 0u32;
    let mut size = 4u32;
    // SAFETY: a DWORD value is read into a live u32 with its size in bytes.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            w!("RunningAppID"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut running as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS {
        running = 0;
    }
    (path, running)
}

#[cfg(not(windows))]
fn read_registry() -> (Option<PathBuf>, u32) {
    (None, 0)
}

// ---- names -------------------------------------------------------------------

/// appid -> name for games that are no longer installed, persisted in
/// ~/.deskhub/steam_names.json (same file as Python).
#[derive(Default)]
pub struct Names {
    known: HashMap<u32, String>,
    failed: HashSet<u32>,
    missing: Vec<u32>,
    file: Option<PathBuf>,
}

impl Names {
    pub fn load(file: Option<PathBuf>) -> Self {
        let mut known = HashMap::new();
        if let Some(Ok(Value::Object(map))) = file
            .as_deref()
            .and_then(read_text)
            .map(|t| serde_json::from_str::<Value>(&t))
        {
            for (k, v) in map {
                if let (Ok(id), Some(name)) = (k.parse::<u32>(), v.as_str()) {
                    known.insert(id, name.to_string());
                }
            }
        }
        Names {
            known,
            failed: HashSet::new(),
            missing: Vec::new(),
            file,
        }
    }

    /// Installed name, else a looked-up one, else "App <id>" (queued for a lookup).
    pub fn name(&mut self, appid: u32, installed: &HashMap<u32, String>) -> String {
        if let Some(n) = installed.get(&appid).or_else(|| self.known.get(&appid)) {
            return n.clone();
        }
        if !self.failed.contains(&appid) && !self.missing.contains(&appid) {
            self.missing.push(appid);
        }
        format!("App {appid}")
    }

    pub fn take_missing(&mut self) -> Vec<u32> {
        let n = self.missing.len().min(NAMES_PER_TICK);
        self.missing.drain(..n).collect()
    }

    pub fn has_missing(&self) -> bool {
        !self.missing.is_empty()
    }

    /// Record a lookup result; a failure is remembered for this run only.
    pub fn resolve(&mut self, appid: u32, name: Option<String>) {
        match name {
            Some(n) => {
                self.known.insert(appid, n);
                if let Some(file) = &self.file {
                    let map: serde_json::Map<String, Value> = self
                        .known
                        .iter()
                        .map(|(k, v)| (k.to_string(), json!(v)))
                        .collect();
                    if let Some(dir) = file.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    if let Err(e) = std::fs::write(file, Value::Object(map).to_string()) {
                        log::debug!("could not save names: {e}");
                    }
                }
            }
            None => {
                self.failed.insert(appid);
            }
        }
    }
}

/// Tools and runtimes that show up in "last played" but aren't games.
pub fn is_game(appid: u32, name: &str) -> bool {
    let low = name.to_lowercase();
    !SKIP_IDS.contains(&appid) && !SKIP_WORDS.iter().any(|w| low.contains(w))
}

// ---- snapshot ----------------------------------------------------------------

pub struct Steam {
    local: SteamLocal,
    local_at: f64,
    run_appid: u32,
    run_start: f64,
}

impl Steam {
    pub fn new() -> Self {
        Steam {
            local: SteamLocal::default(),
            local_at: -1e9,
            run_appid: 0,
            run_start: 0.0,
        }
    }

    fn refresh(&mut self, root: &Path, now: f64) {
        if now - self.local_at < CACHE_S {
            return;
        }
        self.local_at = now;
        self.local = load_local(root);
    }

    /// The Steam game running and the recent ones (steam.py `_steam_part`).
    pub fn part(
        &mut self,
        root: &Path,
        running: u32,
        now: f64,
        names: &mut Names,
    ) -> (Option<Playing>, Vec<Recent>) {
        self.refresh(root, now);
        if running != self.run_appid {
            self.run_appid = running;
            self.run_start = now;
        }
        let mut playing = None;
        if running != 0 {
            let name = names.name(running, &self.local.installed);
            if is_game(running, &name) {
                let info = self.local.apps.get(&running).copied().unwrap_or_default();
                playing = Some(Playing {
                    appid: running,
                    name: ascii_fold(&name, 40),
                    session_s: ((now - self.run_start) as i64).max(0),
                    total_min: info.total_min,
                });
            }
        } else {
            self.run_start = now;
        }

        let mut order: Vec<(&u32, &AppInfo)> = self.local.apps.iter().collect();
        order.sort_by(|a, b| b.1.last.cmp(&a.1.last).then(a.0.cmp(b.0)));
        let mut recent = Vec::new();
        for (&appid, info) in order {
            if recent.len() >= RECENT_MAX {
                break;
            }
            if info.last <= 0 || appid == running || SKIP_IDS.contains(&appid) {
                continue;
            }
            let name = names.name(appid, &self.local.installed);
            if !is_game(appid, &name) {
                continue;
            }
            recent.push(Recent {
                appid,
                name: ascii_fold(&name, 40),
                last: info.last,
                total_min: info.total_min,
            });
        }
        (playing, recent)
    }
}

impl Default for Steam {
    fn default() -> Self {
        Self::new()
    }
}

/// What the board shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub found: bool,
    pub playing: Option<Playing>,
    pub recent: Vec<Recent>,
}

/// Custom recent games joined to Steam's, newest first, at most 5; the game playing now
/// is left out.
pub fn merge_recent(
    mut steam: Vec<Recent>,
    custom: Vec<Recent>,
    playing: Option<u32>,
) -> Vec<Recent> {
    steam.extend(custom.into_iter().filter(|g| Some(g.appid) != playing));
    steam.sort_by_key(|g| std::cmp::Reverse(g.last));
    steam.truncate(RECENT_MAX);
    steam
}

/// Merge Steam and custom games (a running Steam game wins over a custom one).
pub fn combine(
    steam_ok: bool,
    has_custom: bool,
    steam: (Option<Playing>, Vec<Recent>),
    custom_now: Option<Playing>,
    custom_recent: Vec<Recent>,
) -> Snapshot {
    let (steam_playing, steam_recent) = steam;
    let playing = steam_playing.or(custom_now);
    let recent = merge_recent(
        steam_recent,
        custom_recent,
        playing.as_ref().map(|p| p.appid),
    );
    Snapshot {
        found: steam_ok || has_custom,
        playing,
        recent,
    }
}

impl Snapshot {
    pub fn to_value(&self) -> Value {
        let playing = self.playing.as_ref().map(|p| {
            json!({"appid": p.appid, "name": p.name, "session_s": p.session_s, "total_min": p.total_min})
        });
        let recent: Vec<Value> = self
            .recent
            .iter()
            .map(|g| json!({"appid": g.appid, "name": g.name, "last": g.last, "total_min": g.total_min}))
            .collect();
        json!({
            "type": "steam",
            "found": self.found,
            "playing": playing,
            "recent": recent,
            "friends": [],
        })
    }

    /// The message without the session clock, which the board counts itself.
    pub fn change_key(&self) -> Value {
        let mut v = self.to_value();
        if let Some(Value::Object(p)) = v.get_mut("playing") {
            p.remove("session_s");
        }
        v
    }
}

// ---- art wanted --------------------------------------------------------------

/// One cover to keep on the board: where it is published and what it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    pub slot: String,
    pub appid: u32,
    pub kind: u16,
}

/// Covers for a snapshot (steam.py `_wanted`): header and thumb of the game playing, or
/// the thumb of the latest game; a portrait for every recent game.
pub fn wanted_art(snap: &Snapshot) -> Vec<Want> {
    if !snap.found {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut want = |slot: String, appid: u32, kind: u16| out.push(Want { slot, appid, kind });
    if let Some(p) = &snap.playing {
        want("steam_art:header".into(), p.appid, KIND_HEADER);
        want("steam_art:thumb".into(), p.appid, KIND_THUMB);
    } else if let Some(g) = snap.recent.first() {
        want("steam_art:thumb".into(), g.appid, KIND_THUMB);
    }
    for (i, g) in snap.recent.iter().enumerate() {
        want(format!("steam_art:portrait:{i}"), g.appid, KIND_PORTRAIT);
    }
    out
}

// ---- processes and windows ----------------------------------------------------

/// (port, token) from the League client's command line.
pub fn league_args(cmd: &[String]) -> Option<(String, String)> {
    let mut port = String::new();
    let mut token = String::new();
    for arg in cmd {
        if let Some(v) = arg.strip_prefix("--app-port=") {
            port = v.to_string();
        } else if let Some(v) = arg.strip_prefix("--remoting-auth-token=") {
            token = v.to_string();
        }
    }
    (!port.is_empty() && !token.is_empty()).then_some((port, token))
}

/// Visible top-level windows as (process id, title).
#[cfg(windows)]
fn window_titles() -> Vec<(u32, String)> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the address of the Vec below, alive for the whole EnumWindows call.
        let out = unsafe { &mut *(lparam.0 as *mut Vec<(u32, String)>) };
        // SAFETY: plain window queries on the handle EnumWindows gave us.
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                let n = GetWindowTextLengthW(hwnd);
                if n > 0 {
                    let mut buf = vec![0u16; n as usize + 1];
                    let got = GetWindowTextW(hwnd, &mut buf);
                    if got > 0 {
                        let mut pid = 0u32;
                        GetWindowThreadProcessId(hwnd, Some(&mut pid));
                        out.push((pid, String::from_utf16_lossy(&buf[..got as usize])));
                    }
                }
            }
        }
        BOOL(1)
    }

    let mut out: Vec<(u32, String)> = Vec::new();
    // SAFETY: the callback only touches `out`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

#[cfg(not(windows))]
fn window_titles() -> Vec<(u32, String)> {
    Vec::new()
}

fn collect_probe(sys: &mut System, tracker: &Tracker) -> Probe {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::OnlyIfNotSet),
    );
    let wanted = tracker.wanted_procs();
    let want_league = tracker.wants_league();
    let mut probe = Probe::default();
    for p in sys.processes().values() {
        let name = p.name().to_string_lossy().to_lowercase();
        if want_league && name == "leagueclientux.exe" && probe.league.is_none() {
            let cmd: Vec<String> = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            probe.league = league_args(&cmd);
        }
        if wanted.contains(&name) {
            let cmdline = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            probe.procs.push(ProcInfo { name, cmdline });
        }
    }
    if tracker.wants_titles() {
        for (pid, title) in window_titles() {
            let proc = sys
                .process(Pid::from_u32(pid))
                .map(|p| p.name().to_string_lossy().to_lowercase())
                .unwrap_or_default();
            probe.windows.push(WinInfo {
                title: title.to_lowercase(),
                proc,
            });
        }
    }
    probe
}

// ---- the service core (blocking part) ----------------------------------------

struct Core {
    cfg: Option<Games>,
    tracker: Tracker,
    steam: Steam,
    names: Names,
    sys: System,
    steam_root: Option<PathBuf>,
    steam_ok: bool,
    running: u32,
    found: Option<usize>,
    league: Option<(String, String)>,
    custom_now: Option<Playing>,
}

fn riot_client() -> String {
    let base = std::env::var("PROGRAMDATA").unwrap_or_else(|_| "C:/ProgramData".into());
    let file = Path::new(&base)
        .join("Riot Games")
        .join("RiotClientInstalls.json");
    let Some(Ok(v)) = read_text(&file).map(|t| serde_json::from_str::<Value>(&t)) else {
        return String::new();
    };
    ["rc_default", "rc_live"]
        .iter()
        .filter_map(|k| v.get(k).and_then(Value::as_str))
        .find(|s| !s.is_empty())
        .filter(|p| Path::new(p).is_file())
        .map(str::to_string)
        .unwrap_or_default()
}

impl Core {
    fn new() -> Self {
        let dir = deskhub_dir();
        Core {
            cfg: None,
            tracker: Tracker::new(
                Vec::new(),
                dir.as_ref().map(|d| d.join("custom_games.json")),
            ),
            steam: Steam::new(),
            names: Names::load(dir.map(|d| d.join("steam_names.json"))),
            sys: System::new(),
            steam_root: None,
            steam_ok: false,
            running: 0,
            found: None,
            league: None,
            custom_now: None,
        }
    }

    /// Rebuild the game list when the settings changed.
    fn configure(&mut self, cfg: &Games, now: f64) {
        if self.cfg.as_ref() == Some(cfg) {
            return;
        }
        let riot = riot_client();
        let resolve =
            |t: &str| games::resolve_launch(t, &riot, &|p| Path::new(&expand_path(p)).exists());
        let list = load_games(cfg, &resolve);
        self.tracker.set_games(list, now);
        self.cfg = Some(cfg.clone());
    }

    /// Look at the registry and the running programs. Returns true when the League mode
    /// needs asking (League client running, answer older than 30 s).
    fn poll(&mut self, now: f64) -> bool {
        let use_steam = self.cfg.as_ref().is_some_and(|c| c.steam);
        let (path, running) = read_registry();
        self.steam_ok = use_steam && path.as_deref().is_some_and(Path::is_dir);
        self.steam_root = path;
        self.running = running;
        let probe = collect_probe(&mut self.sys, &self.tracker);
        self.found = self.tracker.find(&probe);
        self.league = probe.league;
        self.found.is_some_and(|i| self.tracker.mode_wanted(i, now))
    }

    fn step(&mut self, now: f64) -> Snapshot {
        self.custom_now = self.tracker.apply(now, self.found);
        self.compose(now)
    }

    fn compose(&mut self, now: f64) -> Snapshot {
        let steam = match (&self.steam_root, self.steam_ok) {
            (Some(root), true) => self.steam.part(root, self.running, now, &mut self.names),
            _ => (None, Vec::new()),
        };
        combine(
            self.steam_ok,
            !self.tracker.games().is_empty(),
            steam,
            self.custom_now.clone(),
            self.tracker.recent(),
        )
    }

    fn art_request(&self, snap: &Snapshot) -> ArtRequest {
        ArtRequest {
            wanted: wanted_art(snap),
            root: self.steam_root.clone().filter(|_| self.steam_ok),
            images: self
                .tracker
                .games()
                .iter()
                .filter(|g| !g.image.is_empty())
                .map(|g| (g.id, g.image.clone()))
                .collect(),
        }
    }
}

// ---- http --------------------------------------------------------------------

fn http_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("deskhub")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| log::warn!("steam http client: {e}"))
        .ok()
}

fn https_blocked(url: &str) -> bool {
    if url.starts_with("https://") && !HTTPS {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log::info!("steam: https downloads need TLS, which this build lacks"));
        return true;
    }
    false
}

async fn get(client: &reqwest::Client, url: &str) -> Result<(u16, Vec<u8>), String> {
    if https_blocked(url) {
        return Err("no TLS".into());
    }
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = resp.status().as_u16();
    let body = resp
        .bytes()
        .await
        .map_err(|e| e.without_url().to_string())?;
    Ok((status, body.to_vec()))
}

async fn store_json(client: &reqwest::Client, appid: u32) -> Option<Value> {
    let url = format!("{STORE_API}?appids={appid}&filters=basic");
    match get(client, &url).await {
        Ok((200, body)) => serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get(appid.to_string())?.get("data").cloned()),
        Ok((status, _)) => {
            log::debug!("store api {appid}: HTTP {status}");
            None
        }
        Err(e) => {
            log::debug!("store api {appid}: {e}");
            None
        }
    }
}

async fn store_name(client: &reqwest::Client, appid: u32) -> Option<String> {
    store_json(client, appid)
        .await?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// Raw cover image: Steam's own cache, the ~/.deskhub cache, then the CDN.
async fn fetch_art(
    client: &reqwest::Client,
    root: Option<&Path>,
    appid: u32,
    portrait: bool,
) -> Option<Vec<u8>> {
    if let Some(bytes) = root
        .and_then(|r| find_local_art(r, appid, portrait))
        .and_then(|p| std::fs::read(p).ok())
    {
        return Some(bytes);
    }
    let stem = format!("{appid}_{}", if portrait { "portrait" } else { "header" });
    let dir = art_dir();
    let jpg = dir.as_ref().map(|d| d.join(format!("{stem}.jpg")));
    let none = dir.as_ref().map(|d| d.join(format!("{stem}.none")));
    if let Some(bytes) = jpg.as_deref().and_then(|p| std::fs::read(p).ok()) {
        return Some(bytes);
    }
    if portrait && none.as_deref().is_some_and(Path::is_file) {
        return None;
    }
    let mut url = format!(
        "{CDN}/{appid}/{}",
        if portrait {
            "library_600x900.jpg"
        } else {
            "header.jpg"
        }
    );
    let mut resp = get(client, &url).await;
    if matches!(resp, Ok((404, _))) && !portrait {
        // Newer games keep their art under a hashed path; the Store API knows it.
        let alt = store_json(client, appid)
            .await
            .and_then(|d| d.get("header_image")?.as_str().map(str::to_string));
        if let Some(alt) = alt {
            url = alt;
            resp = get(client, &url).await;
        }
    }
    match resp {
        Ok((404, _)) if portrait => {
            if let Some(n) = &none {
                if let Some(d) = n.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(n, b"");
            }
            None
        }
        Ok((200, body)) if !body.is_empty() => {
            if let Some(j) = &jpg {
                if let Some(d) = j.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(j, &body);
            }
            Some(body)
        }
        Ok((status, _)) => {
            log::debug!("art {url}: HTTP {status}");
            None
        }
        Err(e) => {
            log::debug!("art {url}: {e}");
            None
        }
    }
}

/// A custom game's picture: a local file or an http(s) URL (cached like Python).
async fn custom_image(client: &reqwest::Client, name: &str, src: &str) -> Option<Vec<u8>> {
    if src.starts_with("http://") || src.starts_with("https://") {
        let cache = art_dir().map(|d| d.join(format!("url_{:08x}.img", crc32(src.as_bytes()))));
        if let Some(bytes) = cache.as_deref().and_then(|p| std::fs::read(p).ok()) {
            return Some(bytes);
        }
        match get(client, src).await {
            Ok((200, body)) => {
                if let Some(c) = &cache {
                    if let Some(d) = c.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(c, &body);
                }
                Some(body)
            }
            Ok((status, _)) => {
                log::warn!("custom game {name:?}: image {src}: HTTP {status}");
                None
            }
            Err(e) => {
                log::warn!("custom game {name:?}: image {src}: {e}");
                None
            }
        }
    } else {
        std::fs::read(expand_path(src))
            .map_err(|e| log::warn!("custom game {name:?}: image {src}: {e}"))
            .ok()
    }
}

/// The League client's local API says whether a game is Teamfight Tactics.
#[cfg(windows)]
async fn league_mode(args: Option<(String, String)>) -> &'static str {
    let Some((port, token)) = args else {
        return "lol";
    };
    let Ok(client) = reqwest::Client::builder()
        .danger_accept_invalid_certs(true) // the client's own self-signed certificate, on 127.0.0.1
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return "lol";
    };
    let url = format!("https://127.0.0.1:{port}/lol-gameflow/v1/session");
    let resp = client.get(url).basic_auth("riot", Some(token)).send().await;
    let Ok(resp) = resp else { return "lol" };
    let Ok(body) = resp.bytes().await else {
        return "lol";
    };
    serde_json::from_slice::<Value>(&body).map_or("lol", |v| league_mode_from_session(&v))
}

#[cfg(not(windows))]
async fn league_mode(_args: Option<(String, String)>) -> &'static str {
    let _ = league_mode_from_session;
    "lol"
}

// ---- art worker --------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
struct ArtRequest {
    wanted: Vec<Want>,
    /// Steam folder for its local art cache.
    root: Option<PathBuf>,
    /// Picture source of each custom game id.
    images: HashMap<u32, String>,
}

struct ArtWorker {
    out: crate::session::Publisher,
    client: Option<reqwest::Client>,
    frames: HashMap<(u32, u16), Vec<u8>>,
    sent: HashMap<String, (u32, u16)>,
    failed: HashMap<(u32, u16), Instant>,
}

impl ArtWorker {
    async fn process(&mut self, req: &ArtRequest) {
        let keep: HashSet<(u32, u16)> = req.wanted.iter().map(|w| (w.appid, w.kind)).collect();
        self.frames.retain(|k, _| keep.contains(k));
        for w in &req.wanted {
            let key = (w.appid, w.kind);
            if self.sent.get(&w.slot) == Some(&key) {
                continue; // already on the board (and kept by the publisher)
            }
            if !self.frames.contains_key(&key) {
                if self
                    .failed
                    .get(&key)
                    .is_some_and(|t| t.elapsed() < RETRY_ART)
                {
                    continue;
                }
                match self.build(req, w).await {
                    Some(frame) => {
                        self.frames.insert(key, frame);
                    }
                    None => {
                        self.failed.insert(key, Instant::now());
                        continue;
                    }
                }
            }
            if let Some(frame) = self.frames.get(&key) {
                self.out.publish_bin(&w.slot, frame.clone());
                self.sent.insert(w.slot.clone(), key);
            }
        }
    }

    async fn build(&self, req: &ArtRequest, w: &Want) -> Option<Vec<u8>> {
        let client = self.client.as_ref()?;
        let tall = w.kind != KIND_HEADER;
        let (header, portrait) = if is_custom(w.appid) {
            let src = req.images.get(&w.appid)?;
            let image = custom_image(client, &format!("{:08x}", w.appid), src).await?;
            (Some(image), None)
        } else if tall {
            let portrait = fetch_art(client, req.root.as_deref(), w.appid, true).await;
            let header = if portrait.is_none() {
                fetch_art(client, req.root.as_deref(), w.appid, false).await
            } else {
                None
            };
            if portrait.is_none() && header.is_none() {
                return None;
            }
            (header, portrait)
        } else {
            (
                Some(fetch_art(client, req.root.as_deref(), w.appid, false).await?),
                None,
            )
        };
        let (appid, kind) = (w.appid, w.kind);
        let frame = tokio::task::spawn_blocking(move || {
            art::build_frame(appid, kind, header.as_deref(), portrait.as_deref())
        })
        .await
        .ok()?;
        frame
            .map_err(|e| log::debug!("steam frame {appid}/{kind} failed: {e:#}"))
            .ok()
    }
}

async fn art_worker(out: crate::session::Publisher, mut rx: watch::Receiver<ArtRequest>) {
    let mut worker = ArtWorker {
        out,
        client: http_client(),
        frames: HashMap::new(),
        sent: HashMap::new(),
        failed: HashMap::new(),
    };
    loop {
        let req = rx.borrow_and_update().clone();
        worker.process(&req).await;
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            // Covers that failed are retried after a pause.
            _ = tokio::time::sleep(RETRY_ART) => {}
        }
    }
}

// ---- launch ------------------------------------------------------------------

#[cfg(windows)]
fn shell_open(target: &str) -> bool {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let file = HSTRING::from(target);
    // SAFETY: both strings outlive the call; no window handle is passed.
    let h = unsafe { ShellExecuteW(None, w!("open"), &file, None, None, SW_SHOWNORMAL) };
    h.0 as usize > 32
}

#[cfg(not(windows))]
fn shell_open(target: &str) -> bool {
    log::info!("steam launch (not on Windows): open {target}");
    false
}

/// A command line through cmd.exe, like Python's `Popen(shell=True)`.
#[cfg(windows)]
fn shell_command(cmd: &str) -> bool {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd")
        .raw_arg(format!("/c \"{cmd}\""))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| log::warn!("launch {cmd:?}: {e}"))
        .is_ok()
}

#[cfg(not(windows))]
fn shell_command(cmd: &str) -> bool {
    log::info!("steam launch (not on Windows): run {cmd}");
    false
}

fn launch_custom(game: &Game) -> bool {
    if game.launch.is_empty() {
        log::warn!("custom game {} has nothing to launch", game.name);
        return false;
    }
    let target = expand_path(&game.launch);
    log::info!("launching custom game {:?}: {target}", game.name);
    match plan_launch(&target, Path::new(&target).exists()) {
        LaunchAction::Open(t) => shell_open(&t),
        LaunchAction::Shell(c) => shell_command(&c),
    }
}

/// Start a game the board asked for. Only games in the last snapshot are allowed.
fn launch(core: &Mutex<Core>, allowed: &HashSet<u32>, appid: u32) -> bool {
    if !allowed.contains(&appid) {
        log::warn!("steam launch refused for appid {appid}");
        return false;
    }
    if is_custom(appid) {
        let game = core
            .lock()
            .ok()
            .and_then(|c| c.tracker.game(appid).cloned());
        return game.is_some_and(|g| launch_custom(&g));
    }
    log::info!("launching Steam game {appid}");
    shell_open(&format!("steam://rungameid/{appid}"))
}

// ---- the collector -------------------------------------------------------------

pub async fn run(ctx: Ctx) {
    let Ctx {
        out,
        mut settings,
        mut incoming,
    } = ctx;
    let core = Arc::new(Mutex::new(Core::new()));
    let (art_tx, art_rx) = watch::channel(ArtRequest::default());
    tokio::spawn(art_worker(out.clone(), art_rx));
    let client = http_client();

    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_key: Option<Value> = None;
    let mut sent_at: Option<Instant> = None;
    let mut allowed: HashSet<u32> = HashSet::new();
    let mut incoming_open = true;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let cfg = settings.borrow_and_update().games.clone();
                let now = now_secs();
                let c = core.clone();
                let needs_league = tokio::task::spawn_blocking(move || {
                    let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
                    g.configure(&cfg, now);
                    g.poll(now)
                })
                .await
                .unwrap_or(false);
                if needs_league {
                    let args = core.lock().ok().and_then(|c| c.league.clone());
                    let mode = league_mode(args).await;
                    if let Ok(mut g) = core.lock() {
                        g.tracker.set_league(mode, now);
                    }
                }
                let c = core.clone();
                let mut snap = tokio::task::spawn_blocking(move || {
                    c.lock().unwrap_or_else(|e| e.into_inner()).step(now)
                })
                .await
                .unwrap_or_default();

                // Names of games that are no longer installed: ask the Store API, then redo.
                let pending = core.lock().map(|mut g| g.names.take_missing()).unwrap_or_default();
                if !pending.is_empty() {
                    if let Some(client) = &client {
                        for id in pending {
                            let name = store_name(client, id).await;
                            if let Ok(mut g) = core.lock() {
                                g.names.resolve(id, name);
                            }
                        }
                    } else if let Ok(mut g) = core.lock() {
                        // No http client: don't ask again.
                        for id in pending {
                            g.names.resolve(id, None);
                        }
                    }
                    let c = core.clone();
                    snap = tokio::task::spawn_blocking(move || {
                        c.lock().unwrap_or_else(|e| e.into_inner()).compose(now)
                    })
                    .await
                    .unwrap_or_default();
                }

                allowed = snap
                    .playing
                    .iter()
                    .map(|p| p.appid)
                    .chain(snap.recent.iter().map(|g| g.appid))
                    .collect();
                let key = snap.change_key();
                let due = sent_at.is_none_or(|t| t.elapsed().as_secs() >= KEEPALIVE_S);
                if last_key.as_ref() != Some(&key) || due {
                    out.publish(KEY, snap.to_value());
                    last_key = Some(key);
                    sent_at = Some(Instant::now());
                }
                // Art after the JSON (see the module docs).
                let req = core.lock().map(|g| g.art_request(&snap)).unwrap_or_default();
                art_tx.send_if_modified(|cur| {
                    if *cur == req {
                        false
                    } else {
                        *cur = req;
                        true
                    }
                });
            }
            msg = incoming.recv(), if incoming_open => match msg {
                Ok(Incoming::SteamLaunch { appid }) => {
                    if let Ok(appid) = u32::try_from(appid) {
                        let c = core.clone();
                        let ok = allowed.clone();
                        let _ = tokio::task::spawn_blocking(move || launch(&c, &ok, appid)).await;
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => incoming_open = false,
            },
            changed = settings.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn rec(appid: u32, name: &str, last: i64) -> Recent {
        Recent {
            appid,
            name: name.into(),
            last,
            total_min: 0,
        }
    }

    fn playing(appid: u32, name: &str) -> Playing {
        Playing {
            appid,
            name: name.into(),
            session_s: 12,
            total_min: 3,
        }
    }

    /// A fake Steam folder under the temp dir; removed on drop.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("deskhub-steam-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tree(p)
        }

        fn write(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn acf(appid: u32, name: &str) -> String {
        format!("\"AppState\"\n{{\n\t\"appid\"\t\"{appid}\"\n\t\"name\"\t\"{name}\"\n}}\n")
    }

    fn fake_steam(tag: &str) -> Tree {
        let t = Tree::new(tag);
        let second = t.0.join("lib2");
        let path = second.to_string_lossy().replace('\\', "\\\\");
        t.write(
            "steamapps/libraryfolders.vdf",
            &format!(
                "\"libraryfolders\"\n{{\n\"0\" {{ \"path\" \"{}\" }}\n\"1\" {{ \"path\" \"{path}\" }}\n}}\n",
                t.0.to_string_lossy().replace('\\', "\\\\")
            ),
        );
        t.write("steamapps/appmanifest_570.acf", &acf(570, "Dota 2"));
        t.write(
            "steamapps/appmanifest_228980.acf",
            &acf(228980, "Steamworks Common Redistributables"),
        );
        t.write(
            "lib2/steamapps/appmanifest_730.acf",
            &acf(730, "Counter-Strike 2"),
        );
        t.write(
            "userdata/4242/config/localconfig.vdf",
            "\"UserLocalConfigStore\"\n{\n\"Software\"\n{\n\"Valve\"\n{\n\"Steam\"\n{\n\"apps\"\n{\n\
             \"570\" { \"LastPlayed\" \"1000\" \"Playtime\" \"600\" }\n\
             \"730\" { \"LastPlayed\" \"3000\" \"Playtime\" \"90\" }\n\
             \"228980\" { \"LastPlayed\" \"5000\" }\n\
             \"999\" { \"LastPlayed\" \"2000\" \"Playtime\" \"5\" }\n\
             \"111\" { \"LastPlayed\" \"0\" }\n\
             }\n}\n}\n}\n}\n",
        );
        t
    }

    #[test]
    fn local_files_parse() {
        let t = fake_steam("local");
        let libs = libraries(&t.0);
        assert_eq!(libs.len(), 2); // root repeated in libraryfolders.vdf is dropped
        let local = load_local(&t.0);
        assert_eq!(
            local.installed.get(&570).map(String::as_str),
            Some("Dota 2")
        );
        assert_eq!(
            local.installed.get(&730).map(String::as_str),
            Some("Counter-Strike 2")
        );
        assert_eq!(local.account_id, 4242);
        assert_eq!(
            local.apps[&570],
            AppInfo {
                last: 1000,
                total_min: 600
            }
        );
        assert_eq!(local.apps[&228980].last, 5000);
    }

    #[test]
    fn steam_part_recent_and_playing() {
        let t = fake_steam("part");
        let mut steam = Steam::new();
        let mut names = Names::default();
        // 730 is running; 999 is not installed -> "App 999" queued; the redistributable is skipped.
        let (playing, recent) = steam.part(&t.0, 730, 10_000.0, &mut names);
        let p = playing.unwrap();
        assert_eq!(
            (p.appid, p.name.as_str(), p.total_min, p.session_s),
            (730, "Counter-Strike 2", 90, 0)
        );
        let ids: Vec<u32> = recent.iter().map(|g| g.appid).collect();
        assert_eq!(ids, vec![999, 570]); // newest first, running game and tools left out
        assert_eq!(recent[0].name, "App 999");
        assert!(names.has_missing());
        assert_eq!(names.take_missing(), vec![999]);
        names.resolve(999, Some("Mystery".into()));
        let (playing, recent) = steam.part(&t.0, 730, 10_030.0, &mut names);
        assert_eq!(playing.unwrap().session_s, 30);
        assert_eq!(recent[0].name, "Mystery");
        // Nothing running: no playing, run clock follows now.
        let (playing, recent) = steam.part(&t.0, 0, 10_040.0, &mut names);
        assert!(playing.is_none());
        assert_eq!(recent[0].appid, 730);
        // A failed lookup is not queued again.
        let mut n2 = Names::default();
        n2.resolve(5, None);
        assert_eq!(n2.name(5, &HashMap::new()), "App 5");
        assert!(!n2.has_missing());
    }

    #[test]
    fn names_file_round_trip() {
        let t = Tree::new("names");
        let file = t.0.join("steam_names.json");
        let mut n = Names::load(Some(file.clone()));
        n.resolve(7, Some("Seven".into()));
        let mut again = Names::load(Some(file));
        assert_eq!(again.name(7, &HashMap::new()), "Seven");
    }

    #[test]
    fn game_filter() {
        assert!(is_game(570, "Dota 2"));
        assert!(!is_game(228_980, "Whatever"));
        assert!(!is_game(1, "Steam Linux Runtime 3.0 (sniper)"));
        assert!(!is_game(2, "Proton Experimental"));
        assert!(!is_game(3, "Steamworks Common Redistributables"));
    }

    #[test]
    fn merge_and_combine() {
        // Steam recent 3,2 plus custom 9 (newest) and the custom game playing (7) left out.
        let merged = merge_recent(
            vec![rec(3, "c", 300), rec(2, "b", 200)],
            vec![rec(9, "x", 900), rec(7, "p", 800), rec(8, "y", 100)],
            Some(7),
        );
        let ids: Vec<u32> = merged.iter().map(|g| g.appid).collect();
        assert_eq!(ids, vec![9, 3, 2, 8]);
        // Capped at 5.
        let many: Vec<Recent> = (0..5).map(|i| rec(i, "s", i64::from(i) * 10)).collect();
        assert_eq!(merge_recent(many, vec![rec(50, "c", 1000)], None).len(), 5);

        // A running Steam game wins over a custom one.
        let snap = combine(
            true,
            true,
            (Some(playing(570, "Dota 2")), vec![]),
            Some(playing(0xC000_0001, "MC")),
            vec![],
        );
        assert_eq!(snap.playing.unwrap().appid, 570);
        // Custom playing when Steam has none; found follows custom games when Steam is off.
        let snap = combine(
            false,
            true,
            (None, vec![]),
            Some(playing(0xC000_0001, "MC")),
            vec![],
        );
        assert!(snap.found);
        assert_eq!(snap.playing.unwrap().name, "MC");
        let snap = combine(false, false, (None, vec![]), None, vec![]);
        assert!(!snap.found);
    }

    #[test]
    fn snapshot_json_matches_python_shape() {
        let snap = Snapshot {
            found: true,
            playing: Some(playing(570, "Dota 2")),
            recent: vec![rec(730, "CS2", 3000)],
        };
        let v = snap.to_value();
        assert_eq!(
            v,
            json!({
                "type": "steam", "found": true,
                "playing": {"appid": 570, "name": "Dota 2", "session_s": 12, "total_min": 3},
                "recent": [{"appid": 730, "name": "CS2", "last": 3000, "total_min": 0}],
                "friends": [],
            })
        );
        assert!(snap.change_key()["playing"].get("session_s").is_none());
        let idle = Snapshot::default().to_value();
        assert!(idle["playing"].is_null());
    }

    #[test]
    fn art_slots() {
        let snap = Snapshot {
            found: true,
            playing: Some(playing(570, "Dota 2")),
            recent: vec![rec(730, "CS2", 3000), rec(5, "e", 2)],
        };
        let w = wanted_art(&snap);
        let got: Vec<(&str, u32, u16)> = w
            .iter()
            .map(|w| (w.slot.as_str(), w.appid, w.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("steam_art:header", 570, 0),
                ("steam_art:thumb", 570, 1),
                ("steam_art:portrait:0", 730, 2),
                ("steam_art:portrait:1", 5, 2),
            ]
        );
        // Nothing playing: thumb of the latest game only.
        let idle = Snapshot {
            playing: None,
            ..snap.clone()
        };
        let w = wanted_art(&idle);
        assert_eq!(
            (w[0].slot.as_str(), w[0].appid, w[0].kind),
            ("steam_art:thumb", 730, 1)
        );
        assert_eq!(w.len(), 3);
        assert!(wanted_art(&Snapshot::default()).is_empty());
    }

    #[test]
    fn local_art_lookup_old_and_new_layouts() {
        let t = Tree::new("art");
        assert!(find_local_art(&t.0, 10, false).is_none());
        t.write("appcache/librarycache/10_header.jpg", "old");
        t.write("appcache/librarycache/10_library_600x900.jpg", "oldp");
        t.write("appcache/librarycache/20/header.jpg", "new");
        t.write(
            "appcache/librarycache/30/abcdef0123/library_600x900.jpg",
            "hashed",
        );
        let lib = t.0.join("appcache/librarycache");
        assert_eq!(
            find_local_art(&t.0, 10, false),
            Some(lib.join("10_header.jpg"))
        );
        assert_eq!(
            find_local_art(&t.0, 10, true),
            Some(lib.join("10_library_600x900.jpg"))
        );
        assert_eq!(
            find_local_art(&t.0, 20, false),
            Some(lib.join("20/header.jpg"))
        );
        assert!(find_local_art(&t.0, 20, true).is_none());
        assert_eq!(
            find_local_art(&t.0, 30, true),
            Some(lib.join("30/abcdef0123/library_600x900.jpg"))
        );
    }

    #[test]
    fn league_command_line() {
        let cmd: Vec<String> = [
            "LeagueClientUx.exe",
            "--app-port=5000",
            "--remoting-auth-token=tok",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(league_args(&cmd), Some(("5000".into(), "tok".into())));
        assert_eq!(league_args(&cmd[..2]), None);
    }

    #[test]
    fn launch_only_known_games() {
        let core = Mutex::new(Core::new());
        let allowed: HashSet<u32> = [570].into();
        assert!(!launch(&core, &allowed, 730)); // not in the last snapshot: refused
        assert!(!launch(&core, &allowed, 570)); // allowed, but this host can't open it
    }
}
