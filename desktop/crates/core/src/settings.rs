//! Desk Hub settings: the JSON the frontend edits (README contract), the status
//! the service reports, and the import of the Python companion's config.toml.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::pages::{
    self, ALL_PAGES, DEFAULT_LEFT, DEFAULT_ORDER, DEFAULT_PAGES, DEFAULT_RIGHT,
    DEFAULT_RIGHT_WITH_PRINTER,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub board: Board,
    pub display: Display,
    pub pages: Pages,
    pub home: Home,
    pub appearance: Appearance,
    /// The board's city, coordinates, time zone and units ("location" message).
    pub location: Location,
    pub ai: Ai,
    /// Shortcut tiles, in order (config.toml [[tiles]]). At most MAX_TILES.
    pub tiles: Vec<Tile>,
    /// Steam, built-in game presets and your own games (config.toml [steam],
    /// [game_presets], [[games]]).
    pub games: Games,
    /// Plex server ("Sign in with Plex"); its token lives in the credential store.
    pub media_server: MediaServer,
    /// The P4 board's speaker (layout "sound").
    pub sound: Sound,
    /// .ics feeds for the Calendar page (collectors/calendar.rs).
    pub calendar: Calendar,
    /// The Internet page (collectors/net.rs).
    pub internet: Internet,
    /// config.toml was imported once.
    pub imported: bool,
}

/// The Plex server shown beside the PC on the Media page. `signed_in` means the plex.tv token
/// is in the credential store ("Desk Hub" / "plex-token"); `server_id` is the server's
/// clientIdentifier from plex.tv. `client_id` names this install to Plex (X-Plex-Client-Identifier).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaServer {
    pub signed_in: bool,
    pub server_id: String,
    pub server_name: String,
    pub client_id: String,
}

impl MediaServer {
    /// Signed in with a server chosen: the collector has something to poll.
    pub fn ready(&self) -> bool {
        self.signed_in && !self.server_id.is_empty() && !self.client_id.is_empty()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            board: Board::default(),
            display: Display::default(),
            pages: Pages::default(),
            home: Home::default(),
            appearance: Appearance::default(),
            location: Location::default(),
            ai: Ai::default(),
            tiles: default_tiles(),
            games: Games::default(),
            media_server: MediaServer::default(),
            sound: Sound::default(),
            calendar: Calendar::default(),
            internet: Internet::default(),
            imported: false,
        }
    }
}

/// Weather location, clock and units of the board. Until `enabled` the app sends nothing and
/// the board keeps what it stores (from its local build); the UI shows the board's values
/// (hello_ok) as the starting point. Coordinates are decimal strings ("48.2082").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Location {
    pub enabled: bool,
    pub city: String,
    pub lat: String,
    pub lon: String,
    /// POSIX TZ string, e.g. "CET-1CEST,M3.5.0,M10.5.0/3".
    pub tz: String,
    pub fahrenheit: bool,
    pub h24: bool,
}

impl Default for Location {
    fn default() -> Self {
        // The firmware's Kconfig defaults.
        Self {
            enabled: false,
            city: "New York".into(),
            lat: "40.7128".into(),
            lon: "-74.0060".into(),
            tz: "EST5EDT,M3.2.0,M11.1.0".into(),
            fahrenheit: true,
            h24: false,
        }
    }
}

/// Sounds on the board's speaker (ESP32-P4 board only). Each event names a sound:
/// "off", "chime", "bell", "beep" or "soft" (firmware audio.h).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sound {
    pub mute: bool,
    /// 0-100.
    pub volume: i64,
    /// Claude Code waiting alerts.
    pub alert: String,
    /// Timer done.
    pub timer: String,
    /// Calendar reminder.
    pub reminder: String,
    /// The calendar reminder plays this many minutes before an event (0 = at the start).
    pub remind_minutes: i64,
}

impl Default for Sound {
    fn default() -> Self {
        Self {
            mute: false,
            volume: 60,
            alert: "chime".into(),
            timer: "bell".into(),
            reminder: "chime".into(),
            remind_minutes: 5,
        }
    }
}

/// Calendar feeds: .ics links (Outlook's published calendar, Google's secret address,
/// any webcal:// link). The links can grant access: never logged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calendar {
    pub feeds: Vec<CalendarFeed>,
    /// Days ahead the Calendar page shows (1-60).
    pub days: i64,
}

impl Default for Calendar {
    fn default() -> Self {
        Self { feeds: Vec::new(), days: 7 }
    }
}

/// Internet page options (config.toml [internet]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Internet {
    /// Show the public IP address (looked up at speed.cloudflare.com every 30 minutes).
    pub show_ip: bool,
    /// Run a speed test every this many hours (0 = only when tapped on the board).
    pub speedtest_hours: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarFeed {
    /// Shown on the board under each event ("" = "Calendar N").
    pub name: String,
    pub url: String,
    /// "#RRGGBB" ("" = the default blue).
    pub color: String,
}

/// Sound names the board knows; anything else is sent as "off".
pub const SOUNDS: [&str; 5] = ["off", "chime", "bell", "beep", "soft"];

/// Claude Code / Codex usage and waiting alerts (config.toml [ai] and [claude]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ai {
    /// "auto" (most recently used), "claude" or "codex".
    pub agent: String,
    /// Opt-in: read the 5-hour/weekly limits from Anthropic's usage endpoint.
    pub live_limits: bool,
    /// Show "Claude is waiting" alerts on the board.
    pub alerts: bool,
    /// An idle session raises an alert after this many seconds.
    pub idle_alert_seconds: i64,
    /// Claude Code on a server: its deskhub-relay, e.g. "http://192.168.68.107:8791"
    /// ("" = none). The token is in the credential store (secrets::REMOTE_TOKEN).
    pub remote_url: String,
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            agent: "auto".into(),
            live_limits: false,
            alerts: true,
            idle_alert_seconds: 30,
            remote_url: String::new(),
        }
    }
}

/// Up to this many shortcut tiles (shortcuts.py MAX_TILES).
pub const MAX_TILES: usize = 16;
/// Tile actions (shortcuts.py ACTIONS).
pub const TILE_ACTIONS: [&str; 7] = ["keys", "run", "open", "media", "volume", "lock", "mic"];
/// Windows states a tile can light up for (shortcuts.py STATES).
pub const TILE_STATES: [&str; 2] = ["mic_mute", "speaker_mute"];

/// One shortcut tile on the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tile {
    pub label: String,
    /// A Material icon name the firmware knows ("apps" when unknown).
    pub icon: String,
    /// "#rrggbb".
    pub color: String,
    /// One of TILE_ACTIONS.
    pub action: String,
    /// keys: "ctrl+shift+m"; run: a command; open: a path or URL; media: toggle|next|prev;
    /// volume: up|down|mute; lock and mic: empty.
    pub target: String,
    /// Lights the tile while on: "mic_mute", "speaker_mute", "" (never). None = guess
    /// like shortcuts.py tile_state (volume mute -> speaker_mute, mic -> mic_mute).
    pub state: Option<String>,
}

impl Default for Tile {
    fn default() -> Self {
        Self {
            label: String::new(),
            icon: "apps".into(),
            color: "#2b2f3a".into(),
            action: "keys".into(),
            target: String::new(),
            state: None,
        }
    }
}

impl Tile {
    fn new(label: &str, icon: &str, color: &str, action: &str, target: &str) -> Self {
        Self {
            label: label.into(),
            icon: icon.into(),
            color: color.into(),
            action: action.into(),
            target: target.into(),
            state: None,
        }
    }

    /// The Windows state this tile shows (shortcuts.py tile_state).
    pub fn effective_state(&self) -> &str {
        if let Some(st) = &self.state {
            return TILE_STATES.iter().copied().find(|s| s == st).unwrap_or("");
        }
        if self.action == "volume" && self.target == "mute" {
            "speaker_mute"
        } else if self.action == "mic"
            || ["mic", "mic_off", "mic_none"].contains(&self.icon.as_str())
        {
            "mic_mute"
        } else {
            ""
        }
    }
}

/// The tiles from companion/config.example.toml.
pub fn default_tiles() -> Vec<Tile> {
    vec![
        Tile::new("Mute mic", "mic_off", "#b4413c", "mic", ""),
        Tile::new("Mute sound", "volume_off", "#3a4256", "volume", "mute"),
        Tile::new("Vol -", "volume_down", "#3a4256", "volume", "down"),
        Tile::new("Vol +", "volume_up", "#3a4256", "volume", "up"),
        Tile::new(
            "Screenshot",
            "screenshot_monitor",
            "#2f6f5e",
            "keys",
            "win+shift+s",
        ),
        Tile::new("Terminal", "terminal", "#2b2f3a", "run", "wt.exe"),
        Tile::new("Downloads", "folder", "#8a6d1f", "open", "~/Downloads"),
        Tile::new("Lock PC", "lock", "#5b3fa0", "lock", ""),
    ]
}

/// Games page sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Games {
    /// Read Steam's local files: playing now, recent games, launch.
    pub steam: bool,
    /// Recognise popular non-Steam games (game_presets.py) without setup.
    pub presets: bool,
    /// Preset names turned off.
    pub presets_disabled: Vec<String>,
    /// Your own games ([[games]]); one with a preset's name replaces the preset.
    pub custom: Vec<CustomGame>,
}

impl Default for Games {
    fn default() -> Self {
        Self {
            steam: true,
            presets: true,
            presets_disabled: Vec::new(),
            custom: Vec::new(),
        }
    }
}

/// A non-Steam game, watched by process and/or window title (custom_games.py).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomGame {
    pub name: String,
    /// Program file names, e.g. "javaw.exe".
    pub process: Vec<String>,
    /// Text the command line must contain.
    #[serde(rename = "match")]
    pub match_text: String,
    /// Text a window title must contain.
    pub title: String,
    /// A picture file or https URL.
    pub image: String,
    /// Program, shortcut, folder or URL the board can start.
    pub launch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Board {
    pub host: String,
    pub token: String,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            host: "deskhub.local".into(),
            token: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    /// 1-100.
    pub brightness: i64,
    /// 0-100.
    pub night_brightness: i64,
    /// "" (off) or "HH:MM-HH:MM".
    pub night: String,
    pub night_style: NightStyle,
    pub pc_sleep: PcSleep,
    /// The Home clock shows seconds.
    pub clock_seconds: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            brightness: 100,
            night_brightness: 0,
            night: String::new(),
            night_style: NightStyle::Off,
            pc_sleep: PcSleep::Off,
            clock_seconds: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pages {
    /// Page names in navigation order (home first).
    pub enabled: Vec<String>,
    pub nav: Nav,
}

impl Default for Pages {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_PAGES.iter().map(|p| p.to_string()).collect(),
            nav: Nav::Left,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Home {
    pub header: bool,
    pub grid: Vec<GridItem>,
    /// Layouts the user saved in the Home builder (the built-in ones live in the UI).
    pub presets: Vec<HomePreset>,
}

impl Default for Home {
    fn default() -> Self {
        Self {
            header: true,
            grid: pages::default_grid(),
            presets: Vec::new(),
        }
    }
}

/// A saved Home layout: header on/off plus the block grid.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HomePreset {
    pub name: String,
    pub header: bool,
    pub grid: Vec<GridItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GridItem {
    pub widget: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Default for GridItem {
    fn default() -> Self {
        Self {
            widget: String::new(),
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    /// The board's colours: the app theme, the board's classic amber look, or a theme of its own.
    pub board_theme: BoardTheme,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            board_theme: BoardTheme::Same,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoardTheme {
    /// Follow the app theme.
    #[default]
    Same,
    /// The firmware's own amber look.
    Classic,
    Purple,
    Blue,
    Graphite,
    Midnight,
    Teal,
    Rose,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NightStyle {
    #[default]
    Off,
    Clock,
    /// Stay on with the normal pages, only dimmed to the night brightness.
    Dim,
}

/// Night brightness used for "dim" when none is set.
pub const DIM_DEFAULT: i64 = 20;

impl NightStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Clock => "clock",
            Self::Dim => "dim",
        }
    }

    /// Case and surrounding spaces ignored; unknown values give None.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "clock" => Some(Self::Clock),
            "dim" => Some(Self::Dim),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PcSleep {
    #[default]
    Off,
    Clock,
    /// Stay as it is.
    #[serde(rename = "none")]
    Stay,
}

impl PcSleep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Clock => "clock",
            Self::Stay => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "clock" => Some(Self::Clock),
            "none" => Some(Self::Stay),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Nav {
    #[default]
    Left,
    Right,
    Bottom,
}

impl Nav {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Bottom => "bottom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Purple,
    Blue,
    Graphite,
    #[default]
    Midnight,
    Teal,
    Rose,
}

impl Settings {
    /// Keep brightness in range: 1-100 by day, 0-100 at night.
    pub fn clamp(&mut self) {
        self.display.brightness = self.display.brightness.clamp(1, 100);
        self.display.night_brightness = self.display.night_brightness.clamp(0, 100);
    }

    /// Apply a frontend patch: one level deep per section (`{"display": {"brightness": 50}}`).
    /// Unknown keys are ignored. A value of the wrong type is skipped and the rest still apply.
    /// Brightness is clamped after the merge.
    pub fn merge_patch(&mut self, patch: Value) {
        let Value::Object(entries) = patch else {
            return;
        };
        let Ok(mut tree) = serde_json::to_value(&*self) else {
            return;
        };
        for (key, value) in entries {
            match value {
                Value::Object(fields) if tree.get(&key).is_some_and(Value::is_object) => {
                    for (field, v) in fields {
                        try_set(&mut tree, Some(&key), &field, v);
                    }
                }
                other => try_set(&mut tree, None, &key, other),
            }
        }
        if let Ok(next) = serde_json::from_value::<Settings>(tree) {
            *self = next;
        }
        self.clamp();
    }
}

/// Set one field in `tree` if the field exists and the result still parses as Settings.
fn try_set(tree: &mut Value, section: Option<&str>, key: &str, v: Value) {
    let mut candidate = tree.clone();
    let slot = match section {
        Some(s) => candidate.get_mut(s).and_then(|sec| sec.get_mut(key)),
        None => candidate.get_mut(key),
    };
    let Some(slot) = slot else { return };
    *slot = v;
    if serde_json::from_value::<Settings>(candidate.clone()).is_ok() {
        *tree = candidate;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Searching,
    Connected,
    Refused,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub state: State,
    pub host: String,
    /// Human text, e.g. "Looking for deskhub.local", "Wrong token?".
    pub detail: String,
    /// Firmware version from hello_ok.
    pub firmware: Option<String>,
    /// Unix seconds the link came up.
    pub since: Option<u64>,
    /// The board's build: "s3", "p4rev1", "p4rev3" (hello_ok; older firmware: None).
    #[serde(default)]
    pub variant: Option<String>,
    /// Where the board keeps Wi-Fi and token: "nvs" (release images work), "seed", "none".
    #[serde(default)]
    pub creds: Option<String>,
    /// The location the board uses now (hello_ok; `enabled` is false).
    #[serde(default)]
    pub location: Option<Location>,
}

impl Status {
    pub fn searching(host: &str) -> Self {
        Self {
            state: State::Searching,
            host: host.to_string(),
            detail: format!("Looking for {host}"),
            firmware: None,
            since: None,
            variant: None,
            creds: None,
            location: None,
        }
    }
}

/// Default config.toml locations: an explicit path only, else ./config.toml and
/// %USERPROFILE%/<repo parent>/deskhub/companion/config.toml (see REPO_PARENTS).
/// None when nothing is found.
pub fn find_config(explicit: Option<&Path>) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = match explicit {
        Some(p) => vec![p.to_path_buf()],
        None => {
            let mut v = vec![std::env::current_dir()
                .unwrap_or_default()
                .join("config.toml")];
            if let Some(home) = std::env::var_os("USERPROFILE") {
                v.extend(home_candidates(Path::new(&home)));
            }
            v
        }
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// Common places for the deskhub checkout under the user's home folder.
const REPO_PARENTS: &[&str] = &[
    "",
    "src",
    "source/repos",
    "Documents",
    "Documents/GitHub",
    "GitHub",
    "code",
    "dev",
    "projects",
    "repos",
];

/// `<home>/<parent>/deskhub/companion/config.toml` for every common repo parent.
fn home_candidates(home: &Path) -> Vec<PathBuf> {
    REPO_PARENTS
        .iter()
        .map(|parent| {
            let mut p = home.to_path_buf();
            for part in parent.split('/').filter(|s| !s.is_empty()) {
                p.push(part);
            }
            p.join("deskhub").join("companion").join("config.toml")
        })
        .collect()
}

/// Read a companion config.toml from disk and import it.
pub fn import_config_file(path: &Path) -> Result<Settings> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    import_config_toml(&text)
}

/// Map the Python companion's config.toml onto Settings. Sections not in Settings
/// (calendar, steam, tiles, printers, ...) are not imported. Legacy [home] left/right
/// columns convert to the grid when no grid is set. Printers and [[games]] decide the
/// default page list; the Steam install check is not done here, so Games is on only
/// when [[games]] is configured.
pub fn import_config_toml(text: &str) -> Result<Settings> {
    let cfg: Value = serde_json::to_value(
        toml::from_str::<toml::Table>(text).context("config.toml is not valid TOML")?,
    )?;
    let mut s = Settings::default();

    if let Some(hub) = section(&cfg, "hub") {
        if let Some(host) = text_at(hub, "host") {
            s.board.host = host;
        }
        if let Some(token) = text_at(hub, "token") {
            s.board.token = token;
        }
    }

    if let Some(d) = section(&cfg, "display") {
        s.display.brightness = d.get("brightness").and_then(Value::as_i64).unwrap_or(100);
        s.display.night_brightness = d
            .get("night_brightness")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        s.display.night = text_at(d, "night").unwrap_or_default().trim().to_string();
        s.display.night_style = text_at(d, "night_style")
            .and_then(|v| NightStyle::parse(&v))
            .unwrap_or_default();
        s.display.pc_sleep = text_at(d, "pc_sleep")
            .and_then(|v| PcSleep::parse(&v))
            .unwrap_or_default();
    }

    s.home.grid = import_grid(&cfg);
    s.home.header = match section(&cfg, "home").and_then(|h| h.get("header")) {
        Some(v) => truthy(v),
        None => true,
    };

    let pages_sec = section(&cfg, "pages");
    s.pages.enabled = match pages_sec
        .and_then(|p| p.get("enabled"))
        .and_then(Value::as_array)
    {
        Some(list) => normalize_pages(list),
        None => default_enabled(&cfg, &s.home.grid),
    };
    s.pages.nav = pages_sec
        .and_then(|p| text_at(p, "nav"))
        .and_then(|v| Nav::parse(&v))
        .unwrap_or_default();

    if let Some(a) = section(&cfg, "ai") {
        if let Some(agent) = text_at(a, "agent") {
            let agent = agent.trim().to_lowercase();
            if ["auto", "claude", "codex"].contains(&agent.as_str()) {
                s.ai.agent = agent;
            }
        }
    }
    if let Some(c) = section(&cfg, "claude") {
        if let Some(v) = c.get("live_limits") {
            s.ai.live_limits = truthy(v);
        }
        if let Some(v) = c.get("alerts") {
            s.ai.alerts = truthy(v);
        }
        if let Some(v) = c.get("idle_alert_seconds").and_then(Value::as_i64) {
            s.ai.idle_alert_seconds = v.max(0);
        }
    }

    if let Some(tiles) = cfg.get("tiles").and_then(Value::as_array) {
        s.tiles = tiles
            .iter()
            .filter(|t| t.is_object())
            .take(MAX_TILES)
            .map(|t| {
                let d = Tile::default();
                Tile {
                    label: text_at(t, "label").unwrap_or_default(),
                    icon: text_at(t, "icon").unwrap_or(d.icon),
                    color: text_at(t, "color").unwrap_or(d.color),
                    action: text_at(t, "action").unwrap_or_default(),
                    target: t.get("target").map(scalar_text).unwrap_or_default(),
                    state: t.get("state").map(scalar_text),
                }
            })
            .collect();
    }
    if let Some(st) = section(&cfg, "steam") {
        if let Some(v) = st.get("enabled") {
            s.games.steam = truthy(v);
        }
    }
    if let Some(gp) = section(&cfg, "game_presets") {
        if let Some(v) = gp.get("enabled") {
            s.games.presets = truthy(v);
        }
        if let Some(list) = gp.get("disabled").and_then(Value::as_array) {
            s.games.presets_disabled = list
                .iter()
                .map(scalar_text)
                .filter(|n| !n.is_empty())
                .collect();
        }
    }
    if let Some(games) = cfg.get("games").and_then(Value::as_array) {
        s.games.custom = games
            .iter()
            .filter(|g| g.is_object())
            .map(|g| CustomGame {
                name: text_at(g, "name").unwrap_or_default().trim().to_string(),
                process: match g.get("process") {
                    Some(Value::Array(a)) => a.iter().map(scalar_text).collect(),
                    Some(v) => vec![scalar_text(v)],
                    None => Vec::new(),
                }
                .into_iter()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
                match_text: text_at(g, "match").unwrap_or_default(),
                title: text_at(g, "title").unwrap_or_default(),
                image: text_at(g, "image").unwrap_or_default(),
                launch: text_at(g, "launch").unwrap_or_default(),
            })
            .filter(|g| !g.name.is_empty())
            .collect();
    }

    s.imported = true;
    s.clamp();
    Ok(s)
}

/// A TOML scalar as text (numbers too, like Python's str()).
fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn section<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|x| x.is_object())
}

fn text_at(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_string)
}

/// Python truthiness for the few value kinds a TOML config can hold.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python int() of a config value: missing gives the default; floats truncate;
/// bools count as 0/1; numeric strings parse; anything else gives None (entry skipped).
fn py_int(v: Option<&Value>, default: i64) -> Option<i64> {
    match v {
        None => Some(default),
        Some(Value::Number(n)) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        }),
        Some(Value::Bool(b)) => Some(i64::from(*b)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        Some(_) => None,
    }
}

/// [pages] enabled: home first, then the known names in file order, no repeats.
fn normalize_pages(list: &[Value]) -> Vec<String> {
    let mut order = vec!["home".to_string()];
    for name in list.iter().filter_map(Value::as_str).map(str::to_lowercase) {
        if ALL_PAGES.contains(&name.as_str()) && !order.contains(&name) {
            order.push(name);
        }
    }
    order
}

/// Page list with no [pages] block: pages.py defaults, plus printer, games and github
/// when their config is present.
fn default_enabled(cfg: &Value, grid: &[GridItem]) -> Vec<String> {
    let mut want: Vec<&str> = DEFAULT_PAGES.to_vec();
    if cfg.get("printers").is_some_and(truthy) {
        want.push("printer");
    }
    if cfg.get("games").is_some_and(truthy) {
        want.push("games");
    }
    if grid.iter().any(|g| g.widget == "github") {
        want.push("github");
    }
    let mut out = Vec::new();
    for p in DEFAULT_ORDER {
        if want.contains(p) {
            out.push(p.to_string());
        }
    }
    out
}

/// [home] grid entries (validated), or the legacy left/right columns converted.
fn import_grid(cfg: &Value) -> Vec<GridItem> {
    match section(cfg, "home")
        .and_then(|h| h.get("grid"))
        .and_then(Value::as_array)
    {
        Some(raw) => {
            let items: Vec<GridItem> = raw.iter().filter_map(grid_entry).collect();
            pages::validate_grid(&items)
        }
        None => {
            let (left, right) = legacy_columns(cfg);
            pages::columns_to_grid(&left, &right)
        }
    }
}

fn grid_entry(e: &Value) -> Option<GridItem> {
    let obj = e.as_object()?;
    let widget = obj
        .get("widget")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    Some(GridItem {
        widget,
        x: py_int(obj.get("x"), 0)?,
        y: py_int(obj.get("y"), 0)?,
        w: py_int(obj.get("w"), 1)?,
        h: py_int(obj.get("h"), 1)?,
    })
}

/// Legacy [home] left/right lists, filtered to known widgets, right minus left.
fn legacy_columns(cfg: &Value) -> (Vec<String>, Vec<String>) {
    let home = section(cfg, "home");
    let column = |key: &str, default: &[&str]| -> Vec<String> {
        match home.and_then(|h| h.get(key)).and_then(Value::as_array) {
            Some(list) => list
                .iter()
                .map(|v| v.as_str().unwrap_or("").to_lowercase())
                .filter(|w| pages::is_home_widget(w))
                .collect(),
            None => default.iter().map(|s| s.to_string()).collect(),
        }
    };
    let left = column("left", DEFAULT_LEFT);
    let right_default = if cfg.get("printers").is_some_and(truthy) {
        DEFAULT_RIGHT_WITH_PRINTER
    } else {
        DEFAULT_RIGHT
    };
    let right = column("right", right_default)
        .into_iter()
        .filter(|w| !left.contains(w))
        .collect();
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages;
    use serde_json::json;

    fn example_path() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../companion/config.example.toml"
        ))
    }

    fn example() -> String {
        std::fs::read_to_string(example_path()).expect("companion/config.example.toml")
    }

    #[test]
    fn serializes_with_readme_keys() {
        let v = serde_json::to_value(Settings::default()).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "ai",
                "appearance",
                "board",
                "calendar",
                "display",
                "games",
                "home",
                "imported",
                "internet",
                "location",
                "media_server",
                "pages",
                "sound",
                "tiles"
            ]
        );
        assert_eq!(v["display"]["pc_sleep"], "off");
        assert_eq!(v["display"]["night_style"], "off");
        assert_eq!(v["pages"]["nav"], "left");
        assert_eq!(v["appearance"]["theme"], "midnight");
    }

    #[test]
    fn example_config_imports_to_expected_settings() {
        let s = import_config_toml(&example()).unwrap();
        assert!(s.imported);
        assert_eq!(
            s.board,
            Board {
                host: "deskhub.local".into(),
                token: "change-me".into()
            }
        );
        assert_eq!(s.display, Display::default());
        assert!(s.home.header);
        assert_eq!(s.home.grid, pages::default_grid());
        assert_eq!(s.pages.nav, Nav::Left);
        assert_eq!(
            s.pages.enabled,
            [
                "home",
                "stats",
                "ai",
                "calendar",
                "weather",
                "media",
                "printer",
                "timer",
                "games",
                "network",
                "shortcuts"
            ]
        );
    }

    #[test]
    fn example_config_layout_and_home_match_pages_py() {
        let s = import_config_toml(&example()).unwrap();
        // pages.py layout_message(cfg) for config.example.toml.
        assert_eq!(
            pages::layout_message(&s),
            json!({
                "type": "layout",
                "pages": ["home", "stats", "ai", "calendar", "weather", "media", "printer", "timer", "games", "network", "shortcuts"],
                "ordered": true,
                "nav": "left",
                "pc_sleep": "off",
                "night_start": -1,
                "night_end": -1,
                "night_style": "off",
                "brightness": 100,
                "night_brightness": 0,
                "seconds": false,
                "sound": {"mute": false, "volume": 60, "alert": "chime", "timer": "bell", "reminder": "chime", "remind_min": 5},
            })
        );
        // pages.py home_message(cfg): grid in file order, columns sorted by row.
        assert_eq!(
            pages::home_message(&s),
            json!({
                "type": "home",
                "left": ["claude", "events"],
                "right": ["weather", "media", "stats"],
                "header": true,
                "grid": [
                    {"widget": "claude", "x": 0, "y": 0, "w": 3, "h": 1},
                    {"widget": "events", "x": 0, "y": 1, "w": 3, "h": 2},
                    {"widget": "weather", "x": 3, "y": 0, "w": 2, "h": 1},
                    {"widget": "media", "x": 3, "y": 1, "w": 2, "h": 1},
                    {"widget": "stats", "x": 3, "y": 2, "w": 2, "h": 1},
                ],
            })
        );
    }

    #[test]
    fn legacy_columns_convert_when_no_grid() {
        let text = r#"
            [home]
            header = false
            left = ["Events", "claude", "bogus"]
            right = ["weather", "printer", "events"]
            [[printers]]
            name = "P1S"
        "#;
        let s = import_config_toml(text).unwrap();
        assert!(!s.home.header);
        assert_eq!(
            s.home.grid,
            vec![
                GridItem {
                    widget: "events".into(),
                    x: 0,
                    y: 0,
                    w: 3,
                    h: 1
                },
                GridItem {
                    widget: "claude".into(),
                    x: 0,
                    y: 1,
                    w: 3,
                    h: 1
                },
                GridItem {
                    widget: "weather".into(),
                    x: 3,
                    y: 0,
                    w: 2,
                    h: 1
                },
                GridItem {
                    widget: "printer".into(),
                    x: 3,
                    y: 1,
                    w: 2,
                    h: 1
                },
            ]
        );
        // Printer configured and no [pages]: printer page on by default.
        assert_eq!(
            s.pages.enabled,
            [
                "home",
                "stats",
                "ai",
                "calendar",
                "weather",
                "media",
                "printer",
                "timer",
                "network",
                "shortcuts"
            ]
        );
    }

    #[test]
    fn pages_display_and_clamps_follow_companion_rules() {
        let text = r#"
            [pages]
            enabled = ["Stats", "bogus", "stats", "github", "home"]
            nav = "Bottom"
            [display]
            night = "23:00-07:00"
            night_style = "CLOCK"
            pc_sleep = "weird"
            brightness = 250
            night_brightness = -5
        "#;
        let s = import_config_toml(text).unwrap();
        assert_eq!(s.pages.enabled, ["home", "stats", "github"]);
        assert_eq!(s.pages.nav, Nav::Bottom);
        assert_eq!(s.display.night, "23:00-07:00");
        assert_eq!(s.display.night_style, NightStyle::Clock);
        assert_eq!(s.display.pc_sleep, PcSleep::Off);
        assert_eq!(s.display.brightness, 100);
        assert_eq!(s.display.night_brightness, 0);
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(import_config_toml("[hub\nhost =").is_err());
    }

    #[test]
    fn media_server_defaults_patches_and_old_files() {
        let s = Settings::default();
        assert!(!s.media_server.ready());
        // settings.json from before Plex: the section defaults.
        let old: Settings = serde_json::from_value(json!({"imported": true})).unwrap();
        assert_eq!(old.media_server, MediaServer::default());

        let mut s = Settings::default();
        s.media_server.client_id = "deskhub-1".into();
        s.merge_patch(json!({"media_server": {"signed_in": true, "server_id": "abc", "server_name": "Home"}}));
        assert!(s.media_server.ready());
        assert_eq!(s.media_server.server_name, "Home");
        assert_eq!(s.media_server.client_id, "deskhub-1");
    }

    #[test]
    fn merge_patch_sets_fields_and_clamps() {
        let mut s = Settings::default();
        s.merge_patch(
            json!({"display": {"brightness": 250, "night_brightness": -3, "night": "22:00-06:00"}}),
        );
        assert_eq!(s.display.brightness, 100);
        assert_eq!(s.display.night_brightness, 0);
        assert_eq!(s.display.night, "22:00-06:00");

        s.merge_patch(json!({"display": {"brightness": -5}}));
        assert_eq!(s.display.brightness, 1);
        s.merge_patch(json!({"display": {"night_brightness": 150}}));
        assert_eq!(s.display.night_brightness, 100);
    }

    #[test]
    fn merge_patch_ignores_unknown_and_wrong_typed_keys() {
        let mut s = Settings::default();
        let before = s.clone();
        s.merge_patch(json!({"bogus": {"x": 1}, "display": {"nope": 1}, "pages": 5}));
        assert_eq!(s, before);

        // A wrong-typed value is skipped; valid keys in the same patch still apply.
        s.merge_patch(json!({"display": {"pc_sleep": "bogus", "night_style": "clock"}}));
        assert_eq!(s.display.pc_sleep, PcSleep::Off);
        assert_eq!(s.display.night_style, NightStyle::Clock);
    }

    #[test]
    fn merge_patch_replaces_sections_one_level_deep() {
        let mut s = Settings::default();
        s.merge_patch(json!({
            "pages": {"enabled": ["home", "ai"], "nav": "right"},
            "home": {"header": false},
            "board": {"host": "10.0.0.5"},
            "imported": true,
        }));
        assert_eq!(s.pages.enabled, ["home", "ai"]);
        assert_eq!(s.pages.nav, Nav::Right);
        assert!(!s.home.header);
        assert_eq!(
            s.home.grid,
            pages::default_grid(),
            "grid untouched by a partial home patch"
        );
        assert_eq!(s.board.host, "10.0.0.5");
        assert_eq!(
            s.board.token, "",
            "token untouched by a partial board patch"
        );
        assert!(s.imported);

        s.merge_patch(json!({"appearance": {"theme": "teal"}}));
        assert_eq!(s.appearance.theme, Theme::Teal);
    }

    #[test]
    fn home_candidates_cover_common_repo_folders() {
        let c = home_candidates(Path::new("/home/u"));
        assert_eq!(c[0], PathBuf::from("/home/u/deskhub/companion/config.toml"));
        assert!(c.contains(&PathBuf::from("/home/u/src/deskhub/companion/config.toml")));
        assert!(c.contains(&PathBuf::from(
            "/home/u/source/repos/deskhub/companion/config.toml"
        )));
    }

    #[test]
    fn find_config_uses_explicit_path_only_when_it_exists() {
        assert_eq!(find_config(Some(&example_path())), Some(example_path()));
        assert_eq!(
            find_config(Some(Path::new("/definitely/not/here/config.toml"))),
            None
        );
    }

    #[test]
    fn imports_ai_and_claude_sections() {
        let s = import_config_toml(
            "[ai]\nagent = \"Codex\"\n[claude]\nlive_limits = true\nalerts = false\nidle_alert_seconds = 45\n",
        )
        .unwrap();
        assert_eq!(s.ai.agent, "codex");
        assert!(s.ai.live_limits);
        assert!(!s.ai.alerts);
        assert_eq!(s.ai.idle_alert_seconds, 45);
        // Missing sections keep the defaults.
        let d = import_config_toml("").unwrap();
        assert_eq!(d.ai, Ai::default());
    }

    #[test]
    fn home_presets_round_trip_and_default_empty() {
        let mut s = Settings::default();
        assert!(s.home.presets.is_empty());
        s.home.presets.push(HomePreset {
            name: "Mine".into(),
            header: false,
            grid: pages::default_grid(),
        });
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["home"]["presets"][0]["name"], "Mine");
        let back: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(back.home.presets, s.home.presets);
        // Settings saved before presets existed still load.
        let old: Settings = serde_json::from_str(r#"{"home": {"header": true}}"#).unwrap();
        assert!(old.home.presets.is_empty());
    }

    #[test]
    fn imports_tiles_steam_and_games() {
        let s = import_config_toml(
            r#"
[steam]
enabled = false
[game_presets]
enabled = true
disabled = ["Roblox"]
[[games]]
name = "Minecraft"
process = "javaw.exe"
match = "minecraft"
launch = "C:/mc.exe"
[[games]]
name = "GTNH"
title = "GT: New Horizons"
process = ["java.exe", "javaw.exe"]
[[tiles]]
label = "Mic"
icon = "mic_off"
action = "mic"
[[tiles]]
label = "F13"
action = "keys"
target = "f13"
state = ""
"#,
        )
        .unwrap();
        assert!(!s.games.steam);
        assert_eq!(s.games.presets_disabled, ["Roblox"]);
        assert_eq!(s.games.custom.len(), 2);
        assert_eq!(s.games.custom[0].process, ["javaw.exe"]);
        assert_eq!(s.games.custom[0].match_text, "minecraft");
        assert_eq!(s.games.custom[1].process, ["java.exe", "javaw.exe"]);
        assert_eq!(s.tiles.len(), 2);
        assert_eq!(s.tiles[0].color, "#2b2f3a");
        assert_eq!(s.tiles[0].effective_state(), "mic_mute");
        assert_eq!(s.tiles[1].effective_state(), "");
        // match is the JSON key, like config.toml.
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["games"]["custom"][0]["match"], "minecraft");
    }

    #[test]
    fn default_tiles_and_state_guess() {
        let s = Settings::default();
        assert_eq!(s.tiles.len(), 8);
        assert_eq!(s.tiles[1].effective_state(), "speaker_mute");
        assert_eq!(s.tiles[2].effective_state(), "");
        assert!(s.games.steam && s.games.presets);
    }
}
