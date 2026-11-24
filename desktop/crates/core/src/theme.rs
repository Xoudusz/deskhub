//! The board palette for each app theme ("theme" message). The colours come from the
//! app's src/styles.css; the board stores them and restarts once when they change.

use serde_json::{json, Value};

use crate::settings::{BoardTheme, Settings, Theme};

/// The board's own look (firmware ui_internal.h defaults).
const CLASSIC: Palette = Palette {
    bg: 0x0d0e11,
    rail: 0x111216,
    card: 0x16181d,
    card_hi: 0x262a32,
    border: 0x262a32,
    pressed: 0x30343d,
    text: 0xf3f4f6,
    muted: 0x9aa0aa,
    dim: 0x6b717c,
    accent: 0xffb224,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: u32,
    pub rail: u32,
    pub card: u32,
    pub card_hi: u32,
    pub border: u32,
    pub pressed: u32,
    pub text: u32,
    pub muted: u32,
    pub dim: u32,
    pub accent: u32,
}

/// styles.css variables per theme: bg, panel2 (rail), panel (card), panel3 (card_hi),
/// line (border), line2 (pressed), muted, accent. Text is the shared --fg.
fn app_palette(theme: Theme) -> Palette {
    let (bg, rail, card, card_hi, border, pressed, muted, accent) = match theme {
        Theme::Blue => (
            0x0a101c, 0x0f1727, 0x121b2e, 0x16223a, 0x22304d, 0x2f4370, 0x8a97b0, 0x3b8eea,
        ),
        Theme::Purple => (
            0x120b20, 0x160f27, 0x1b1230, 0x251a40, 0x3a2a58, 0x533c80, 0x8a97b0, 0x9a5cf5,
        ),
        Theme::Graphite => (
            0x111214, 0x17191c, 0x1c1e22, 0x24272c, 0x2c2f35, 0x42464f, 0x8a97b0, 0xaab4c3,
        ),
        Theme::Midnight => (
            0x05060a, 0x0a0c13, 0x0d0f18, 0x141829, 0x1d2030, 0x2c3150, 0x8a97b0, 0x7b8cff,
        ),
        Theme::Teal => (
            0x081517, 0x0c1c1e, 0x0f2224, 0x143032, 0x1d3a3c, 0x2a5457, 0x8a97b0, 0x1fb5a8,
        ),
        Theme::Rose => (
            0x16080f, 0x1d0d15, 0x241019, 0x301624, 0x4a2236, 0x66304b, 0x8a97b0, 0xe0569b,
        ),
    };
    Palette {
        bg,
        rail,
        card,
        card_hi,
        border,
        pressed,
        text: 0xe3e9f5,
        muted,
        dim: mix(muted, card, 0.45),
        accent,
    }
}

/// The palette the board should use for these settings.
pub fn palette(s: &Settings) -> Palette {
    let theme = match s.appearance.board_theme {
        BoardTheme::Classic => return CLASSIC,
        BoardTheme::Same => s.appearance.theme,
        BoardTheme::Purple => Theme::Purple,
        BoardTheme::Blue => Theme::Blue,
        BoardTheme::Graphite => Theme::Graphite,
        BoardTheme::Midnight => Theme::Midnight,
        BoardTheme::Teal => Theme::Teal,
        BoardTheme::Rose => Theme::Rose,
    };
    app_palette(theme)
}

/// {"type":"theme", slot: 0xRRGGBB, ...} for the firmware.
pub fn theme_message(s: &Settings) -> Value {
    let p = palette(s);
    json!({
        "type": "theme",
        "bg": p.bg,
        "rail": p.rail,
        "card": p.card,
        "card_hi": p.card_hi,
        "border": p.border,
        "pressed": p.pressed,
        "text": p.text,
        "muted": p.muted,
        "dim": p.dim,
        "accent": p.accent,
        // Dark text on light accents (amber, graphite), white on the rest.
        "on_accent": if luminance(p.accent) > 0.45 { p.card } else { 0xffffff },
        "accent_pressed": mix(p.accent, 0x000000, 0.15),
    })
}

/// `a` moved toward `b` by `t` (0..1), per channel.
fn mix(a: u32, b: u32, t: f64) -> u32 {
    let ch = |shift: u32| -> u32 {
        let x = f64::from((a >> shift) & 0xff);
        let y = f64::from((b >> shift) & 0xff);
        (x + (y - x) * t).round().clamp(0.0, 255.0) as u32
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Relative luminance 0..1 (sRGB, WCAG).
fn luminance(c: u32) -> f64 {
    let lin = |v: u32| {
        let x = f64::from(v) / 255.0;
        if x <= 0.039_28 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin((c >> 16) & 0xff) + 0.7152 * lin((c >> 8) & 0xff) + 0.0722 * lin(c & 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_board_theme() {
        let mut s = Settings::default();
        s.appearance.board_theme = BoardTheme::Classic;
        let v = theme_message(&s);
        assert_eq!(v["accent"], 0xffb224);
        assert_eq!(v["on_accent"], 0x16181d);
        assert_eq!(v["bg"], 0x0d0e11);
    }

    #[test]
    fn midnight_by_default_with_white_on_accent() {
        let v = theme_message(&Settings::default());
        assert_eq!(v["type"], "theme");
        assert_eq!(v["accent"], 0x7b8cff);
        assert_eq!(v["on_accent"], 0xffffff);
        assert_eq!(v["bg"], 0x05060a);
    }

    #[test]
    fn same_follows_the_app_and_own_theme_does_not() {
        let mut s = Settings::default();
        s.appearance.theme = Theme::Teal;
        assert_eq!(theme_message(&s)["accent"], 0x1fb5a8);
        s.appearance.board_theme = BoardTheme::Rose;
        assert_eq!(theme_message(&s)["accent"], 0xe0569b);
        s.appearance.theme = Theme::Blue;
        assert_eq!(theme_message(&s)["accent"], 0xe0569b);
    }

    #[test]
    fn board_theme_from_json() {
        let mut s = Settings::default();
        s.merge_patch(serde_json::json!({"appearance": {"board_theme": "classic"}}));
        assert_eq!(s.appearance.board_theme, BoardTheme::Classic);
        // Old settings files carry "board": true; it is ignored and the board follows the app.
        let old: Settings = serde_json::from_value(
            serde_json::json!({"appearance": {"theme": "rose", "board": true}}),
        )
        .unwrap();
        assert_eq!(old.appearance.board_theme, BoardTheme::Same);
    }

    #[test]
    fn mix_and_luminance() {
        assert_eq!(mix(0xffffff, 0x000000, 0.5), 0x808080);
        assert_eq!(mix(0x123456, 0x123456, 0.7), 0x123456);
        assert!(luminance(0xffffff) > 0.99 && luminance(0x000000) < 0.01);
    }
}
