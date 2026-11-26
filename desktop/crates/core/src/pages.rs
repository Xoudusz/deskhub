//! Port of companion/deskhub/pages.py: the page list, the night schedule, the layout
//! and home messages sent to the board, and the home grid rules.

use std::collections::HashMap;

use log::warn;
use serde_json::{json, Value};

use crate::settings::{GridItem, NightStyle, Settings, DIM_DEFAULT};

/// Every page the board knows, pages.py ALL_PAGES (plus "plex": the desktop app only).
pub const ALL_PAGES: &[&str] = &[
    "home",
    "stats",
    "ai",
    "calendar",
    "weather",
    "media",
    "printer",
    "shortcuts",
    "timer",
    "games",
    "network",
    "github",
    "plex",
];

/// Navigation order, pages.py DEFAULT_ORDER (Shortcuts last).
pub const DEFAULT_ORDER: &[&str] = &[
    "home",
    "stats",
    "ai",
    "calendar",
    "weather",
    "media",
    "plex",
    "printer",
    "timer",
    "games",
    "network",
    "github",
    "shortcuts",
];

/// Pages on with no [pages] list and no printer, games or github config.
pub const DEFAULT_PAGES: &[&str] = &[
    "home",
    "stats",
    "ai",
    "calendar",
    "weather",
    "media",
    "timer",
    "network",
    "shortcuts",
];

pub const GRID_COLS: i64 = 5;
pub const GRID_ROWS: i64 = 3;

/// Inclusive (lo, hi) range.
type Range = (i64, i64);

/// Sizes each home widget supports: (widths, heights) in grid cells, pages.py HOME_SIZES.
const HOME_SIZES: &[(&str, Range, Range)] = &[
    ("clock", (2, 5), (1, 2)),
    ("claude", (1, 5), (1, 1)),
    ("events", (2, 5), (1, 3)),
    ("weather", (1, 5), (1, 1)),
    ("media", (2, 5), (1, 1)),
    ("stats", (1, 5), (1, 1)),
    ("printer", (2, 5), (1, 1)),
    ("game", (2, 5), (1, 1)),
    ("net", (1, 5), (1, 1)),
    ("github", (2, 5), (1, 2)),
];

/// Legacy column defaults (no [home] grid): the slim top bar layout.
pub const DEFAULT_LEFT: &[&str] = &["claude", "events"];
pub const DEFAULT_RIGHT: &[&str] = &["weather", "media", "stats"];
pub const DEFAULT_RIGHT_WITH_PRINTER: &[&str] = &["weather", "media", "printer"];

fn sizes(name: &str) -> Option<(Range, Range)> {
    HOME_SIZES
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, w, h)| (*w, *h))
}

pub fn is_home_widget(name: &str) -> bool {
    sizes(name).is_some()
}

/// Sizes a widget supports, like ["2x1", "3x1"].
pub fn widget_sizes(name: &str) -> Vec<String> {
    let Some((ws, hs)) = sizes(name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for h in hs.0..=hs.1 {
        for w in ws.0..=ws.1 {
            out.push(format!("{w}x{h}"));
        }
    }
    out
}

/// The default home grid: the two-column layout, converted.
pub fn default_grid() -> Vec<GridItem> {
    let left: Vec<String> = DEFAULT_LEFT.iter().map(|s| s.to_string()).collect();
    let right: Vec<String> = DEFAULT_RIGHT.iter().map(|s| s.to_string()).collect();
    columns_to_grid(&left, &right)
}

/// Enabled pages in navigation order: home first, then the settings list (unknown
/// names and repeats dropped).
pub fn enabled_pages(s: &Settings) -> Vec<String> {
    let mut order = vec!["home".to_string()];
    for name in &s.pages.enabled {
        let name = name.to_lowercase();
        if ALL_PAGES.contains(&name.as_str()) && !order.contains(&name) {
            order.push(name);
        }
    }
    order
}

/// "23:00", "7:30", "7", "7pm", "11pm", "7 a.m." -> minutes after midnight.
pub fn parse_hhmm(text: &str) -> Option<i64> {
    let s = text.trim().to_lowercase();
    let (body, meridiem) = split_meridiem(&s);
    let body = body.trim();
    let (hour, minute) = match body.split_once(':') {
        Some((h, m)) => (h, Some(m)),
        None => (body, None),
    };
    let mut h = digits(hour, 1, 2)?;
    let m = match minute {
        Some(m) => digits(m, 2, 2)?,
        None => 0,
    };
    if let Some(is_pm) = meridiem {
        if !(1..=12).contains(&h) {
            return None;
        }
        h = h % 12 + if is_pm { 12 } else { 0 };
    }
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

fn digits(s: &str, min: usize, max: usize) -> Option<i64> {
    if s.len() < min || s.len() > max || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Strips a trailing am/pm (any of the forms pages.py accepts). Returns Some(is_pm) if found.
fn split_meridiem(s: &str) -> (&str, Option<bool>) {
    const SUFFIXES: [(&str, bool); 8] = [
        ("a.m.", false),
        ("a.m", false),
        ("am.", false),
        ("am", false),
        ("p.m.", true),
        ("p.m", true),
        ("pm.", true),
        ("pm", true),
    ];
    for (suffix, is_pm) in SUFFIXES {
        if let Some(body) = s.strip_suffix(suffix) {
            return (body, Some(is_pm));
        }
    }
    (s, None)
}

/// "23:00-07:00" -> (start, end) in minutes, or None when off or invalid.
pub fn night_schedule(raw: &str) -> Option<(i64, i64)> {
    let (a, b) = raw.trim().split_once('-')?;
    let start = parse_hhmm(a)?;
    let end = parse_hhmm(b)?;
    (start != end).then_some((start, end))
}

/// The layout message the board restarts from (night -1/-1 when off).
pub fn layout_message(s: &Settings) -> Value {
    let (night_start, night_end) = night_schedule(&s.display.night).unwrap_or((-1, -1));
    json!({
        "type": "layout",
        "pages": enabled_pages(s),
        "ordered": true,
        "nav": s.pages.nav.as_str(),
        "pc_sleep": s.display.pc_sleep.as_str(),
        "night_start": night_start,
        "night_end": night_end,
        // The board dims instead of going dark when the style is "off" with a night
        // brightness above 0 (display.c dark_mode): that is what "dim" sends.
        "night_style": match s.display.night_style {
            NightStyle::Dim => "off",
            other => other.as_str(),
        },
        "brightness": s.display.brightness.clamp(1, 100),
        "night_brightness": match s.display.night_style {
            NightStyle::Dim if s.display.night_brightness <= 0 => DIM_DEFAULT,
            NightStyle::Dim => s.display.night_brightness.clamp(1, 100),
            _ => s.display.night_brightness.clamp(0, 100),
        },
        "seconds": s.display.clock_seconds,
    })
}

/// Why each grid entry cannot be used (empty = all fine), pages.py grid_problem.
pub fn grid_problem(items: &[GridItem]) -> Vec<String> {
    let mut taken: HashMap<(i64, i64), String> = HashMap::new();
    let mut problems = Vec::new();
    for it in items {
        let Some((ws, hs)) = sizes(&it.widget) else {
            problems.push(format!("{}: no such widget", it.widget));
            continue;
        };
        if !(ws.0..=ws.1).contains(&it.w) || !(hs.0..=hs.1).contains(&it.h) {
            problems.push(format!(
                "{}: {}x{} isn't a size it supports ({})",
                it.widget,
                it.w,
                it.h,
                widget_sizes(&it.widget).join(", ")
            ));
            continue;
        }
        if it.x < 0 || it.y < 0 || it.x + it.w > GRID_COLS || it.y + it.h > GRID_ROWS {
            problems.push(format!(
                "{}: at {},{} a {}x{} widget runs off the {}x{} grid",
                it.widget, it.x, it.y, it.w, it.h, GRID_COLS, GRID_ROWS
            ));
            continue;
        }
        let cells: Vec<(i64, i64)> = (it.x..it.x + it.w)
            .flat_map(|cx| (it.y..it.y + it.h).map(move |cy| (cx, cy)))
            .collect();
        if let Some(clash) = cells.iter().find_map(|c| taken.get(c)) {
            problems.push(format!("{}: overlaps {}", it.widget, clash));
            continue;
        }
        for c in cells {
            taken.insert(c, it.widget.clone());
        }
    }
    problems
}

/// Keep the entries that pass the grid rules, in order; the others are logged and dropped.
pub fn validate_grid(items: &[GridItem]) -> Vec<GridItem> {
    let mut good: Vec<GridItem> = Vec::new();
    for it in items {
        let mut candidate = good.clone();
        candidate.push(it.clone());
        match grid_problem(&candidate).last() {
            Some(problem) => warn!("[home] grid: {problem}; leaving it out"),
            None => good.push(it.clone()),
        }
    }
    good
}

/// Today's two columns on the grid: left 3 cells wide, right 2, one row per widget;
/// Up next (events) takes the rest of its column.
pub fn columns_to_grid(left: &[String], right: &[String]) -> Vec<GridItem> {
    let mut grid = Vec::new();
    for (x, w, col) in [(0i64, 3i64, left), (3, 2, right)] {
        let mut y = 0i64;
        for (i, name) in col.iter().enumerate() {
            if y >= GRID_ROWS {
                break;
            }
            let Some((ws, hs)) = sizes(name) else {
                continue;
            };
            let rest = GRID_ROWS - y;
            let h = if name == "events" && i == col.len() - 1 {
                rest
            } else {
                1
            };
            let h = h.min(hs.1);
            if (ws.0..=ws.1).contains(&w) {
                grid.push(GridItem {
                    widget: name.clone(),
                    x,
                    y,
                    w,
                    h,
                });
            }
            y += h;
        }
    }
    grid
}

/// The home message: grid (validated), plus the left/right columns older firmware reads.
pub fn home_message(s: &Settings) -> Value {
    let grid = validate_grid(&s.home.grid);
    let mut by_row: Vec<&GridItem> = grid.iter().collect();
    by_row.sort_by_key(|g| g.y);
    let left: Vec<&str> = by_row
        .iter()
        .filter(|g| g.x < 3)
        .map(|g| g.widget.as_str())
        .collect();
    let right: Vec<&str> = by_row
        .iter()
        .filter(|g| g.x >= 3)
        .map(|g| g.widget.as_str())
        .collect();
    let grid_json: Vec<Value> = grid
        .iter()
        .map(|g| json!({"widget": g.widget, "x": g.x, "y": g.y, "w": g.w, "h": g.h}))
        .collect();
    json!({
        "type": "home",
        "left": left,
        "right": right,
        "header": s.home.header,
        "grid": grid_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Nav, NightStyle, PcSleep};

    fn item(widget: &str, x: i64, y: i64, w: i64, h: i64) -> GridItem {
        GridItem {
            widget: widget.into(),
            x,
            y,
            w,
            h,
        }
    }

    #[test]
    fn hhmm_formats_match_pages_py() {
        assert_eq!(parse_hhmm("23:00"), Some(1380));
        assert_eq!(parse_hhmm("7:30"), Some(450));
        assert_eq!(parse_hhmm("7"), Some(420));
        assert_eq!(parse_hhmm("7pm"), Some(1140));
        assert_eq!(parse_hhmm("11pm"), Some(1380));
        assert_eq!(parse_hhmm("12am"), Some(0));
        assert_eq!(parse_hhmm("12pm"), Some(720));
        assert_eq!(parse_hhmm("7 a.m."), Some(420));
        assert_eq!(parse_hhmm(" 7:30 PM "), Some(1170));
        assert_eq!(parse_hhmm("13pm"), None);
        assert_eq!(parse_hhmm("0am"), None);
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("7:60"), None);
        assert_eq!(parse_hhmm("7:5"), None);
        assert_eq!(parse_hhmm("abc"), None);
    }

    #[test]
    fn night_schedule_parses_ranges() {
        assert_eq!(night_schedule("23:00-07:00"), Some((1380, 420)));
        assert_eq!(night_schedule("11pm-7am"), Some((1380, 420)));
        assert_eq!(night_schedule(""), None);
        assert_eq!(night_schedule("7-7"), None);
        assert_eq!(night_schedule("bad"), None);
        assert_eq!(night_schedule("23:00-nope"), None);
    }

    #[test]
    fn default_layout_message() {
        let s = Settings::default();
        assert_eq!(
            layout_message(&s),
            json!({
                "type": "layout",
                "pages": ["home", "stats", "ai", "calendar", "weather", "media", "timer", "network", "shortcuts"],
                "ordered": true,
                "nav": "left",
                "pc_sleep": "off",
                "night_start": -1,
                "night_end": -1,
                "night_style": "off",
                "brightness": 100,
                "night_brightness": 0,
                "seconds": false,
            })
        );
    }

    #[test]
    fn layout_message_with_night_and_clamp() {
        let mut s = Settings::default();
        s.display.night = "23:00-07:00".into();
        s.display.night_style = NightStyle::Clock;
        s.display.pc_sleep = PcSleep::Stay;
        s.pages.nav = Nav::Bottom;
        s.display.brightness = 250;
        s.display.night_brightness = -4;
        let v = layout_message(&s);
        assert_eq!(v["night_start"], 1380);
        assert_eq!(v["night_end"], 420);
        assert_eq!(v["night_style"], "clock");
        assert_eq!(v["pc_sleep"], "none");
        assert_eq!(v["nav"], "bottom");
        assert_eq!(v["brightness"], 100);
        assert_eq!(v["night_brightness"], 0);
    }

    #[test]
    fn default_home_grid_matches_columns() {
        let g = default_grid();
        assert_eq!(
            g,
            vec![
                item("claude", 0, 0, 3, 1),
                item("events", 0, 1, 3, 2),
                item("weather", 3, 0, 2, 1),
                item("media", 3, 1, 2, 1),
                item("stats", 3, 2, 2, 1),
            ]
        );
        assert_eq!(Settings::default().home.grid, g);
    }

    #[test]
    fn grid_rules_drop_bad_entries() {
        let problems = grid_problem(&[
            item("nope", 0, 0, 1, 1),
            item("weather", 0, 0, 6, 1),
            item("weather", 4, 0, 2, 1),
            item("weather", 0, 0, 1, 1),
            item("media", 0, 0, 2, 1),
        ]);
        assert_eq!(problems.len(), 4);
        assert!(problems[0].contains("no such widget"));
        assert!(problems[1].contains("isn't a size it supports"));
        assert!(problems[2].contains("runs off"));
        assert!(problems[3].contains("overlaps weather"));

        let good = validate_grid(&[
            item("weather", 0, 0, 2, 1),
            item("weather", 0, 0, 1, 1),
            item("media", 2, 0, 2, 1),
        ]);
        assert_eq!(
            good,
            vec![item("weather", 0, 0, 2, 1), item("media", 2, 0, 2, 1)]
        );
    }

    #[test]
    fn home_message_columns_and_grid() {
        let s = Settings::default();
        let v = home_message(&s);
        assert_eq!(v["type"], "home");
        assert_eq!(v["header"], true);
        assert_eq!(v["left"], json!(["claude", "events"]));
        assert_eq!(v["right"], json!(["weather", "media", "stats"]));
        assert_eq!(
            v["grid"][1],
            json!({"widget": "events", "x": 0, "y": 1, "w": 3, "h": 2})
        );
    }

    #[test]
    fn columns_with_printer_and_full_height_events() {
        let left = vec!["events".to_string()];
        let right = vec![
            "weather".to_string(),
            "printer".to_string(),
            "media".to_string(),
            "stats".to_string(),
        ];
        let g = columns_to_grid(&left, &right);
        assert_eq!(
            g,
            vec![
                item("events", 0, 0, 3, 3),
                item("weather", 3, 0, 2, 1),
                item("printer", 3, 1, 2, 1),
                item("media", 3, 2, 2, 1),
            ]
        );
    }

    #[test]
    fn layout_carries_clock_seconds() {
        let mut s = Settings::default();
        s.display.clock_seconds = true;
        assert_eq!(layout_message(&s)["seconds"], serde_json::json!(true));
    }

    #[test]
    fn dim_night_style_keeps_pages_on() {
        let mut s = Settings::default();
        s.display.night_style = NightStyle::Dim;
        let v = layout_message(&s);
        assert_eq!(v["night_style"], "off");
        assert_eq!(v["night_brightness"], DIM_DEFAULT);
        s.display.night_brightness = 35;
        assert_eq!(layout_message(&s)["night_brightness"], 35);
    }
}
