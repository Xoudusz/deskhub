//! Custom games and built-in presets: process / window-title watch, session counting,
//! last-played state. Port of companion/deskhub/custom_games.py and game_presets.py.
//!
//! Everything here is pure (probe data and clock are passed in) except the state file and
//! the helpers marked platform-specific, so it is unit-tested on Linux.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::collectors::stats::ascii_fold;
use crate::settings::{CustomGame, Games};

/// Ids at or above this are custom games (above every Steam appid).
pub const ID_BASE: u32 = 0xC000_0000;
const SAVE_EVERY_S: f64 = 60.0;
/// How long a League mode answer stays valid.
pub const LEAGUE_EVERY_S: f64 = 30.0;

/// The game on screen: now playing.
#[derive(Debug, Clone, PartialEq)]
pub struct Playing {
    pub appid: u32,
    pub name: String,
    pub session_s: i64,
    pub total_min: i64,
}

/// A recently played game.
#[derive(Debug, Clone, PartialEq)]
pub struct Recent {
    pub appid: u32,
    pub name: String,
    pub last: i64,
    pub total_min: i64,
}

// ---- ids and small helpers ---------------------------------------------------

/// CRC-32 (IEEE), same as Python `zlib.crc32`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Stable id for a custom game, derived from its name (custom_games.py `game_id`).
pub fn game_id(name: &str) -> u32 {
    ID_BASE | (crc32(name.trim().to_lowercase().as_bytes()) & 0x3FFF_FFFF)
}

pub fn is_custom(appid: u32) -> bool {
    appid >= ID_BASE
}

/// `~` and `%VAR%` expansion (Python expanduser + expandvars, Windows flavour).
pub fn expand_path(s: &str) -> String {
    let mut out = s.to_string();
    if out == "~" || out.starts_with("~/") || out.starts_with("~\\") {
        let home = if cfg!(windows) {
            std::env::var("USERPROFILE")
        } else {
            std::env::var("HOME")
        };
        if let Ok(home) = home {
            out = format!("{home}{}", &out[1..]);
        }
    }
    let mut result = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(start) = rest.find('%') {
        result.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match std::env::var(&after[..end]) {
                Ok(v) => {
                    result.push_str(&v);
                    rest = &after[end + 1..];
                }
                Err(_) => {
                    result.push('%');
                    rest = after;
                }
            },
            _ => {
                result.push('%');
                rest = after;
            }
        }
    }
    result.push_str(rest);
    result
}

/// How to start a launch target (custom_games.py `launch`).
#[derive(Debug, Clone, PartialEq)]
pub enum LaunchAction {
    /// ShellExecute "open": URL, file, shortcut or folder.
    Open(String),
    /// A command line, run through the shell.
    Shell(String),
}

pub fn plan_launch(target: &str, exists: bool) -> LaunchAction {
    if target.contains("://") || exists {
        LaunchAction::Open(target.to_string())
    } else {
        LaunchAction::Shell(target.to_string())
    }
}

// ---- games and presets --------------------------------------------------------

/// A watched game, normalised (lower-case process/match/title).
#[derive(Debug, Clone, PartialEq)]
pub struct Game {
    pub id: u32,
    pub name: String,
    pub process: Vec<String>,
    pub match_text: String,
    pub title: String,
    pub image: String,
    pub launch: String,
    /// Presets: "lol" / "tft" share one program.
    pub mode: String,
    /// World of Warcraft: each client flavour found running becomes its own game.
    pub flavours: bool,
}

/// An entry before normalising: a user game or a preset.
#[derive(Debug, Clone, Default)]
pub struct Raw {
    pub name: String,
    pub process: Vec<String>,
    pub match_text: String,
    pub title: String,
    pub image: String,
    pub launch: String,
    pub mode: String,
    pub flavours: bool,
}

impl From<&CustomGame> for Raw {
    fn from(g: &CustomGame) -> Self {
        Raw {
            name: g.name.clone(),
            process: g.process.clone(),
            match_text: g.match_text.clone(),
            title: g.title.clone(),
            image: g.image.clone(),
            launch: g.launch.clone(),
            mode: String::new(),
            flavours: false,
        }
    }
}

struct Preset {
    name: &'static str,
    process: &'static [&'static str],
    match_text: &'static str,
    launch: &'static str,
    steam_appid: u32,
    mode: &'static str,
}

const fn p(name: &'static str, process: &'static [&'static str]) -> Preset {
    Preset {
        name,
        process,
        match_text: "",
        launch: "",
        steam_appid: 0,
        mode: "",
    }
}

const RIOT_LOL: &str = "{riot} --launch-product=league_of_legends --launch-patchline=live";

fn presets() -> Vec<Preset> {
    vec![
        // Riot
        Preset {
            launch: RIOT_LOL,
            mode: "lol",
            ..p("League of Legends", &["league of legends.exe"])
        },
        Preset {
            launch: RIOT_LOL,
            mode: "tft",
            ..p("Teamfight Tactics", &["league of legends.exe"])
        },
        Preset {
            launch: "{riot} --launch-product=valorant --launch-patchline=live",
            ..p("VALORANT", &["valorant-win64-shipping.exe"])
        },
        // Epic
        Preset {
            launch: "com.epicgames.launcher://apps/Fortnite?action=launch&silent=true",
            ..p("Fortnite", &["fortniteclient-win64-shipping.exe"])
        },
        Preset {
            steam_appid: 252_950,
            launch: "com.epicgames.launcher://apps/Sugar?action=launch&silent=true",
            ..p("Rocket League", &["rocketleague.exe"])
        },
        // Minecraft
        Preset {
            match_text: "minecraft",
            launch: "C:/XboxGames/Minecraft Launcher/Content/Minecraft.exe",
            ..p("Minecraft", &["javaw.exe", "java.exe"])
        },
        Preset {
            launch: "minecraft://",
            ..p("Minecraft Bedrock", &["minecraft.windows.exe"])
        },
        p("Roblox", &["robloxplayerbeta.exe"]),
        // Battle.net
        Preset {
            steam_appid: 2_357_570,
            launch: "battlenet://Pro",
            ..p("Overwatch 2", &["overwatch.exe"])
        },
        Preset {
            launch: "battlenet://WoW",
            ..p(
                "World of Warcraft",
                &[
                    "wow.exe",
                    "wowclassic.exe",
                    "wowt.exe",
                    "wowb.exe",
                    "wowclassict.exe",
                ],
            )
        },
        Preset {
            steam_appid: 2_344_520,
            launch: "battlenet://Fen",
            ..p("Diablo IV", &["diablo iv.exe"])
        },
        Preset {
            launch: "battlenet://WTCG",
            ..p("Hearthstone", &["hearthstone.exe"])
        },
        Preset {
            launch: "battlenet://S2",
            ..p("StarCraft II", &["sc2_x64.exe", "sc2.exe"])
        },
        p("Call of Duty", &["cod.exe"]),
        // HoYoverse
        p("Genshin Impact", &["genshinimpact.exe", "yuanshen.exe"]),
        p("Honkai: Star Rail", &["starrail.exe"]),
        p("Zenless Zone Zero", &["zenlesszonezero.exe"]),
        // Others
        Preset {
            steam_appid: 271_590,
            ..p("Grand Theft Auto V", &["gta5.exe", "gta5_enhanced.exe"])
        },
        Preset {
            steam_appid: 1_172_470,
            ..p("Apex Legends", &["r5apex.exe", "r5apex_dx12.exe"])
        },
        p("EA SPORTS FC", &["fc25.exe", "fc26.exe"]),
        Preset {
            steam_appid: 1_222_670,
            ..p("The Sims 4", &["ts4_x64.exe"])
        },
        Preset {
            steam_appid: 1_240_440,
            ..p("Halo Infinite", &["haloinfinite.exe"])
        },
        p("Escape from Tarkov", &["escapefromtarkov.exe"]),
        p("osu!", &["osu!.exe"]),
    ]
}

/// A preset's launch target with `{riot}` filled in; "" when it can't be started here
/// (Riot client or the Minecraft Launcher not installed). `exists` checks local paths.
pub fn resolve_launch(template: &str, riot: &str, exists: &dyn Fn(&str) -> bool) -> String {
    if template.contains("{riot}") {
        return if riot.is_empty() {
            String::new()
        } else {
            template.replace("{riot}", &format!("\"{riot}\""))
        };
    }
    if !template.contains("://") && !exists(template) {
        return String::new();
    }
    template.to_string()
}

/// Presets as raw entries, minus those turned off or replaced by a user game of the same
/// name (`taken`, lower-case).
pub fn preset_entries(
    disabled: &[String],
    taken: &HashSet<String>,
    resolve: &dyn Fn(&str) -> String,
) -> Vec<Raw> {
    let disabled: HashSet<String> = disabled.iter().map(|n| n.trim().to_lowercase()).collect();
    let mut out = Vec::new();
    for pr in presets() {
        let key = pr.name.to_lowercase();
        if disabled.contains(&key) || taken.contains(&key) {
            continue;
        }
        out.push(Raw {
            name: pr.name.to_string(),
            process: pr.process.iter().map(|s| s.to_string()).collect(),
            match_text: pr.match_text.to_string(),
            title: String::new(),
            image: if pr.steam_appid != 0 {
                format!(
                    "https://cdn.cloudflare.steamstatic.com/steam/apps/{}/header.jpg",
                    pr.steam_appid
                )
            } else {
                String::new()
            },
            launch: if pr.launch.is_empty() {
                String::new()
            } else {
                resolve(pr.launch)
            },
            mode: pr.mode.to_string(),
            flavours: pr.name == WOW,
        });
    }
    out
}

/// User games, then the presets they haven't turned off or replaced, normalised.
pub fn load_games(cfg: &Games, resolve: &dyn Fn(&str) -> String) -> Vec<Game> {
    let user: Vec<Raw> = cfg.custom.iter().map(Raw::from).collect();
    let taken: HashSet<String> = user.iter().map(|g| g.name.trim().to_lowercase()).collect();
    let mut raw = user;
    if cfg.presets {
        raw.extend(preset_entries(&cfg.presets_disabled, &taken, resolve));
    }
    let mut games = Vec::new();
    for g in raw {
        let name = g.name.trim().to_string();
        let process: Vec<String> = g
            .process
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let title = g.title.trim().to_lowercase();
        if name.is_empty() || (process.is_empty() && title.is_empty()) {
            log::warn!("custom game {name:?} needs a name and a process or title");
            continue;
        }
        games.push(Game {
            id: game_id(&name),
            name: ascii_fold(&name, 40),
            process,
            match_text: g.match_text.trim().to_lowercase(),
            title,
            image: g.image.trim().to_string(),
            launch: g.launch.trim().to_string(),
            mode: g.mode,
            flavours: g.flavours,
        });
    }
    games
}

const WOW: &str = "World of Warcraft";

/// Name of the WoW client flavour from its install folder (`_retail_`, `_classic_`, ...).
pub fn wow_flavour_name(exe: &str) -> String {
    let lower = exe.to_lowercase();
    let folder = lower
        .rsplit(['/', '\\'])
        .find(|c| c.len() > 2 && c.starts_with('_') && c.ends_with('_'))
        .map(|c| &c[1..c.len() - 1]);
    match folder {
        None | Some("retail") => WOW.into(),
        Some("classic") => "WoW Classic".into(),
        Some("classic_era") => "WoW Classic Era".into(),
        Some("anniversary") => "WoW Anniversary".into(),
        Some("ptr") => "WoW PTR".into(),
        Some("beta") => "WoW Beta".into(),
        Some(other) => {
            let words: Vec<String> = other
                .split('_')
                .filter(|w| !w.is_empty())
                .map(|w| {
                    let mut c = w.chars();
                    c.next()
                        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                        .unwrap_or_default()
                })
                .collect();
            format!("WoW {}", words.join(" "))
        }
    }
}

// ---- detection ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    /// Lower-case program name.
    pub name: String,
    /// Lower-case command line.
    pub cmdline: String,
    /// Lower-case path of the executable (empty when unknown).
    pub exe: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WinInfo {
    /// Lower-case window title.
    pub title: String,
    /// Lower-case name of the owning program.
    pub proc: String,
}

/// What is running right now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    pub procs: Vec<ProcInfo>,
    pub windows: Vec<WinInfo>,
    /// League client local API (port, token), when its client runs.
    pub league: Option<(String, String)>,
}

/// "tft" when the League client's session says Teamfight Tactics, else "lol".
pub fn league_mode_from_session(session: &Value) -> &'static str {
    let mode = session
        .pointer("/gameData/queue/gameMode")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| session.pointer("/map/gameMode").and_then(Value::as_str));
    match mode {
        Some(m) if m.eq_ignore_ascii_case("TFT") => "tft",
        _ => "lol",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stat {
    pub last: i64,
    pub total_s: i64,
}

/// Tracks the running custom game and the played history.
pub struct Tracker {
    games: Vec<Game>,
    state: HashMap<u32, Stat>,
    running: u32,
    start: f64,
    saved_at: f64,
    league: (String, f64),
    state_file: Option<PathBuf>,
    /// Games added by flavour detection: id -> (name, base game name). Kept in the state
    /// file so they come back after a restart.
    flav: HashMap<u32, (String, String)>,
}

impl Tracker {
    pub fn new(games: Vec<Game>, state_file: Option<PathBuf>) -> Self {
        let state = state_file.as_deref().map(load_state).unwrap_or_default();
        let flav = state_file.as_deref().map(load_flavours).unwrap_or_default();
        let mut t = Self {
            games,
            state,
            running: 0,
            start: 0.0,
            saved_at: 0.0,
            league: ("lol".into(), -1e9),
            state_file,
            flav,
        };
        t.restore_flavours();
        t
    }

    /// Recreate the saved flavour games whose base game is active.
    fn restore_flavours(&mut self) {
        let mut add = Vec::new();
        for (id, (name, base)) in &self.flav {
            if self.games.iter().any(|g| g.id == *id) {
                continue;
            }
            let b = base.to_lowercase();
            if let Some(g) = self
                .games
                .iter()
                .find(|g| g.flavours && g.name.to_lowercase() == b)
            {
                let mut f = g.clone();
                f.id = *id;
                f.name = name.clone();
                f.flavours = false;
                add.push(f);
            }
        }
        add.sort_by_key(|g| g.id);
        self.games.extend(add);
    }

    pub fn games(&self) -> &[Game] {
        &self.games
    }

    /// Swap in a new game list (settings changed), keeping the history. A running game
    /// that is gone is stopped.
    pub fn set_games(&mut self, games: Vec<Game>, now: f64) {
        self.games = games;
        self.restore_flavours();
        if self.running != 0 && !self.games.iter().any(|g| g.id == self.running) {
            self.stop(now);
            self.running = 0;
        }
    }

    pub fn game(&self, id: u32) -> Option<&Game> {
        self.games.iter().find(|g| g.id == id)
    }

    /// Program names worth listing (those of games without a window title).
    pub fn wanted_procs(&self) -> HashSet<String> {
        self.games
            .iter()
            .filter(|g| g.title.is_empty())
            .flat_map(|g| g.process.iter().cloned())
            .collect()
    }

    pub fn wants_titles(&self) -> bool {
        self.games.iter().any(|g| !g.title.is_empty())
    }

    pub fn wants_league(&self) -> bool {
        self.games.iter().any(|g| !g.mode.is_empty())
    }

    /// Index of the game the probe shows, before League mode is resolved.
    pub fn find(&self, probe: &Probe) -> Option<usize> {
        for w in &probe.windows {
            for (i, g) in self.games.iter().enumerate() {
                if !g.title.is_empty()
                    && w.title.contains(&g.title)
                    && (g.process.is_empty() || g.process.contains(&w.proc))
                {
                    return Some(i);
                }
            }
        }
        for pr in &probe.procs {
            for (i, g) in self.games.iter().enumerate() {
                if g.title.is_empty()
                    && g.process.contains(&pr.name)
                    && (g.match_text.is_empty() || pr.cmdline.contains(&g.match_text))
                {
                    return Some(i);
                }
            }
        }
        None
    }

    /// For a WoW game, the entry of the client flavour that runs (added on first sight).
    /// A game with the flavour's exact name, user-made or not, is used as is.
    pub fn find_flavour(&mut self, probe: &Probe, idx: usize) -> usize {
        let g = &self.games[idx];
        if !g.flavours {
            return idx;
        }
        let Some(pr) = probe.procs.iter().find(|pr| {
            g.process.contains(&pr.name)
                && (g.match_text.is_empty() || pr.cmdline.contains(&g.match_text))
        }) else {
            return idx;
        };
        let name = wow_flavour_name(&pr.exe);
        let id = game_id(&name);
        if let Some(i) = self.games.iter().position(|o| o.id == id) {
            return i;
        }
        let mut flavour = g.clone();
        flavour.id = id;
        flavour.name = name;
        flavour.flavours = false;
        self.flav.insert(id, (flavour.name.clone(), g.name.clone()));
        self.games.push(flavour);
        self.games.len() - 1
    }

    /// True when game `idx` needs the League mode and the last answer is stale.
    pub fn mode_wanted(&self, idx: usize, now: f64) -> bool {
        !self.games[idx].mode.is_empty() && now - self.league.1 >= LEAGUE_EVERY_S
    }

    pub fn set_league(&mut self, mode: &str, now: f64) {
        self.league = (mode.to_string(), now);
    }

    /// League and TFT are one program: map to the entry matching the known mode.
    fn pick_mode(&self, idx: usize) -> u32 {
        let g = &self.games[idx];
        if g.mode.is_empty() {
            return g.id;
        }
        for other in &self.games {
            if other.mode == self.league.0 && other.process.iter().any(|p| g.process.contains(p)) {
                return other.id;
            }
        }
        g.id
    }

    /// Update the session clock for what `found` says is running; returns the game now
    /// playing. Port of `CustomGames.poll`.
    pub fn apply(&mut self, now: f64, found: Option<usize>) -> Option<Playing> {
        if self.games.is_empty() {
            return None;
        }
        let running = found.map_or(0, |i| self.pick_mode(i));
        if running != self.running {
            if self.running != 0 {
                self.stop(now);
            }
            self.running = running;
            self.start = now;
            if running != 0 {
                log::info!(
                    "custom game started: {}",
                    self.game(running).map_or("", |g| &g.name)
                );
            }
        }
        if running == 0 {
            return None;
        }
        let st = self.state.entry(running).or_default();
        st.last = now as i64;
        let (last_total, name) = (st.total_s, self.game(running)?.name.clone());
        if now - self.saved_at >= SAVE_EVERY_S {
            self.save(now);
        }
        let session = ((now - self.start) as i64).max(0);
        Some(Playing {
            appid: running,
            name,
            session_s: session,
            total_min: (last_total + session) / 60,
        })
    }

    fn stop(&mut self, now: f64) {
        let session = ((now - self.start) as i64).max(0);
        let st = self.state.entry(self.running).or_default();
        st.total_s += session;
        st.last = now as i64;
        self.save(now);
    }

    /// The service is stopping: add the running game's session and save.
    pub fn finish(&mut self, now: f64) {
        if self.running != 0 {
            self.stop(now);
            self.running = 0;
        } else {
            self.save(now);
        }
    }

    /// Save the history now.
    pub fn save(&mut self, now: f64) {
        self.saved_at = now;
        let Some(path) = &self.state_file else { return };
        let map: serde_json::Map<String, Value> = self
            .state
            .iter()
            .map(|(k, v)| {
                let mut e = serde_json::json!({ "last": v.last, "total_s": v.total_s });
                if let Some((name, base)) = self.flav.get(k) {
                    e["name"] = name.as_str().into();
                    e["base"] = base.as_str().into();
                }
                (k.to_string(), e)
            })
            .collect();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(path, Value::Object(map).to_string()) {
            log::debug!("could not save custom games: {e}");
        }
    }

    /// Configured games played before, newest first.
    pub fn recent(&self) -> Vec<Recent> {
        let mut out: Vec<Recent> = self
            .games
            .iter()
            .filter_map(|g| {
                let st = self.state.get(&g.id)?;
                (st.last > 0).then(|| Recent {
                    appid: g.id,
                    name: g.name.clone(),
                    last: st.last,
                    total_min: st.total_s / 60,
                })
            })
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.last));
        out
    }
}

/// Read the history file; empty when missing or broken (Python `_load`).
pub fn load_state(path: &Path) -> HashMap<u32, Stat> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for (k, v) in map {
        let Ok(id) = k.parse::<u32>() else {
            return HashMap::new();
        };
        let num = |key: &str| v.get(key).and_then(Value::as_i64).unwrap_or(0);
        out.insert(
            id,
            Stat {
                last: num("last"),
                total_s: num("total_s"),
            },
        );
    }
    out
}

/// Flavour games saved in the history file (`name` and `base` keys): id -> (name, base).
pub fn load_flavours(path: &Path) -> HashMap<u32, (String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for (k, v) in map {
        let (Ok(id), Some(name), Some(base)) = (
            k.parse::<u32>(),
            v.get("name").and_then(Value::as_str),
            v.get("base").and_then(Value::as_str),
        ) else {
            continue;
        };
        out.insert(id, (name.to_string(), base.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cg(name: &str, proc_: &[&str]) -> CustomGame {
        CustomGame {
            name: name.into(),
            process: proc_.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn no_launch(_: &str) -> String {
        String::new()
    }

    fn tracker(games: Vec<Game>) -> Tracker {
        Tracker::new(games, None)
    }

    #[test]
    fn crc32_and_ids_match_python() {
        // zlib.crc32(b"123456789") == 0xCBF43926
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        // zlib.crc32(b"minecraft") & 0x3FFFFFFF | 0xC0000000, name is trimmed and lowered
        let expect = ID_BASE | (crc32(b"minecraft") & 0x3FFF_FFFF);
        assert_eq!(game_id("  Minecraft "), expect);
        assert!(is_custom(game_id("x")));
        assert!(!is_custom(570));
    }

    #[test]
    fn user_game_overrides_preset_by_name() {
        let cfg = Games {
            custom: vec![cg("minecraft", &["mc.exe"])],
            ..Games::default()
        };
        let games = load_games(&cfg, &no_launch);
        let mc: Vec<_> = games
            .iter()
            .filter(|g| g.name.to_lowercase() == "minecraft")
            .collect();
        assert_eq!(mc.len(), 1);
        assert_eq!(mc[0].process, vec!["mc.exe"]);
        // user entries come first; other presets are still there
        assert_eq!(games[0].name, "minecraft");
        assert!(games.iter().any(|g| g.name == "Roblox"));
    }

    #[test]
    fn presets_can_be_disabled_by_name_or_wholesale() {
        let cfg = Games {
            presets_disabled: vec!["roblox".into(), " VALORANT ".into()],
            ..Games::default()
        };
        let games = load_games(&cfg, &no_launch);
        assert!(!games
            .iter()
            .any(|g| g.name == "Roblox" || g.name == "VALORANT"));
        assert!(games.iter().any(|g| g.name == "Fortnite"));

        let off = Games {
            presets: false,
            ..Games::default()
        };
        assert!(load_games(&off, &no_launch).is_empty());
    }

    #[test]
    fn invalid_user_games_skipped_and_fields_normalised() {
        let cfg = Games {
            presets: false,
            custom: vec![
                cg("", &["a.exe"]),
                cg("NoProc", &[]),
                CustomGame {
                    name: "Titled".into(),
                    title: " Some Window ".into(),
                    ..Default::default()
                },
                CustomGame {
                    name: " Gäme ".into(),
                    process: vec!["  Game.EXE ".into(), " ".into()],
                    match_text: " MC ".into(),
                    ..Default::default()
                },
            ],
            ..Games::default()
        };
        let games = load_games(&cfg, &no_launch);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].title, "some window");
        assert_eq!(games[1].name, "Gme");
        assert_eq!(games[1].id, game_id("Gäme"));
        assert_eq!(games[1].process, vec!["game.exe"]);
        assert_eq!(games[1].match_text, "mc");
    }

    #[test]
    fn preset_launch_and_image() {
        let resolve = |t: &str| resolve_launch(t, "C:/Riot/rc.exe", &|_| false);
        let entries = preset_entries(&[], &HashSet::new(), &resolve);
        let lol = entries
            .iter()
            .find(|e| e.name == "League of Legends")
            .unwrap();
        assert_eq!(
            lol.launch,
            "\"C:/Riot/rc.exe\" --launch-product=league_of_legends --launch-patchline=live"
        );
        assert_eq!(lol.mode, "lol");
        let rl = entries.iter().find(|e| e.name == "Rocket League").unwrap();
        assert!(rl.image.ends_with("/apps/252950/header.jpg"));
        assert!(rl.launch.starts_with("com.epicgames.launcher://"));
        // Minecraft's launcher path is not there -> no launch target
        let mc = entries.iter().find(|e| e.name == "Minecraft").unwrap();
        assert_eq!(mc.launch, "");
        assert_eq!(mc.match_text, "minecraft");
        assert_eq!(entries.len(), 24);
    }

    #[test]
    fn resolve_launch_cases() {
        let yes = |_: &str| true;
        assert_eq!(resolve_launch("{riot} -x", "", &yes), "");
        assert_eq!(resolve_launch("x://y", "", &|_| false), "x://y");
        assert_eq!(resolve_launch("C:/a.exe", "", &|_| false), "");
        assert_eq!(resolve_launch("C:/a.exe", "", &yes), "C:/a.exe");
    }

    #[test]
    fn launch_plan() {
        assert_eq!(
            plan_launch("steam://x", false),
            LaunchAction::Open("steam://x".into())
        );
        assert_eq!(
            plan_launch("C:/a.exe", true),
            LaunchAction::Open("C:/a.exe".into())
        );
        assert_eq!(
            plan_launch("\"C:/a.exe\" -x", false),
            LaunchAction::Shell("\"C:/a.exe\" -x".into())
        );
    }

    #[test]
    fn expand_percent_vars() {
        std::env::set_var("DESKHUB_TEST_VAR", "XY");
        assert_eq!(expand_path("%DESKHUB_TEST_VAR%/a"), "XY/a");
        assert_eq!(
            expand_path("%NO_SUCH_DESKHUB_VAR%/a"),
            "%NO_SUCH_DESKHUB_VAR%/a"
        );
        assert_eq!(expand_path("100%"), "100%");
    }

    fn game(name: &str, procs: &[&str], mt: &str, title: &str, mode: &str) -> Game {
        Game {
            id: game_id(name),
            name: name.into(),
            process: procs.iter().map(|s| s.to_string()).collect(),
            match_text: mt.into(),
            title: title.into(),
            image: String::new(),
            launch: String::new(),
            mode: mode.into(),
            flavours: false,
        }
    }

    fn proc_(name: &str, cmd: &str) -> ProcInfo {
        ProcInfo {
            name: name.into(),
            cmdline: cmd.into(),
            exe: String::new(),
        }
    }

    #[test]
    fn wow_flavour_names() {
        let n = |p: &str| wow_flavour_name(p);
        let base = "C:\\Program Files (x86)\\World of Warcraft\\";
        assert_eq!(n(&format!("{base}_retail_\\Wow.exe")), "World of Warcraft");
        assert_eq!(
            n(&format!("{base}_classic_\\WowClassic.exe")),
            "WoW Classic"
        );
        assert_eq!(
            n(&format!("{base}_classic_era_\\WowClassic.exe")),
            "WoW Classic Era"
        );
        assert_eq!(
            n(&format!("{base}_anniversary_\\Wow.exe")),
            "WoW Anniversary"
        );
        assert_eq!(n(&format!("{base}_ptr_\\WowT.exe")), "WoW PTR");
        assert_eq!(n(&format!("{base}_beta_\\WowB.exe")), "WoW Beta");
        assert_eq!(n(&format!("{base}_forever_\\Wow.exe")), "WoW Forever");
        assert_eq!(n("/g/wow/_my_new_flavour_/wow.exe"), "WoW My New Flavour");
        assert_eq!(n(&format!("{base}Wow.exe")), "World of Warcraft");
        assert_eq!(n(""), "World of Warcraft");
    }

    #[test]
    fn wow_flavours_are_separate_games() {
        let mut base = game("World of Warcraft", &["wow.exe"], "", "", "");
        base.id = game_id("World of Warcraft");
        base.flavours = true;
        let mut t = tracker(vec![base]);
        let wow = |exe: &str| Probe {
            procs: vec![ProcInfo {
                name: "wow.exe".into(),
                cmdline: String::new(),
                exe: exe.into(),
            }],
            ..Probe::default()
        };
        let p1 = wow("c:\\wow\\_retail_\\wow.exe");
        let i = t.find(&p1).unwrap();
        let i = t.find_flavour(&p1, i);
        assert_eq!(t.games()[i].name, "World of Warcraft");
        let p2 = wow("c:\\wow\\_forever_\\wow.exe");
        let i = t.find(&p2).unwrap();
        let i = t.find_flavour(&p2, i);
        assert_eq!(t.games()[i].name, "WoW Forever");
        assert_eq!(t.games().len(), 2);
        assert_ne!(t.games()[0].id, t.games()[1].id);
        // Seen again: no duplicate.
        let i = t.find(&p2).unwrap();
        t.find_flavour(&p2, i);
        assert_eq!(t.games().len(), 2);
        // Settings reload keeps the flavour game.
        let keep = vec![t.games()[0].clone()];
        t.set_games(keep, 0.0);
        assert_eq!(t.games().len(), 2);
    }

    #[test]
    fn find_by_process_command_line_and_title() {
        let t = tracker(vec![
            game("Minecraft", &["javaw.exe"], "minecraft", "", ""),
            game("Other", &["other.exe"], "", "", ""),
            game("Win", &[], "", "cool game", ""),
            game("WinP", &["x.exe"], "", "tool", ""),
        ]);
        let probe = Probe {
            procs: vec![
                proc_("javaw.exe", "javaw -jar idea.jar"),
                proc_("other.exe", ""),
            ],
            ..Default::default()
        };
        // javaw without "minecraft" on the command line is not Minecraft; Other matches.
        assert_eq!(t.find(&probe), Some(1));
        let probe = Probe {
            procs: vec![proc_("javaw.exe", "javaw -cp minecraft.jar")],
            ..Default::default()
        };
        assert_eq!(t.find(&probe), Some(0));
        let win = |title: &str, p: &str| WinInfo {
            title: title.into(),
            proc: p.into(),
        };
        let probe = Probe {
            windows: vec![win("my cool game v2", "z.exe")],
            ..Default::default()
        };
        assert_eq!(t.find(&probe), Some(2));
        // title + process: only when the window belongs to that program
        let probe = Probe {
            windows: vec![win("a tool window", "y.exe")],
            ..Default::default()
        };
        assert_eq!(t.find(&probe), None);
        let probe = Probe {
            windows: vec![win("a tool window", "x.exe")],
            ..Default::default()
        };
        assert_eq!(t.find(&probe), Some(3));
        // title games ignore plain processes
        assert_eq!(t.wanted_procs().len(), 2);
        assert!(t.wants_titles());
    }

    #[test]
    fn league_and_tft_share_a_process() {
        let mut t = tracker(vec![
            game(
                "League of Legends",
                &["league of legends.exe"],
                "",
                "",
                "lol",
            ),
            game(
                "Teamfight Tactics",
                &["league of legends.exe"],
                "",
                "",
                "tft",
            ),
        ]);
        let probe = Probe {
            procs: vec![proc_("league of legends.exe", "")],
            ..Default::default()
        };
        let idx = t.find(&probe);
        assert_eq!(idx, Some(0));
        assert!(t.mode_wanted(0, 1000.0));
        t.set_league("tft", 1000.0);
        assert!(!t.mode_wanted(0, 1010.0));
        assert!(t.mode_wanted(0, 1031.0));
        let p = t.apply(1000.0, idx).unwrap();
        assert_eq!(p.name, "Teamfight Tactics");
        assert!(t.wants_league());
    }

    #[test]
    fn league_session_parsing() {
        let tft = serde_json::json!({"gameData": {"queue": {"gameMode": "TFT"}}});
        assert_eq!(league_mode_from_session(&tft), "tft");
        let classic = serde_json::json!({"gameData": {"queue": {"gameMode": "CLASSIC"}}});
        assert_eq!(league_mode_from_session(&classic), "lol");
        let map = serde_json::json!({"map": {"gameMode": "tft"}});
        assert_eq!(league_mode_from_session(&map), "tft");
        assert_eq!(league_mode_from_session(&serde_json::json!({})), "lol");
    }

    #[test]
    fn sessions_totals_and_recent() {
        let mut t = tracker(vec![
            game("A", &["a.exe"], "", "", ""),
            game("B", &["b.exe"], "", "", ""),
        ]);
        assert!(t.recent().is_empty());
        assert!(t.apply(1000.0, None).is_none());
        let p = t.apply(1000.0, Some(0)).unwrap();
        assert_eq!((p.session_s, p.total_min), (0, 0));
        let p = t.apply(1130.0, Some(0)).unwrap();
        assert_eq!((p.session_s, p.total_min), (130, 2));
        // A stops, B starts: A's session is added to its total
        let p = t.apply(1200.0, Some(1)).unwrap();
        assert_eq!(p.appid, game_id("B"));
        assert_eq!(p.session_s, 0);
        t.apply(1210.0, Some(1));
        let r = t.recent();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].name, "B"); // newest first
        assert_eq!(r[1].appid, game_id("A"));
        assert_eq!(r[1].total_min, 3); // 200 s
        assert!(t.apply(1300.0, None).is_none());
        assert_eq!(
            t.recent().iter().find(|x| x.name == "B").unwrap().total_min,
            1
        );
    }

    #[test]
    fn state_file_round_trip_python_format() {
        let dir = std::env::temp_dir().join(format!("deskhub-games-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("custom_games.json");
        let games = vec![game("A", &["a.exe"], "", "", "")];
        let mut t = Tracker::new(games.clone(), Some(file.clone()));
        t.apply(5000.0, Some(0));
        t.apply(5100.0, None);
        let text = std::fs::read_to_string(&file).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        let entry = &v[game_id("A").to_string()];
        assert_eq!(entry["total_s"], 100);
        assert_eq!(entry["last"], 5100);
        let t2 = Tracker::new(games, Some(file.clone()));
        assert_eq!(t2.recent()[0].total_min, 1);
        std::fs::write(&file, "not json").unwrap();
        assert!(load_state(&file).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flavour_games_survive_restart() {
        let dir = std::env::temp_dir().join(format!("deskhub-flav-{}", std::process::id()));
        let file = dir.join("state.json");
        let mk = || {
            let mut base = game("World of Warcraft", &["wow.exe"], "", "", "");
            base.flavours = true;
            base
        };
        let mut t = Tracker::new(vec![mk()], Some(file.clone()));
        let probe = Probe {
            procs: vec![ProcInfo {
                name: "wow.exe".into(),
                cmdline: String::new(),
                exe: "c:\\wow\\_forever_\\wow.exe".into(),
            }],
            ..Probe::default()
        };
        let i = t.find(&probe).unwrap();
        let i = t.find_flavour(&probe, i);
        t.apply(1000.0, Some(i));
        t.finish(1120.0);
        // Restart: same settings, WoW not running.
        let t2 = Tracker::new(vec![mk()], Some(file.clone()));
        assert_eq!(t2.games().len(), 2);
        let r = t2.recent();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "WoW Forever");
        assert_eq!(r[0].last, 1120);
        assert_eq!(r[0].total_min, 2);
        // Base preset off: flavour is not recreated.
        let t3 = Tracker::new(vec![], Some(file));
        assert!(t3.games().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
