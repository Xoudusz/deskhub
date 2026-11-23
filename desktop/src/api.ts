// Typed wrappers for the frontend <-> backend contract in desktop/README.md.
// Outside Tauri (npm run dev in a browser) an in-memory mock backend answers instead.
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type Page = "home" | "stats" | "ai" | "calendar" | "weather" | "media" | "printer" | "shortcuts" | "timer" | "games" | "network" | "github";
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
  appearance: { theme: "purple" | "blue" | "graphite" | "midnight" | "teal" | "rose"; board: boolean };
  ai: { agent: "auto" | "claude" | "codex"; live_limits: boolean; alerts: boolean; idle_alert_seconds: number };
  tiles: Tile[];
  games: { steam: boolean; presets: boolean; presets_disabled: string[]; custom: CustomGame[] };
  imported: boolean;
};

export type Status = {
  state: "searching" | "connected" | "refused" | "stopped";
  host: string;
  detail: string;
  firmware: string | null;
  since: number | null;
};

export type SettingsPatch = { [K in keyof Settings]?: Partial<Settings[K]> };

export const ALL_PAGES: Page[] = ["home", "stats", "ai", "calendar", "weather", "media", "printer", "shortcuts", "timer", "games", "network", "github"];
export const PAGE_LABELS: Record<Page, string> = {
  home: "Home", stats: "PC stats", ai: "AI agent usage", calendar: "Calendar", weather: "Weather", media: "Now playing",
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
  appearance: { theme: "midnight", board: true },
  ai: { agent: "auto", live_limits: false, alerts: true, idle_alert_seconds: 30 },
  tiles: [
    { label: "Mute mic", icon: "mic_off", color: "#b4413c", action: "mic", target: "", state: null },
    { label: "Vol +", icon: "volume_up", color: "#3a4256", action: "volume", target: "up", state: null },
    { label: "Screenshot", icon: "screenshot_monitor", color: "#2f6f5e", action: "keys", target: "win+shift+s", state: null },
    { label: "Lock PC", icon: "lock", color: "#5b3fa0", action: "lock", target: "", state: null },
  ],
  games: { steam: true, presets: true, presets_disabled: [], custom: [] },
  imported: false,
};
let mockStatus: Status = { state: "searching", host: "deskhub.local", detail: "Looking for deskhub.local", firmware: null, since: null };
const subs = { status: new Set<(s: Status) => void>(), settings: new Set<(s: Settings) => void>() };

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
  return clone(mock);
}

setTimeout(() => {
  if (TAURI) return;
  mockStatus = {
    state: "connected", host: mock.board.host, detail: "Connected", firmware: "0.1.0 (mock)",
    since: Math.floor(Date.now() / 1000),
  };
  subs.status.forEach(cb => cb(mockStatus));
}, 1500);

// ---- API ----
export const api = {
  getSettings: (): Promise<Settings> => (TAURI ? invoke<Settings>("get_settings") : Promise.resolve(clone(mock))),
  setSettings: (patch: SettingsPatch): Promise<Settings> => (TAURI ? invoke<Settings>("set_settings", { patch }) : Promise.resolve(mockMerge(patch))),
  getStatus: (): Promise<Status> => (TAURI ? invoke<Status>("get_status") : Promise.resolve({ ...mockStatus })),
  /** PNG as a data URL (800x480); the browser mock returns a static picture. */
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
