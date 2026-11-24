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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub board: Board,
    pub display: Display,
    pub pages: Pages,
    pub home: Home,
    pub appearance: Appearance,
    /// config.toml was imported once.
    pub imported: bool,
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
}

impl Default for Display {
    fn default() -> Self {
        Self {
            brightness: 100,
            night_brightness: 0,
            night: String::new(),
            night_style: NightStyle::Off,
            pc_sleep: PcSleep::Off,
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
}

impl Default for Home {
    fn default() -> Self {
        Self {
            header: true,
            grid: pages::default_grid(),
        }
    }
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NightStyle {
    #[default]
    Off,
    Clock,
}

impl NightStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Clock => "clock",
        }
    }

    /// Case and surrounding spaces ignored; unknown values give None.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "clock" => Some(Self::Clock),
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
    #[default]
    Purple,
    Blue,
    Graphite,
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
}

impl Status {
    pub fn searching(host: &str) -> Self {
        Self {
            state: State::Searching,
            host: host.to_string(),
            detail: format!("Looking for {host}"),
            firmware: None,
            since: None,
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

    s.imported = true;
    s.clamp();
    Ok(s)
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
                "appearance",
                "board",
                "display",
                "home",
                "imported",
                "pages"
            ]
        );
        assert_eq!(v["display"]["pc_sleep"], "off");
        assert_eq!(v["display"]["night_style"], "off");
        assert_eq!(v["pages"]["nav"], "left");
        assert_eq!(v["appearance"]["theme"], "purple");
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
}
