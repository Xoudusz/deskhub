# Desk Hub desktop app

The Windows app that replaces the Python companion: a Rust background service that talks to the board, plus a settings window in the Atreia (aion2-craft) look. It sits in the tray and starts with Windows.

Layout:

- `crates/core/`: the service. It has no Tauri or GUI dependency, so it builds and tests on Linux. It holds the board protocol, the WebSocket session, the settings with config.toml import, and the HTTP calls (screenshot, tap).
- `src-tauri/`: the Tauri shell. It provides the window, tray, autostart and single-instance, and wraps `core` in commands and events.
- `src/`, `index.html`: the frontend (vanilla TypeScript + Vite). Run `npm run dev` to open it in a browser with a mock backend.

The Python companion (`../companion`) still does calendars, printers, internet stats and GitHub. Don't run it at the same time as the app: the board accepts one companion only.

## Frontend ↔ backend contract

Commands (`invoke`):

| Command | Args | Returns |
|---|---|---|
| `get_settings` | – | `Settings` |
| `set_settings` | `{ patch: Partial<Settings> }` (deep merge, one level per section) | `Settings` (after save; changes reach the board right away) |
| `get_status` | – | `Status` |
| `board_screenshot` | – | `string`: PNG as a `data:image/png;base64,...` URL (800x480) |
| `board_tap` | `{ x: number, y: number }` (board pixels, 0-799 / 0-479) | `null` |
| `import_config` | `{ path?: string }` (default: `./config.toml`, then `%USERPROFILE%\<"", src, source\repos, Documents, Documents\GitHub, GitHub, code, dev, projects, repos>\deskhub\companion\config.toml`; the error "No config.toml found" makes the UI open a file picker) | `Settings` |

Events (`listen`):

- `status`: `Status`, sent on every change.
- `settings`: `Settings`, sent after any change.

```ts
type Settings = {
  board: { host: string; token: string };                // host default "deskhub.local"
  display: {
    brightness: number;          // 1-100, default 100
    night_brightness: number;    // 0-100, default 0 (0 = night style as before)
    night: string;               // "" (off) or "HH:MM-HH:MM"
    night_style: "off" | "clock";
    pc_sleep: "off" | "clock" | "none";
  };
  pages: { enabled: string[]; nav: "left" | "right" | "bottom" };   // order = navigation order
  home: { header: boolean; grid: { widget: string; x: number; y: number; w: number; h: number }[] };
  appearance: { theme: "purple" | "blue" | "graphite" | "midnight" | "teal" | "rose" };
  imported: boolean;             // config.toml already imported once
};

type Status = {
  state: "searching" | "connected" | "refused" | "stopped";
  host: string;
  detail: string;                // human text, e.g. "Looking for deskhub.local", "Wrong token"
  firmware: string | null;       // version from hello_ok
  since: number | null;          // unix seconds the link came up
};
```

Pages known to the board: home, stats, ai, calendar, weather, media, printer, shortcuts, timer, games, network, github. Home grid: 5 columns x 3 rows. Widgets and sizes follow `companion/deskhub/pages.py`.
