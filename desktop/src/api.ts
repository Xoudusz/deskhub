// Typed wrappers for the frontend <-> backend contract in desktop/README.md.
// Outside Tauri (npm run dev in a browser) an in-memory mock backend answers instead.
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type Page = "home" | "stats" | "ai" | "calendar" | "weather" | "media" | "plex" | "printer" | "shortcuts" | "timer" | "games" | "network" | "github";
export type Widget = "clock" | "claude" | "events" | "weather" | "media" | "stats" | "printer" | "game" | "net" | "github";

/** Claude Code wiring: status line and hooks point at deskhub-cli; cli = the helper exists. */
export type ClaudeState = { statusline: boolean; hooks: boolean; cli: boolean };

export type Settings = {
  board: { host: string; token: string };
  display: {
    brightness: number;
    night_brightness: number;
    clock_seconds: boolean;
    night: string;
    night_style: "off" | "clock" | "dim";
    pc_sleep: "off" | "clock" | "none";
  };
  pages: { enabled: string[]; nav: "left" | "right" | "bottom" };
  home: { header: boolean; grid: GridCell[]; presets: HomePreset[] };
  location: Location;
  appearance: { theme: "purple" | "blue" | "graphite" | "midnight" | "teal" | "rose"; board_theme: "same" | "classic" | "purple" | "blue" | "graphite" | "midnight" | "teal" | "rose" };
  ai: { agent: "auto" | "claude" | "codex"; live_limits: boolean; alerts: boolean; idle_alert_seconds: number };
  tiles: Tile[];
  games: { steam: boolean; presets: boolean; presets_disabled: string[]; custom: CustomGame[] };
  media_server: MediaServer;
  imported: boolean;
};

/** "Sign in with Plex" state; the account token itself stays in the credential store. */
export type MediaServer = { signed_in: boolean; server_id: string; server_name: string; client_id: string };
export type PlexServer = { id: string; name: string; owned: boolean };
export type PlexOutcome = { state: "done" | "error" | "expired"; message: string };

/** The board's city, coordinates (decimal strings), POSIX time zone and units. */
export type Location = { enabled: boolean; city: string; lat: string; lon: string; tz: string; fahrenheit: boolean; h24: boolean };

export type Status = {
  state: "searching" | "connected" | "refused" | "stopped";
  host: string;
  detail: string;
  firmware: string | null;
  since: number | null;
  /** Board build ("s3", "p4rev1", "p4rev3") and where it keeps Wi-Fi/token ("nvs", "seed", "none"). */
  variant?: string | null;
  creds?: string | null;
  /** What the board uses now (hello_ok). */
  location?: Location | null;
};

/** Firmware card: newest GitHub release and whether this board can and should take it. */
export type FirmwareCheck = { latest: string | null; update_available: boolean; blocked: string | null; error: string | null };
/** A board found on USB (its token is saved by the backend, never sent here). */
export type UsbBoard = { port: string; version: string; variant: string; ssid: string; ip: string; saved: number; token_saved: boolean };
export type UsbNetwork = { ssid: string; rssi: number; secure: boolean };
export type UsbJoined = { ip: string; host: string };

export type SettingsPatch = { [K in keyof Settings]?: Partial<Settings[K]> };

export const ALL_PAGES: Page[] = ["home", "stats", "ai", "calendar", "weather", "media", "plex", "printer", "shortcuts", "timer", "games", "network", "github"];
export const PAGE_LABELS: Record<Page, string> = {
  home: "Home", stats: "PC stats", ai: "AI agent usage", calendar: "Calendar", weather: "Weather", media: "Now playing", plex: "Plex",
  printer: "3D printer", shortcuts: "Shortcuts", timer: "Timer", games: "Games", network: "Internet", github: "GitHub",
};

export const WIDGET_LABELS: Record<Widget, string> = {
  clock: "Clock", claude: "Claude usage", events: "Events", weather: "Weather", media: "Now playing", stats: "PC stats",
  printer: "3D printer", game: "Game", net: "Internet", github: "GitHub",
};
export const WIDGETS = Object.keys(WIDGET_LABELS) as Widget[];

/** Allowed block sizes per widget (min/max width and height), from companion/deskhub/pages.py HOME_SIZES. */
export const HOME_SIZES: Record<Widget, { w: [number, number]; h: [number, number] }> = {
  clock: { w: [2, 5], h: [1, 2] },
  claude: { w: [1, 5], h: [1, 1] },
  events: { w: [2, 5], h: [1, 3] },
  weather: { w: [1, 5], h: [1, 1] },
  media: { w: [2, 5], h: [1, 1] },
  stats: { w: [1, 5], h: [1, 1] },
  printer: { w: [2, 5], h: [1, 1] },
  game: { w: [2, 5], h: [1, 1] },
  net: { w: [1, 5], h: [1, 1] },
  github: { w: [2, 5], h: [1, 2] },
};

/** A shortcut tile. state: null = guess, "" = never lit, or a Windows state. */
export type Tile = { label: string; icon: string; color: string; action: TileAction; target: string; state: string | null };
export type TileAction = "keys" | "run" | "open" | "media" | "volume" | "lock" | "mic";
/** A non-Steam game the companion watches. */
export type CustomGame = { name: string; process: string[]; match: string; title: string; image: string; launch: string };

export const MAX_TILES = 16;
/** The example tiles (settings.rs default_tiles). */
export const EXAMPLE_TILES: Tile[] = [
  { label: "Mute mic", icon: "mic_off", color: "#b4413c", action: "mic", target: "", state: null },
  { label: "Mute sound", icon: "volume_off", color: "#3a4256", action: "volume", target: "mute", state: null },
  { label: "Vol -", icon: "volume_down", color: "#3a4256", action: "volume", target: "down", state: null },
  { label: "Vol +", icon: "volume_up", color: "#3a4256", action: "volume", target: "up", state: null },
  { label: "Screenshot", icon: "screenshot_monitor", color: "#2f6f5e", action: "keys", target: "win+shift+s", state: null },
  { label: "Terminal", icon: "terminal", color: "#2b2f3a", action: "run", target: "wt.exe", state: null },
  { label: "Downloads", icon: "folder", color: "#8a6d1f", action: "open", target: "~/Downloads", state: null },
  { label: "Lock PC", icon: "lock", color: "#5b3fa0", action: "lock", target: "", state: null },
];
/** Icons the board has for tiles (config.example.toml). */
export const TILE_ICONS = (
  "home insights memory speed music_note event calendar_today wb_sunny cloud grid_view apps wifi computer " +
  "desktop_windows play_arrow pause skip_next skip_previous volume_up volume_down volume_off mic mic_off lock " +
  "photo_camera screenshot_monitor terminal code public folder keyboard power_settings_new schedule timer mail chat " +
  "sports_esports rocket_launch bolt star favorite settings refresh dark_mode headphones videocam movie " +
  "developer_board lan shuffle repeat"
).split(" ");
export const TILE_ACTIONS: [TileAction, string, string][] = [
  ["keys", "Press keys", "e.g. ctrl+shift+m, win+shift+s, f13"],
  ["run", "Run a program", "e.g. notepad.exe or code C:\\Code\\project"],
  ["open", "Open file, folder or URL", "e.g. ~/Downloads or https://..."],
  ["media", "Media control", "toggle, next or prev"],
  ["volume", "Volume", "up, down or mute"],
  ["lock", "Lock the PC", ""],
  ["mic", "Mute microphone", ""],
];
/** Non-Steam games recognised without setup (companion/deskhub/game_presets.py). */
export const GAME_PRESETS = [
  "League of Legends", "Teamfight Tactics", "VALORANT", "Fortnite", "Rocket League", "Minecraft",
  "Minecraft Bedrock", "Roblox", "Overwatch 2", "World of Warcraft", "Diablo IV", "Hearthstone", "StarCraft II",
  "Call of Duty", "Genshin Impact", "Honkai: Star Rail", "Zenless Zone Zero", "Grand Theft Auto V", "Apex Legends",
  "EA SPORTS FC", "The Sims 4", "Halo Infinite", "Escape from Tarkov", "osu!",
];

/** One Home block, in grid cells. */
export type GridCell = { widget: string; x: number; y: number; w: number; h: number };
/** A named Home layout. */
export type HomePreset = { name: string; header: boolean; grid: GridCell[] };

/** Built-in layouts. "Default" matches the Rust default (pages::default_grid). */
export const BUILTIN_PRESETS: HomePreset[] = [
  {
    name: "Default", header: true, grid: [
      { widget: "claude", x: 0, y: 0, w: 3, h: 1 },
      { widget: "events", x: 0, y: 1, w: 3, h: 2 },
      { widget: "weather", x: 3, y: 0, w: 2, h: 1 },
      { widget: "media", x: 3, y: 1, w: 2, h: 1 },
      { widget: "stats", x: 3, y: 2, w: 2, h: 1 },
    ],
  },
  {
    name: "Focus", header: false, grid: [
      { widget: "clock", x: 0, y: 0, w: 3, h: 2 },
      { widget: "claude", x: 0, y: 2, w: 3, h: 1 },
      { widget: "weather", x: 3, y: 0, w: 2, h: 1 },
      { widget: "media", x: 3, y: 1, w: 2, h: 1 },
      { widget: "stats", x: 3, y: 2, w: 2, h: 1 },
    ],
  },
  {
    name: "Dashboard", header: true, grid: [
      { widget: "claude", x: 0, y: 0, w: 5, h: 1 },
      { widget: "stats", x: 0, y: 1, w: 2, h: 1 },
      { widget: "media", x: 2, y: 1, w: 3, h: 1 },
      { widget: "weather", x: 0, y: 2, w: 2, h: 1 },
      { widget: "events", x: 2, y: 2, w: 3, h: 1 },
    ],
  },
];

export const GRID_COLS = 5;
export const GRID_ROWS = 3;

export const TAURI = (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !== undefined;

const clone = <T>(v: T): T => structuredClone(v);

// ---- mock backend (browser dev only) ----
const mock: Settings = {
  board: { host: "deskhub.local", token: "" },
  display: { brightness: 100, night_brightness: 0, night: "", night_style: "off", pc_sleep: "off", clock_seconds: false },
  pages: { enabled: ["home", "stats", "ai", "weather", "media", "shortcuts", "timer", "network"], nav: "left" },
  home: {
    header: true,
    grid: [
      { widget: "clock", x: 0, y: 0, w: 2, h: 1 },
      { widget: "claude", x: 2, y: 0, w: 2, h: 1 },
      { widget: "weather", x: 0, y: 1, w: 2, h: 1 },
      { widget: "media", x: 2, y: 1, w: 3, h: 1 },
    ],
    presets: [],
  },
  appearance: { theme: "midnight", board_theme: "same" },
  ai: { agent: "auto", live_limits: false, alerts: true, idle_alert_seconds: 30 },
  tiles: [
    { label: "Mute mic", icon: "mic_off", color: "#b4413c", action: "mic", target: "", state: null },
    { label: "Vol +", icon: "volume_up", color: "#3a4256", action: "volume", target: "up", state: null },
    { label: "Screenshot", icon: "screenshot_monitor", color: "#2f6f5e", action: "keys", target: "win+shift+s", state: null },
    { label: "Lock PC", icon: "lock", color: "#5b3fa0", action: "lock", target: "", state: null },
  ],
  games: { steam: true, presets: true, presets_disabled: [], custom: [] },
  location: { enabled: false, city: "New York", lat: "40.7128", lon: "-74.0060", tz: "EST5EDT,M3.2.0,M11.1.0", fahrenheit: true, h24: false },
  media_server: { signed_in: false, server_id: "", server_name: "", client_id: "deskhub-mock" },
  imported: false,
};
let mockStatus: Status = { state: "searching", host: "deskhub.local", detail: "Looking for deskhub.local", firmware: null, since: null };
let mockAutostart = true;
const subs = {
  status: new Set<(s: Status) => void>(),
  settings: new Set<(s: Settings) => void>(),
  sim: new Set<(f: SimFrame) => void>(),
  plex: new Set<(o: PlexOutcome) => void>(),
};

/** One frame for the simulator, as the board would get it: JSON, or a binary frame in base64. */
export type SimFrame = { json: unknown } | { bin: string };

/** The home message the app sends the board (crates/core/src/pages.rs home_message, grid part). */
function mockHomeFrame(): SimFrame {
  return { json: { type: "home", header: mock.home.header, grid: mock.home.grid.map(c => ({ widget: c.widget, x: c.x, y: c.y, w: c.w, h: c.h })) } };
}

/** Browser mock: a plausible board state for the simulator. */
function mockSimFrames(): SimFrame[] {
  return [
    { json: { type: "layout", pages: ["home", "stats", "ai", "weather", "media", "plex", "shortcuts", "timer"], ordered: true, nav: "bottom", seconds: mock.display.clock_seconds } },
    mockHomeFrame(),
    { json: { type: "stats", cpu: 14, cpu_temp: 52, gpu: 31, gpu_temp: 48, ram_used: 11.2, ram_total: 32, vram_used: 3.1, vram_total: 16, net_down: 120000, net_up: 9000, disk: 61, cpu_name: "Ryzen 7 7800X3D", gpu_name: "Radeon RX 7900 XT" } },
    { json: { type: "media", active: true, playing: true, title: "Midnight City", artist: "M83", album: "Hurry Up, We're Dreaming", source: "Spotify", pos: 95, dur: 243, can_seek: true, art_id: 0 } },
    { json: { type: "media_server", configured: true, name: "Home", kind: "plex", online: true, version: "Plex Media Server 1.41.2", library: "312 movies, 45 shows, 2,104 episodes", streams: 2, transcoding: 0, active: true, playing: true, title: "Pilot", artist: "Show - S1 E2", album: "Living Room TV - alex (+1 more)", source: "Home", pos: 61, dur: 1800, can_seek: false, controls: false, art_id: 0 } },
    { json: { type: "ai", provider: "claude", limits: { five_hour: { pct: 42, reset: Math.floor(Date.now() / 1000) + 5400 }, updated: Math.floor(Date.now() / 1000) }, block: { active: true, tokens: 182000, messages: 23 }, today: { tokens: 640000, output: 52000, messages: 88 }, days: [12, 30, 22, 41, 18, 9, 33], last_active: Math.floor(Date.now() / 1000) } },
  ];
}

function mockEmitSettings() {
  subs.settings.forEach(cb => cb(clone(mock)));
}

function mockMerge(p: SettingsPatch): Settings {
  for (const k of Object.keys(p) as (keyof Settings)[]) {
    const v = p[k];
    if (v && typeof v === "object" && !Array.isArray(v)) Object.assign(mock[k] as object, v);
    else (mock as unknown as Record<string, unknown>)[k] = v;
  }
  mockEmitSettings();
  if (p.home) subs.sim.forEach(cb => cb(mockHomeFrame()));
  return clone(mock);
}

setTimeout(() => {
  if (TAURI) return;
  mockStatus = {
    state: "connected", host: mock.board.host, detail: "Connected", firmware: "0.1.0 (mock)",
    since: Math.floor(Date.now() / 1000), variant: "p4rev1", creds: "nvs",
    location: { enabled: false, city: "Vienna", lat: "48.2082", lon: "16.3738", tz: "CET-1CEST,M3.5.0,M10.5.0/3", fahrenheit: false, h24: true },
  };
  subs.status.forEach(cb => cb(mockStatus));
}, 1500);

// ---- API ----
export const api = {
  getSettings: (): Promise<Settings> => (TAURI ? invoke<Settings>("get_settings") : Promise.resolve(clone(mock))),
  setSettings: (patch: SettingsPatch): Promise<Settings> => (TAURI ? invoke<Settings>("set_settings", { patch }) : Promise.resolve(mockMerge(patch))),
  getStatus: (): Promise<Status> => (TAURI ? invoke<Status>("get_status") : Promise.resolve({ ...mockStatus })),
  /** PNG as a data URL (800x480); the browser mock returns a static picture. */
  /** The simulator's starting state: every frame the board would get on connect. */
  simState: (): Promise<SimFrame[]> => (TAURI ? invoke<SimFrame[]>("sim_state") : Promise.resolve(mockSimFrames())),
  /** Simulator frames as they are published (and layout/home when settings change). */
  onSimFrame: async (cb: (f: SimFrame) => void): Promise<void> => {
    if (TAURI) await listen<SimFrame>("sim-frame", e => cb(e.payload));
    else subs.sim.add(cb);
  },
  /** Frames were missed: reload the whole state with simState. */
  onSimResync: async (cb: () => void): Promise<void> => {
    if (TAURI) await listen("sim-resync", () => cb());
  },
  boardScreenshot: (): Promise<string> => (TAURI ? invoke<string>("board_screenshot") : Promise.resolve("/mock-board.png")),
  boardTap: async (x: number, y: number): Promise<void> => {
    if (TAURI) await invoke("board_tap", { x, y });
    else console.info("[mock] board_tap", x, y);
  },
  /** File picker for config.toml; null when cancelled or in browser mock mode. */
  /** Send a firmware image to the board (picker when no path); resolves with the board's answer. */
  boardFirmware: async (path?: string): Promise<string> => {
    if (TAURI) return invoke<string>("board_firmware", { path: path ?? null });
    await new Promise(r => setTimeout(r, 1500));
    return "installed; restarting";
  },
  /** Find the board on USB; saves its token and host. */
  usbFind: async (): Promise<UsbBoard> => {
    if (TAURI) return invoke<UsbBoard>("usb_find");
    await new Promise(r => setTimeout(r, 800));
    mockMerge({ board: { host: "deskhub.local", token: "mock-token-from-usb" } });
    return { port: "COM3", version: "firmware-v0.9.0", variant: "p4rev1", ssid: "", ip: "", saved: 0, token_saved: true };
  },
  /** Networks the board on `port` sees. */
  usbScan: async (port: string): Promise<UsbNetwork[]> => {
    if (TAURI) return invoke<UsbNetwork[]>("usb_scan", { port });
    await new Promise(r => setTimeout(r, 1200));
    return [
      { ssid: "Home Wi-Fi", rssi: -42, secure: true },
      { ssid: "Office", rssi: -61, secure: true },
      { ssid: "Cafe Guest", rssi: -79, secure: false },
    ];
  },
  /** Join and save a network on the board; resolves with its address. */
  usbWifi: async (port: string, ssid: string, pass: string): Promise<UsbJoined> => {
    if (TAURI) return invoke<UsbJoined>("usb_wifi", { port, ssid, pass });
    await new Promise(r => setTimeout(r, 1500));
    if (pass.startsWith("wrong")) throw new Error("couldn't join: check the password and that the network is in range");
    return { ip: "192.168.1.50", host: "deskhub" };
  },
  /** A fresh 4-digit code for the board's pairing prompt. */
  boardPairCode: (): Promise<string> =>
    TAURI ? invoke<string>("board_pair_code") : Promise.resolve(String(Math.floor(Math.random() * 10000)).padStart(4, "0")),
  /** Ask the board for its token (it shows `code`, the user taps Allow); saves it. */
  boardPair: async (code: string): Promise<void> => {
    if (TAURI) return invoke<void>("board_pair", { code });
    await new Promise(r => setTimeout(r, 2500));
    mockMerge({ board: { token: "mock-token-from-pair" } });
  },
  /** Start with Windows; the setter returns the state Windows reports afterwards. */
  autostartGet: (): Promise<boolean> => (TAURI ? invoke<boolean>("autostart_get") : Promise.resolve(mockAutostart)),
  autostartSet: (on: boolean): Promise<boolean> => (TAURI ? invoke<boolean>("autostart_set", { on }) : Promise.resolve((mockAutostart = on))),
  /** Newest firmware release for this board (GitHub). */
  firmwareCheck: (): Promise<FirmwareCheck> => (TAURI ? invoke<FirmwareCheck>("firmware_check")
    : Promise.resolve({ latest: "firmware-v0.7.0", update_available: true, blocked: null, error: null })),
  /** Download the newest release image for this board and send it; resolves with the board's answer. */
  boardFirmwareGithub: async (): Promise<string> => {
    if (TAURI) return invoke<string>("board_firmware_github");
    await new Promise(r => setTimeout(r, 1500));
    return "installed; restarting";
  },
  pickImage: async (): Promise<string | null> => {
    if (!TAURI) return null;
    const picked = await open({
      title: "Pick a picture",
      multiple: false,
      directory: false,
      filters: [{ name: "Pictures", extensions: ["png", "jpg", "jpeg", "bmp", "webp", "gif"] }],
    });
    return typeof picked === "string" ? picked : null;
  },
  pickConfig: async (): Promise<string | null> => {
    if (!TAURI) return null;
    const picked = await open({
      title: "Pick the Python companion's config.toml",
      multiple: false,
      directory: false,
      filters: [{ name: "config.toml", extensions: ["toml"] }],
    });
    return typeof picked === "string" ? picked : null;
  },
  /** Claude Code status line + hooks state; the mock says "not connected". */
  claudeStatus: (): Promise<ClaudeState> =>
    TAURI ? invoke<ClaudeState>("claude_status") : Promise.resolve({ statusline: false, hooks: false, cli: true }),
  claudeConnect: (on: boolean): Promise<ClaudeState> =>
    TAURI ? invoke<ClaudeState>("claude_connect", { on }) : Promise.resolve({ statusline: on, hooks: on, cli: true }),
  /** Opens the Plex sign-in page; resolves with its address. The result comes via onPlex. */
  plexSignIn: async (): Promise<string> => {
    if (TAURI) return invoke<string>("plex_sign_in");
    setTimeout(() => {
      mockMerge({ media_server: { signed_in: true, server_id: "", server_name: "" } });
      subs.plex.forEach(cb => cb({ state: "done", message: "" }));
    }, 2000);
    return "https://app.plex.tv/auth#?clientID=deskhub-mock&code=mock";
  },
  plexSignInCancel: (): Promise<void> => (TAURI ? invoke<void>("plex_sign_in_cancel") : Promise.resolve()),
  plexServers: (): Promise<PlexServer[]> =>
    TAURI ? invoke<PlexServer[]>("plex_servers")
      : Promise.resolve([{ id: "srv1", name: "Home", owned: true }, { id: "srv2", name: "Friend's server", owned: false }]),
  plexSelectServer: async (id: string, name: string): Promise<void> => {
    if (TAURI) return invoke<void>("plex_select_server", { id, name });
    mockMerge({ media_server: { server_id: id, server_name: name } });
  },
  plexSignOut: async (): Promise<void> => {
    if (TAURI) return invoke<void>("plex_sign_out");
    mockMerge({ media_server: { signed_in: false, server_id: "", server_name: "" } });
  },
  importConfig: (path?: string): Promise<Settings> => {
    if (TAURI) return invoke<Settings>("import_config", { path });
    mock.imported = true;
    mockEmitSettings();
    return Promise.resolve(clone(mock));
  },
};

export async function onStatus(cb: (s: Status) => void): Promise<void> {
  if (TAURI) await listen<Status>("status", e => cb(e.payload));
  else subs.status.add(cb);
}

export async function onPlex(cb: (o: PlexOutcome) => void): Promise<void> {
  if (TAURI) await listen<PlexOutcome>("plex-sign-in", e => cb(e.payload));
  else subs.plex.add(cb);
}

export async function onSettings(cb: (s: Settings) => void): Promise<void> {
  if (TAURI) await listen<Settings>("settings", e => cb(e.payload));
  else subs.settings.add(cb);
}

export async function appVersion(): Promise<string> {
  if (!TAURI) return "dev";
  try {
    return await getVersion();
  } catch {
    return "dev";
  }
}

/** Window controls. Close hides the window: the app keeps running in the tray. Mock mode: no-ops. */
export const win = {
  minimize: async (): Promise<void> => {
    if (TAURI) await getCurrentWindow().minimize();
  },
  hide: async (): Promise<void> => {
    if (TAURI) await getCurrentWindow().hide();
  },
  startResize: async (): Promise<void> => {
    if (TAURI) await getCurrentWindow().startResizeDragging("SouthEast");
  },
};
