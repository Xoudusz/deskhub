---
version: alpha
name: Desk Hub
description: >
  Calm, dark, glanceable desk display (800x480 LVGL board) plus its Tauri settings app.
  Flat cards on near-black, one warm accent for the current item and the primary action,
  colour otherwise only for data and status. Tokens below are the board's built-in
  "classic" look (firmware ui.c / ui_internal.h); app themes are in ## Colors.
colors:
  primary: "#ffb224"          # COL_ACCENT
  primary-pressed: "#d9971f"  # COL_ACCENT_PRESSED (= theme.rs: accent 15 % toward black)
  on-primary: "#16181d"       # COL_ON_ACCENT
  background: "#0d0e11"       # COL_BG
  rail: "#111216"             # COL_RAIL (nav rail / bottom dock)
  surface: "#16181d"          # COL_CARD
  surface-raised: "#262a32"   # COL_CARD_HI (pressed card, selected tab, bar track)
  border: "#262a32"           # COL_BORDER (1px hairline)
  pressed: "#30343d"          # COL_PRESSED
  text: "#f3f4f6"             # COL_TEXT
  text-muted: "#9aa0aa"       # COL_MUTED
  text-dim: "#6b717c"         # COL_DIM (icons/placeholders only, not body text)
  success: "#3dd68c"          # COL_GOOD
  warning: "#f3c14b"          # COL_WARN (yellow, apart from the amber accent)
  error: "#ff6b6b"            # COL_BAD
  data:
    series-1: "#6cb4ff"       # COL_ACCENT2 (CPU, upload, rain)
    series-2: "#5ed3c6"       # COL_CYAN (GPU, download)
    series-3: "#b49cff"       # COL_DATA3
    series-4: "#f79ad3"       # COL_DATA4
  brand:
    claude: "#d97757"       # COL_BRAND_CLAUDE
    codex: "#10a37f"        # COL_BRAND_CODEX
  github:                     # COL_GH_L1..L4
    level-1: "#0e4429"
    level-2: "#006d32"
    level-3: "#26a641"
    level-4: "#39d353"
  media-track: "#3a3f49"      # COL_TRACK
typography:
  caption:
    fontFamily: Inter
    fontSize: 14px
    fontWeight: 500
    letterSpacing: 2px
  body:
    fontFamily: Inter
    fontSize: 16px
    fontWeight: 500
  label:
    fontFamily: Inter
    fontSize: 20px
    fontWeight: 500
  title:
    fontFamily: Inter
    fontSize: 24px
    fontWeight: 500
  number:
    fontFamily: Inter
    fontSize: 28px
    fontWeight: 600
  page-title:
    fontFamily: Inter
    fontSize: 32px
    fontWeight: 600
  big-number:
    fontFamily: Inter
    fontSize: 48px
    fontWeight: 600
  timer:
    fontFamily: Inter
    fontSize: 72px
    fontWeight: 600
  clock:
    fontFamily: Inter
    fontSize: 128px
    fontWeight: 600
rounded:
  bar: 2px
  chart-bar: 6px
  key: 10px
  sm: 12px
  tab: 14px
  card: 16px
  pill: 28px
  full: 9999px
spacing:
  hairline: 1px
  xs: 8px
  sm: 12px
  md: 16px
  page: 20px
  rail-width: 88px
  dock-height: 68px
motion:
  press:
    duration: 0ms
  marquee:
    duration: 2500ms
    timingFunction: ease-in-out
components:
  card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    rounded: "{rounded.card}"
    padding: 16px
  card-caption:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text-muted}"
    typography: "{typography.caption}"
  rail:
    backgroundColor: "{colors.rail}"
    textColor: "{colors.text-muted}"
    width: 88px
  rail-tab-selected:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.text}"
    rounded: "{rounded.tab}"
    width: 60px
    height: 52px
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.label}"
    rounded: "{rounded.pill}"
    width: 180px
    height: 56px
  button-secondary:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.text}"
    typography: "{typography.label}"
    rounded: "{rounded.pill}"
    width: 180px
    height: 56px
  round-button:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    rounded: "{rounded.full}"
    size: 48px
  progress-bar:
    backgroundColor: "{colors.surface-raised}"
    rounded: "{rounded.full}"
    height: 8px
  badge:
    backgroundColor: "{colors.primary}"
    rounded: "{rounded.full}"
    size: 12px
---

## Overview

Desk Hub is a desk display: an 800x480 touch board (ESP32-S3 RGB panel, or ESP32-P4 with a
480x800 MIPI panel rotated to landscape) fed by a Windows desktop app (Tauri). The board is read
at a glance from arm's length, so it is calm, dark and quiet. Big type, few colours, no clutter.
Every page is a **full-screen view**. Home is the idle screen.

Two targets share one language:
- **Board (LVGL, `firmware/main`)**: the tokens in the front matter. They are runtime slots in
  `g_theme` (`ui.h`), read through the `COL_*` macros in `ui_internal.h`, with defaults in `ui.c`.
  The app can replace them with a theme palette (`theme` message, `desktop/crates/core/src/theme.rs`).
- **Desktop app (CSS, `desktop/src/styles.css`, `settings.css`)**: CSS variables per app theme, see
  ## Colors. Board palettes for app themes are derived from these variables in `theme.rs`.

## Colors

One accent (`primary`) marks the current item and the single primary action. Status colours
(`success`, `warning`, `error`) appear only for state. `data.*` colours are for charts and gauges in
this fixed order. Everything else is neutral greys on `background`.

Code names (board): `background`=COL_BG, `rail`=COL_RAIL, `surface`=COL_CARD (COL_CARD_LO is an
alias), `surface-raised`=COL_CARD_HI, `border`=COL_BORDER, `pressed`=COL_PRESSED, `text`=COL_TEXT,
`text-muted`=COL_MUTED, `text-dim`=COL_DIM, `primary`=COL_ACCENT, `primary-pressed`=COL_ACCENT_PRESSED,
`on-primary`=COL_ON_ACCENT. Not themed (compile-time constants): `success`=COL_GOOD, `warning`=COL_WARN,
`error`=COL_BAD, `data.series-1..4`=COL_ACCENT2/COL_CYAN/COL_DATA3/COL_DATA4, `media-track`=COL_TRACK,
`brand.*`=COL_BRAND_*, `github.level-*`=COL_GH_L*, `fixed.white/black`=COL_WHITE/COL_BLACK.
Status colours are the same on board and app (`--good`/`--warn`/`--bad`).

Level colours: values >= 80 % `error`, >= 50 % `warning`, else `success` (`ui_level_color`).
Wi-Fi RSSI >= -62 dBm `success`, >= -74 `warning`, else `error`.
GitHub chart: level 0 = `surface-raised`; levels 1-4 green (`github.*`) or, in accent mode,
`primary` mixed over `surface-raised` at 30/55/80/100 %.

### Desktop app (CSS)

Shared variables (`:root` in `styles.css`): `--fg` = text, `--muted`, `--dim` (lighter than
`--muted` in the app, the reverse of the board), `--good`, `--bad`, `--warn`, each with a `-bg` tint
at 12 % alpha, `--accent-bg` = accent at 12 %, `--on-accent` (white; graphite: `--panel`, the same
luminance rule as `theme.rs`), `--radius` 10px.

```yaml
app-shared:
  fg: "#e3e9f5"
  muted: "#8a97b0"
  dim: "#b4bfd4"
  good: "#3dd68c"
  bad: "#ff6b6b"
  warn: "#f3c14b"
```

App themes (`[data-theme]`). The board gets bg=bg, rail=panel2, card=panel, card_hi=panel3,
border=line, pressed=line2, text=#e3e9f5, muted=#8a97b0, accent=accent; dim, on_accent and
accent_pressed are computed in `theme.rs`.

```yaml
themes:
  blue:     { accent: "#3b8eea", accent2: "#5cc6f0", bg: "#0a101c", bg2: "#0d1524", panel: "#121b2e", panel2: "#0f1727", panel3: "#16223a", line: "#22304d", line2: "#2f4370" }
  purple:   { accent: "#9a5cf5", accent2: "#d08cff", bg: "#120b20", bg2: "#170f29", panel: "#1b1230", panel2: "#160f27", panel3: "#251a40", line: "#3a2a58", line2: "#533c80" }
  graphite: { accent: "#aab4c3", accent2: "#d7dde6", bg: "#111214", bg2: "#16171a", panel: "#1c1e22", panel2: "#17191c", panel3: "#24272c", line: "#2c2f35", line2: "#42464f" }
  midnight: { accent: "#7b8cff", accent2: "#a5b4ff", bg: "#05060a", bg2: "#080a10", panel: "#0d0f18", panel2: "#0a0c13", panel3: "#141829", line: "#1d2030", line2: "#2c3150" }
  teal:     { accent: "#1fb5a8", accent2: "#5fe0d0", bg: "#081517", bg2: "#0b1a1c", panel: "#0f2224", panel2: "#0c1c1e", panel3: "#143032", line: "#1d3a3c", line2: "#2a5457" }
  rose:     { accent: "#e0569b", accent2: "#ff9cc8", bg: "#16080f", bg2: "#1b0b13", panel: "#241019", panel2: "#1d0d15", panel3: "#301624", line: "#4a2236", line2: "#66304b" }
```

Other palettes, each defined once in code (games: `ui_games.c name_color`, weather: `weather_data.c`):

```yaml
games-initials: ["#2f5d8a", "#2e7d6b", "#6b4e9b", "#9a4a5a", "#8a6a2f", "#4a6b2f", "#34707f", "#7a4a2e"]
tile-presets: ["#b4413c", "#3a4256", "#2f6f5e", "#2b2f3a", "#8a6d1f", "#5b3fa0"]   # api.ts TILE_COLORS; --tile-default = #2b2f3a
feed-default: "#7c9cff"                                                      # api.ts FEED_COLOR
weather-icons:                      # weather_data.c, by weather code
  clear-day: "#fbbf5a"
  clear-night: "#c08cff"            # also thunderstorm
  partly-day: "#fcd38d"
  partly-night: "#a9b4ff"
  cloud-fog: "#aeb7e0"
  snow: "#e6f0ff"
  rain: "#67e8f9"
fixed:
  white: "#ffffff"                  # COL_WHITE / --white: games initials text, app slider thumb, --on-accent
  black: "#000000"                  # COL_BLACK / --black: screensaver, playtime badge (70 %), app board preview
data-fallbacks:                     # hub_dispatch.c defaults when a message has no colour
  tile: "#2a2f3a"
  ams-slot: "#444444"
  ams-dark-threshold: "#202020"     # ui_printer.c: darker swatches get a border
```

`find_hardcoded.py` also reports `0xffffff` used as a 24-bit mask in C (`ui.c`, `ui_github.c`,
`hub_dispatch.c`): not colours.

## Typography

Board: **Inter** (no kerning; Montserrat fallback), Medium (500) up to 24 px, SemiBold (600) from
28 px. Fonts: `g_font_14/16/20/24` (Medium), `g_font_28/32/48` (SemiBold), `g_font_clock` 128,
`ui_header_font` 40, `ui_timer_font` 72, `ui_date_font(px)` fitted at runtime (min 12).
Use: caption 14 (`ui_caption`, `text-muted`, letter-space 2), body 16, buttons/values 20,
titles/empty states 24, page titles 32, big numbers 48.
Icons: Material Icons at 20 (dock status), 24 (side-rail status, small buttons), 32 (rail tabs,
buttons up to 56), 48 (media controls, tiles), 80 (weather, empty states).

App: `--body` "IBM Plex Sans" (400, 600), `--mono` "IBM Plex Mono" (500), bundled with @fontsource
(imports at the top of `main.ts`), system-ui / ui-monospace as fallback. Body 15px/1.4.
Settings: section header .72rem uppercase, letter-spacing .07em, accent colour; rows .82rem
(compact .78rem); sub text .72rem; nav .8rem; buttons .84rem (.74rem in rows); codes 600 mono.

## Layout

Board screen 800x480 landscape. Navigation is a **bottom dock** by default (68 px high, page area
800x412, content 760x372); a side rail (88 px, page 712x480, content 672x440) is the alternative.
Page padding 20. `UI_COMPACT` when content height < 400. Card gaps 12 (8 compact); Home grid
left column 384, header 44, event pitch 60. Rail tab 60 x <=52 with gap 2; dock tab <=72 x 52.

App: settings window with a 186 px nav column; main padding 12px 14px 18px; card margin 12; row
padding 9px 12px (compact 6px 12px), gap 16. Home grid editor 5x3, 44 px rows, gap 4. No
breakpoints; the board preview keeps aspect-ratio 800/480.

## Elevation & Depth

Flat. No shadows or gradients on the board: depth comes from `surface` on `background`, a 1 px
`border`, and `surface-raised` for pressed/selected. App windows: panel at 85 % opacity
(`--ov-alpha`), 1 px accent border at 40 %.

## Shapes

Board radii: cards and tiles 16, rail tab 14, art/frames/badges/rows 12, keys 10, pill buttons 28
(alert 23), chart bars 6, event bars 2, circles for round buttons, bars and dots.
App radii: `--radius` 10 (cards), controls 8, windows 12, segmented 7, grid cells 6, pills 999.
Borders 1 px; 2 px only for a checked tile, an alert panel and the rail badge ring.

## Motion & Micro-interaction

Board: page switches are instant (no transitions). Press feedback: round buttons scale to 92 %,
shortcut tiles to 94 %, pressed colours swap at once. Long text marquees: 2.5 s rest, 30 px/s,
ease-in-out ping-pong. Bars animate with the LVGL default. Nothing moves on its own except
data updates; content never jumps position when an item ends (fade in place).

App: controls transition .15s (border, background, colour); buttons press down 1 px; toggles .15s.

## Voice & Content Strategy

Short, plain English labels (German only for website UI strings). Status lines say what is
happening in a few words ("Asking GitHub", "Connected"). Empty states say what to do next.
Panels render with zeros instead of empty placeholders. No exclamation marks, no jargon on the board.

## Components

Board helpers in `ui.c`: `ui_card` (surface, 1 px border, radius 16, pad 16, pressed
surface-raised), `ui_caption`, `ui_round_button` (circle; pressed primary-pressed or pressed;
icon on-primary on accent, else text; sizes 32-48 small, 68-72 secondary, 88-92 primary),
`ui_bar` (8 px, track surface-raised), rail/dock tabs (selected = surface-raised pill, icon text;
idle icon text-muted), badge (12 px circle with a 2 px rail ring). Pill buttons 180x56: primary
= primary / on-primary, secondary = surface-raised. Shortcut tile: card with a user tint
(pressed mix 90 % toward card; checked darkened fill + lightened 2 px border; disabled 40 %).

App components (`styles.css`, `settings.css`): card `.swc` (1 px line, radius 10, header tinted
accent 10 %, uppercase accent label), row `.swr`, button (panel, 1 px line, radius 8; `.on` =
accent text, accent border 60 %, accent-bg), input/select (panel2, focus accent border + 3 px ring),
toggle `.tgl` 36x20 (on = accent track, white knob), nav `.swnav`, segmented `.seg` (selected =
filled accent), status dot 8 px.

## Do's and Don'ts

- Do keep every page a full-screen view; don't merge pages into combined dashboards (media and
  Plex stay separate pages).
- Do treat Home as the idle screen; don't auto-switch tabs unless the user turns on an option.
- Do keep Home widgets at a fixed size and position; when an event ends, replace the content in
  place with a fade. Don't let widgets jump or resize.
- Do use the bottom dock as the default navigation in new layouts, screenshots and videos.
- Do use one accent per screen for the current item / primary action; use status and data colours
  only for state and charts.
- Do take colours from the `COL_*` macros / CSS variables; don't add hex literals in UI code
  (use the design-use skill and its checker).
- Don't use `text-dim` for body text (3.6:1 on surface): icons and placeholders only.
- Classic text (#f3f4f6) differs from the app themes' text (#e3e9f5) on purpose: classic is the
  board's own built-in look; app themes send the app's text colour.
- Known issues: none open. Fixed 2026-11-27: colour literals moved to named constants, alert
  button label uses COL_ON_ACCENT, `--text`/`--on-accent` defined, board/app status colours
  unified, classic accent_pressed matches `theme.rs`, unused `--display` removed.
