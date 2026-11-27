# Desk Hub desktop app

The Windows app that replaces the Python companion: a Rust background service that talks to the board, plus a settings window in the Atreia (aion2-craft) look. It sits in the tray and starts with Windows.

Layout:

- `crates/core/`: the service. It has no Tauri or GUI dependency, so it builds and tests on Linux. It holds the board protocol, the WebSocket session, the settings with config.toml import, and the HTTP calls (screenshot, tap).
- `src-tauri/`: the Tauri shell. It provides the window, tray, autostart and single-instance, and wraps `core` in commands and events.
- `src/`, `index.html`: the frontend (vanilla TypeScript + Vite). Run `npm run dev` to open it in a browser with a mock backend.
- `src/sim.ts`: the simulated board (Status preview and Home builder): the firmware UI compiled to WASM by
  `bash ../firmware/sim/build.sh` into `public/sim/` (not committed; CI builds it). Without it the views say
  "Simulator not built".

The Python companion (`../companion`) is only needed for 3D printers and iCloud CalDAV calendars. Don't run it at the same time as the app: the board accepts one companion only.

## Frontend ↔ backend contract

Commands (`invoke`):

| Command | Args | Returns |
|---|---|---|
| `get_settings` | – | `Settings` |
| `set_settings` | `{ patch: Partial<Settings> }` (deep merge, one level per section) | `Settings` (after save; changes reach the board right away) |
| `get_status` | – | `Status` |
| `board_screenshot` | – | `string`: PNG as a `data:image/png;base64,...` URL (800x480) |
| `board_tap` | `{ x: number, y: number }` (board pixels, 0-799 / 0-479) | `null` |
| `sound_test` | `{ sound: string }` | `null`: sends `sound_test` to the board (unknown names play nothing); errors when the board service isn't running |
| `sim_state` | – | `SimFrame[]`: every frame the board would get on connect (layout, home, then the published state, binary first; the board's own location while the app sets none). `SimFrame = { json: object } \| { bin: string /* base64 ART1/STM1 */ }` |
| `autostart_get` / `autostart_set` | – / `{ on: boolean }` | `boolean`: Start with Windows (enabled on first run) |
| `firmware_check` | – | `{ latest: string \| null, update_available: boolean, blocked: string \| null, error: string \| null }`: newest `firmware-latest/firmware.json` release for the connected board |
| `board_firmware_github` | – | `string`: downloads the release image for the board's variant, checks sha256 and size, sends it (`POST /firmware`, the board asks first). Refused unless the board reports `creds: "nvs"` and a known variant |
| `usb_find` | – | `{ port, version, variant, ssid, ip, saved, token_saved }`: the first USB serial port (VID 303A / 1A86 / 10C4) whose board answers `info`; saves the board's token and `<host>.local`. Never returns the token. Windows only. The port is opened with DTR/RTS off (serialport's default DCB): a port opened with them on hung the P4 board until replugged |
| `usb_scan` | `{ port: string }` | `{ ssid, rssi, secure }[]`: networks the board sees |
| `usb_wifi` | `{ port: string, ssid: string, pass: string }` | `{ ip, host }`: the board joined and saved the network (saves `<host>.local`); errors name the cause, never the password |
| `board_pair_code` | – | `string`: a random 4-digit code |
| `board_pair` | `{ code: string }` | `null`: `POST /pair` to the board with the computer name and code; saves the token on Allow, explains Deny / no answer / cooldown / old firmware |
| `plex_sign_in` | – | `string`: the plex.tv sign-in page (opened in the browser; shown in case none opened). Starts a PIN (`POST plex.tv/api/v2/pins?strong=true`) and polls it every 2 s for up to 5 min; the result arrives as `plex-sign-in`. On approval the account token goes to Credential Manager ("Desk Hub" / "plex-token", read back to confirm) and a single server on the account is picked |
| `plex_sign_in_cancel` | – | `null`: stops waiting |
| `plex_servers` | – | `{ id, name, owned }[]`: Plex Media Servers on the signed-in account (plex.tv `/resources`) |
| `plex_select_server` | `{ id: string, name: string }` | `null`: saves `media_server.server_id` / `server_name` |
| `plex_sign_out` | – | `null`: deletes the token and forgets the server |
| `github_token_set` | `{ token: string }` | `null`: saves a GitHub token (`github_pat_...` or `ghp_...`) in Credential Manager ("Desk Hub" / "github-token", read back to confirm) and asks GitHub now; `""` deletes it |
| `github_status` | – | `{ token_saved, signed_in, pending, ok, error, login, total }`: the contribution collector's last answer (`pending` until it has asked); `token_saved` is the token fallback, `signed_in` the GitHub sign-in |
| `gh_sign_in` | – | `{ user_code, uri }`: starts the device flow of the Desk Hub GitHub App (`ghauth::CLIENT_ID`), opens `uri`; polls in the background and emits `gh-sign-in` `{ state: "done" \| "error" \| "expired", message }`. Tokens go to Credential Manager ("Desk Hub" / "github-oauth", JSON) |
| `gh_sign_in_cancel` | – | `null`: stops the waiting sign-in |
| `gh_sign_out` | – | `null`: deletes the sign-in (a saved token is used again) |
| `calendar_status` | – | `{ name, ok, error, events }[]`: the last read of each calendar feed and of the Outlook sign-in (error names the problem, never the link; "Sign in again" when Microsoft no longer accepts it) |
| `ms_sign_in` | `{ clientId: string, tenant: string }` | the Microsoft sign-in address (also opened in the browser): saves `calendar.ms_client_id` / `ms_tenant`, listens once on `127.0.0.1` for the `http://localhost:<port>` redirect (auth code + PKCE, 5 min), keeps the refresh token in Credential Manager ("Desk Hub" / "ms-refresh-token") and sets `ms_signed_in`. Result as `ms-sign-in` |
| `ms_sign_in_cancel` | – | `null`: stops waiting |
| `ms_sign_out` | – | `null`: deletes the refresh token, clears `ms_signed_in` (client id and tenant stay) |
| `import_config` | `{ path?: string }` (default: `./config.toml`, then `%USERPROFILE%\<"", src, source\repos, Documents, Documents\GitHub, GitHub, code, dev, projects, repos>\deskhub\companion\config.toml`; the error "No config.toml found" makes the UI open a file picker) | `Settings` |

Events (`listen`):

- `status`: `Status`, sent on every change.
- `settings`: `Settings`, sent after any change.
- `ms-sign-in`: `{ state: "done" | "error" | "expired", message }`, the end of `ms_sign_in`.
- `sim-frame`: `SimFrame`, every frame published for the board (and layout/home when settings change them).
- `sim-resync`: frames were missed; call `sim_state` again.
- `plex-sign-in`: `{ state: "done" | "error" | "expired", message: string }`, the end of a `plex_sign_in`.

```ts
type Settings = {
  board: { host: string; token: string };                // host default "deskhub.local"
  display: {
    brightness: number;          // 1-100, default 100
    night_brightness: number;    // 0-100, default 0 (0 = night style as before)
    night: string;               // "" (off) or "HH:MM-HH:MM"
    night_style: "off" | "clock" | "dim";
    pc_sleep: "off" | "clock" | "none";
    clock_seconds: boolean;      // Home clock shows seconds (layout key "seconds")
  };
  pages: { enabled: string[]; nav: "left" | "right" | "bottom" };   // order = navigation order
  home: {
    presets: { name: string; header: boolean; grid: GridCell[] }[];  // user-saved layouts header: boolean; grid: { widget: string; x: number; y: number; w: number; h: number }[] };
  appearance: { theme: "purple" | "blue" | "graphite" | "midnight" | "teal" | "rose"; board_theme: "same" | "classic" | <theme> };   // default midnight / "same"; board_theme picks the palette sent to the board ("theme" message)
  ai: {
    agent: "auto" | "claude" | "codex";   // auto = most recently used
    live_limits: boolean;        // opt-in: 5-hour/weekly limits from Anthropic's usage endpoint with Claude Code's sign-in (~/.claude/.credentials.json; token never logged or sent to the board), default false
    alerts: boolean;             // "Claude is waiting" alerts, default true
    idle_alert_seconds: number;  // default 30
    remote_url: string;          // deskhub-relay of a server ("" = off); its token: remote_token_set (credential store "remote-claude-token")
  };
  tiles: {                       // Shortcuts page, max 16; default = config.example.toml tiles
    label: string; icon: string; color: string;   // icon: a board icon name, color "#rrggbb"
    action: "keys" | "run" | "open" | "media" | "volume" | "lock" | "mic";
    target: string;
    state: "mic_mute" | "speaker_mute" | "" | null;  // null = guess, "" = never lit
  }[];
  games: {
    steam: boolean;              // default true
    presets: boolean;            // popular non-Steam games, default true
    presets_disabled: string[];
    custom: { name: string; process: string[]; match: string; title: string; image: string; launch: string }[];
  };
  location: { enabled: boolean; city: string; lat: string; lon: string; tz: string; fahrenheit: boolean; h24: boolean };
                                 // sent to the board ("location") only once enabled; until then the UI shows the board's values
  media_server: {                // Plex "now playing" (collectors/plex.rs); kept across import_config
    signed_in: boolean;          // the plex.tv token is in Credential Manager ("Desk Hub" / "plex-token")
    server_id: string;           // plex.tv clientIdentifier of the chosen server
    server_name: string;
    client_id: string;           // X-Plex-Client-Identifier, made once per install
  };
  calendar: {                    // .ics feeds for the Calendar page (collectors/calendar.rs)
    feeds: { name: string; url: string; color: string }[];  // url: https:// or webcal://; color "#RRGGBB" ("" = blue)
    days: number;                // 1-60, default 7
    ms_client_id: string;        // "Sign in with Microsoft": Entra app registration (client) ID
    ms_tenant: string;           // work email, domain or tenant ID (single-tenant app)
    ms_signed_in: boolean;       // refresh token saved
    ms_name: string;             // calendar name on the board ("" = "Outlook")
    ms_color: string;            // "#RRGGBB" ("" = blue)
  };
  internet: {                    // the Network page (collectors/net.rs); Settings > Connection
    show_ip: boolean;            // public IP from speed.cloudflare.com/cdn-cgi/trace every 30 min
    speedtest_hours: number;     // automatic speed test every N hours (0 = only from the board)
  };
  sound: {                       // P4 board speaker (layout "sound"); Settings > Sound
    mute: boolean;
    volume: number;              // 0-100, default 60
    alert: string;               // "off" | "chime" | "bell" | "beep" | "soft"; Claude Code alerts, default "chime"
    timer: string;               // timer done, default "bell"
    reminder: string;            // calendar reminder, default "chime"
    remind_minutes: number;      // 0-120, default 5 (0 = at the start)
  };
  imported: boolean;             // config.toml already imported once
};

type Status = {
  state: "searching" | "connected" | "refused" | "stopped";
  host: string;
  detail: string;                // human text, e.g. "Looking for deskhub.local", "Wrong token"
  firmware: string | null;       // version from hello_ok
  since: number | null;          // unix seconds the link came up
  variant: string | null;        // "s3" | "p4rev1" | "p4rev3" (hello_ok; older firmware: null)
  creds: string | null;          // "nvs" | "seed" | "none": release images need "nvs"
  location: Location | null;     // what the board uses now (enabled: false)
};

The board token lives in Windows Credential Manager ("Desk Hub" / "board-token", `crates/core/src/secrets.rs`);
`settings.json` keeps it blank once the store holds it. `get_settings` still returns it.
```

The Plex collector asks plex.tv for the server's addresses and tries its HTTPS `*.plex.direct`
addresses (local first, then remote; never relays), each pinned to its IP (no DNS lookup, so
DNS-rebind protection doesn't matter), with the server's own token only. It keeps the first
whose `/identity` is the chosen server: a server on Docker's host network also lists its bridge
addresses (172.x) as local. When none answers, `media_server` carries `error` (every address
tried and why), which the Media section shows; the board ignores it. Signing in adds the
board's `plex` page to `pages.enabled`. No plain-HTTP fallback (Linux dev builds, which lack TLS, use HTTP). It polls `/status/sessions` every 3 s while
anything plays (15 s idle, 60 s after a failure) and sends `media_server` JSON plus an `ARS1`
poster (128x192, letterboxed). Everyone on the server counts; one session shows, playing first
and sticky, with "(+N more)". Read-only: no seek or play/pause.

The calendar collector (`collectors/calendar.rs`, parser `ics.rs`) reads each `calendar.feeds`
link every 10 minutes and whenever the calendar settings change (`webcal://` becomes `https://`,
4 MB cap). Recurring events are expanded with the `rrule` crate (EXDATE, RECURRENCE-ID overrides,
UNTIL applied by hand); TZIDs are IANA names or Outlook's Windows names (table in `ics.rs`),
anything else is the PC's zone. It sends `events` (up to 16, the next `days`, window from a day
back) only when at least one feed is set, so the Python companion's calendar isn't overwritten.
With `ms_signed_in`, it also reads Microsoft Graph `/me/calendarView` (`msgraph.rs`: one-hour access
token kept in memory, refreshed from the rotating refresh token, one retry after a 401, next pages on
the same host only, all-day events rounded to their date) and merges those events in.
The GitHub collector (`collectors/github.rs`, port of `companion/deskhub/github.py`) asks
api.github.com every 15 minutes (10 after a failure, at once after `github_token_set` or a sign-in) and sends
`github` when it changes. It uses the GitHub sign-in first (`ghauth.rs`: device flow, no secret;
the 8 h access token is renewed 5 minutes before it runs out or once after a 401, the rotated pair is saved
before use unless the user signed out or in meanwhile; a refused refresh token deletes the sign-in and
shows "Sign in again"), else the token. Saving a token or signing in adds `github` to `pages.enabled`.

The internet collector (`collectors/net.rs`, port of `companion/deskhub/netstats.py`) connects to
1.1.1.1:443 (then 8.8.8.8:443) every 2 s and sends `net`: latency (median of 5), jitter, loss and
history over 60 probes, uptime and the last drop, data used today and this month (all
interfaces, kept in `~/.deskhub/net_usage.json`, the companion's file), the last speed test
(speed.cloudflare.com: 25 MB down, 10 MB up; on the board's `speedtest`, at most one per 2 min,
or every `speedtest_hours`) and, when `show_ip`, the public IP.

Text for the board goes through `text::board_text` (every collector's `ascii_fold` calls it):
the board's Inter fonts have ASCII, Latin-1 (ä ö ü ß é …) and some punctuation (€ “ ” „ – —
…), so those stay; other Latin letters fold to their base (ł → l), combining marks join their
letter when Latin-1 has the result, and anything else (emoji, CJK) is dropped. Limits are bytes,
cut on a character boundary, because the board's buffers are byte arrays. The firmware's
`copy_str` also trims a cut-off character. The Python companion still sends ASCII only.

Pages known to the board: home, stats, ai, calendar, weather, media, plex (desktop app only), printer, shortcuts, timer, games, network, github. Home grid: 5 columns x 3 rows. Widgets and sizes follow `companion/deskhub/pages.py`.

## Claude Code on a server

`crates/core/src/relay.rs` (binary `deskhub-relay`, runs on the server) serves
`GET /v1/sessions` and `GET /v1/usage?rev=N` (hourly buckets per model) with a Bearer
token. `collectors/remote.rs` polls it every 2 s (usage every 10 s) and shares the result
through `Ctx.remote`: `alerts` merges the sessions as `r-<id>` with `focus: false` (the
board hides Show), `ai` adds the hours to the PC's usage. Commands: `remote_token_set(token)`,
`remote_status()`.

Allow / Deny: a server's permission request carries `request` (an id per request) and
`detail` ("Bash: npm test"); its alert has `decide: true`. The board's `alert_action`
`allow`/`deny` goes to `remote::decide` -> relay `POST /v1/decide?id=&request=&behavior=`
(409 unless that request is pending) -> `~/.deskhub/claude_decisions/<id>.json` ->
deskhub-cli's waiting PermissionRequest hook prints the decision (only with
`~/.deskhub/board_decisions`, made by `deskhub-relay install-hooks`).

## deskhub-cli sidecar

The installer ships `deskhub-cli.exe` next to the app (`bundle.externalBin`). Before
`npx tauri dev` or a local `npx tauri build`, build it and copy it with the target triple:

```powershell
cargo build --release -p deskhub-cli
New-Item -ItemType Directory -Force src-tauri/binaries | Out-Null
Copy-Item target/release/deskhub-cli.exe src-tauri/binaries/deskhub-cli-x86_64-pc-windows-msvc.exe
```

## Releases and app updates

Installed apps update themselves from GitHub Releases (Tauri updater, signed). To publish:

1. Bump `version` in `src-tauri/tauri.conf.json` (and `src-tauri/Cargo.toml`).
2. Commit, then tag and push: `git tag desktop-vX.Y.Z && git push origin desktop-vX.Y.Z`.

`.github/workflows/desktop-release.yml` builds the signed installer into release `desktop-vX.Y.Z` and copies
`latest.json` to the fixed release `desktop-latest`, which the app checks every 10 minutes. The app downloads the
new version in the background and shows "Update to X.Y.Z" in the title bar.

Signing key: `~/.tauri/deskhub.key` on vibecode (public key in `tauri.conf.json`), stored as repo secrets
`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (empty). Losing the key means installed apps
can't verify new versions: keep a backup.
