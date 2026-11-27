// Settings window: boot, sidebar sections, rendering, live preview, events.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles.css";
import "./settings.css";
import { fillIcons, icon } from "./icons";
import {
  api, win, TAURI, onStatus, onSettings, onPlex, onMs, onGh, appVersion, ALL_PAGES, PAGE_LABELS, WIDGETS, WIDGET_LABELS, HOME_SIZES, GRID_COLS, GRID_ROWS,
  type Page, type Settings, type SettingsPatch, type Status, type Widget, type ClaudeState, type RemoteStatus, type AlertLogEntry, type HomePreset, BUILTIN_PRESETS,
  type Tile, type TileAction, type SoundName, type CalendarFeed, type FeedStatus, type GithubStatus, type CustomGame, type FirmwareCheck, type PlexServer, type SimFrame, type UsbBoard, type UsbNetwork, type Location, MAX_TILES, TILE_COLORS, FEED_COLOR, TILE_ICONS, TILE_ACTIONS, GAME_PRESETS, EXAMPLE_TILES,
} from "./api";
import { card, row, tgl, text, slider, select, esc } from "./ui";
import { mountSim, onSimState, simState } from "./sim";

type SectionId = "status" | "display" | "pages" | "shortcuts" | "games" | "media" | "ai" | "calendar" | "sound" | "appearance" | "connection" | "about";
type SliderKey = "brightness" | "nightBrightness" | "volume";
type Cell = Settings["home"]["grid"][number];

const SECTIONS: [SectionId, string, string][] = [
  ["status", "gauge", "Status"],
  ["display", "sun", "Display"],
  ["pages", "layout-grid", "Pages"],
  ["shortcuts", "zap", "Shortcuts"],
  ["games", "gamepad", "Games"],
  ["media", "film", "Media"],
  ["ai", "sparkles", "AI agents"],
  ["calendar", "calendar", "Calendar & GitHub"],
  ["sound", "volume-2", "Sound"],
  ["appearance", "palette", "Appearance"],
  ["connection", "wifi", "Connection"],
  ["about", "info", "About"],
];
const THEMES: [string, string][] = [
  ["midnight", "Midnight (default)"], ["purple", "Purple"], ["blue", "Blue"], ["graphite", "Graphite"], ["teal", "Teal"], ["rose", "Rose"], ["slate", "Slate (calm)"], ["sage", "Sage (calm)"],
];
const BOARD_THEMES: [string, string][] = [
  ["same", "Same as app"], ["classic", "Classic amber"], ...THEMES.map(([k, v]): [string, string] => [k, v.replace(" (default)", "")]),
];
const STATE_TEXT: Record<Status["state"], string> = { connected: "Connected", searching: "Searching", refused: "Refused", stopped: "Stopped" };
const DOT: Record<Status["state"], string> = { connected: "good", searching: "warn", refused: "bad", stopped: "" };
const SLIDER_SPEC: Record<SliderKey, { label: string; min: number; max: number }> = {
  brightness: { label: "Brightness", min: 1, max: 100 },
  nightBrightness: { label: "Night brightness", min: 0, max: 100 },
  volume: { label: "Volume", min: 0, max: 100 },
};

const main = document.getElementById("main") as HTMLElement;
const nav = document.getElementById("nav") as HTMLElement;
const dot = document.getElementById("dot") as HTMLElement;

let S: Settings;
let st: Status = { state: "searching", host: "", detail: "", firmware: null, since: null };
let section: SectionId = "status";
let version = "dev";
let showToken = false;
let homeNote = "";
let shotBusy = false;
const timers = new Map<string, number>();

/** Don't redraw under the user's fingers: a field being typed into, a time picked, a slider dragged, an open select. */
const busy = () => !!document.activeElement?.matches("input:not([type=checkbox]), select");

const sliderValue = (k: SliderKey) =>
  k === "brightness" ? S.display.brightness : k === "volume" ? S.sound.volume : S.display.night_brightness;
const setSliderLocal = (k: SliderKey, v: number) => {
  if (k === "brightness") S.display.brightness = v;
  else if (k === "volume") S.sound.volume = v;
  else S.display.night_brightness = v;
};
const sliderPatch = (k: SliderKey, v: number): SettingsPatch =>
  k === "brightness" ? { display: { brightness: v } }
    : k === "volume" ? { sound: { volume: v } } : { display: { night_brightness: v } };

const fillRange = (el: HTMLInputElement) =>
  el.style.setProperty("--p", `${((+el.value - +el.min) / (+el.max - +el.min || 1)) * 100}%`);

function syncSlider(k: SliderKey) {
  const v = sliderValue(k);
  main.querySelectorAll<HTMLInputElement>(`input[data-slider="${k}"]`).forEach(el => {
    el.value = String(v);
    fillRange(el);
  });
  main.querySelectorAll<HTMLElement>(`b[data-val="${k}"]`).forEach(el => {
    el.textContent = `${v}%`;
  });
}

function applyTheme() {
  document.documentElement.dataset.theme = S.appearance.theme;
}

function setDot() {
  dot.className = `dot ${DOT[st.state]}`;
  dot.title = `${STATE_TEXT[st.state]}: ${st.detail}`;
}

function render(force = false) {
  if (!force && busy()) return;
  const top = main.scrollTop;
  main.innerHTML = sectionHtml();
  main.querySelectorAll<HTMLInputElement>("input[type=range]").forEach(fillRange);
  mountSim(main, section, section === "pages");
  main.scrollTop = top;
  if (section === "status") refreshShot();
}

// ---- simulated board (sim.ts) ----
/** Status preview: the board's screenshot ("live") or the simulator ("sim"). */
let preview: "live" | "sim" = "live";

/** The simulator's placeholder, with a note when it can't run. */
function simHtml(): string {
  const st = simState();
  const note = st === "missing" ? "Simulator not built (firmware/sim/build.sh)"
    : st === "failed" ? "The simulator failed to start"
    : st === "loading" || st === "idle" ? "Starting the simulator" : "";
  return `<div class="prev sim" data-sim>${note ? `<span class="simnote">${esc(note)}</span>` : ""}</div>`;
}

// The note over the canvas follows the simulator's state (in place: no re-render mid-edit).
onSimState(() => {
  const host = main.querySelector<HTMLElement>("[data-sim]");
  if (!host) return;
  const html = simHtml();
  const note = /<span class="simnote">(.*?)<\/span>/.exec(html)?.[1] ?? "";
  host.querySelector(".simnote")?.remove();
  if (note) host.insertAdjacentHTML("afterbegin", `<span class="simnote">${note}</span>`);
});

function renderNav() {
  nav.innerHTML =
    SECTIONS.map(([id, ic, label]) => `<button type="button" data-sec="${id}" class="${id === section ? "on" : ""}">${icon(ic, 15)}${label}</button>`).join("") +
    `<span class="ver">Desk Hub ${esc(version)}</span>`;
}

async function save(patch: SettingsPatch) {
  // Apply locally first (same one-level merge as Rust merge_patch), so a second edit
  // before the reply builds on this one instead of reverting it.
  const local = S as unknown as Record<string, unknown>;
  for (const [k, v] of Object.entries(patch as Record<string, unknown>)) {
    const cur = local[k];
    local[k] = v && typeof v === "object" && !Array.isArray(v) && cur && typeof cur === "object"
      ? { ...(cur as object), ...(v as object) } : v;
  }
  try {
    S = await api.setSettings(patch);
  } catch (err) {
    console.error("set_settings failed", err);
    return;
  }
  applyTheme();
  render();
  window.setTimeout(refreshShot, 300);
}

function saveLater(key: string, patch: SettingsPatch) {
  window.clearTimeout(timers.get(key));
  timers.set(key, window.setTimeout(() => {
    timers.delete(key);
    void save(patch);
  }, 120));
}

// ---- live preview ----
// The last board picture: shown at once when Status opens (and while offline) instead
// of an empty box until the next screenshot arrives.
let lastShot = (() => { try { return localStorage.getItem("lastShot") ?? ""; } catch { return ""; } })();

function refreshShot() {
  const img = document.getElementById("shot") as HTMLImageElement | null;
  if (!img || shotBusy || section !== "status" || st.state !== "connected" || document.visibilityState !== "visible") return;
  shotBusy = true;
  api.boardScreenshot()
    .then(url => {
      img.src = url;
      lastShot = url;
      try { localStorage.setItem("lastShot", url); } catch { /* full: keep it in memory only */ }
    })
    .catch(() => undefined)
    .finally(() => { shotBusy = false; });
}

function tap(e: MouseEvent, img: HTMLImageElement) {
  const r = img.getBoundingClientRect();
  const fx = (e.clientX - r.left) / r.width;
  const fy = (e.clientY - r.top) / r.height;
  const x = Math.min(799, Math.max(0, Math.floor(fx * 800)));
  const y = Math.min(479, Math.max(0, Math.floor(fy * 480)));
  const mark = document.createElement("span");
  mark.className = "pulse";
  mark.style.left = `${fx * 100}%`;
  mark.style.top = `${fy * 100}%`;
  img.parentElement?.appendChild(mark);
  window.setTimeout(() => mark.remove(), 700);
  api.boardTap(x, y).catch(() => undefined);
}

// ---- sections ----
function sectionHtml(): string {
  switch (section) {
    case "status": return statusHtml();
    case "display": return displayHtml();
    case "pages": return pagesHtml();
    case "shortcuts": return shortcutsHtml();
    case "games": return gamesHtml();
    case "media": return mediaHtml();
    case "ai": return aiHtml();
    case "calendar": return calendarHtml();
    case "sound": return soundHtml();
    case "appearance": return appearanceHtml();
    case "connection": return connectionHtml();
    case "about": return aboutHtml();
  }
}

function statusHtml(): string {
  const on = st.state === "connected";
  const board = card("Board", "", [
    row(`${icon(on ? "wifi" : "wifi-off", 14)}${STATE_TEXT[st.state]}`, "", esc(st.detail)),
    row("Host", `<span>${esc(st.host || S.board.host)}</span>`),
    row("Firmware", esc(st.firmware ?? "Unknown")),
    row("Connected since", st.since ? esc(new Date(st.since * 1000).toLocaleString()) : "Not yet"),
  ].join(""));
  const sim = !on || preview === "sim";
  const body = sim
    ? simHtml()
    : `<div class="prev" id="prev"><img id="shot" alt="Board screen" draggable="false"${lastShot ? ` src="${lastShot}"` : ""}></div>`;
  const seg = `<span class="seg">` +
    `<button type="button" data-act="preview" data-v="live" class="${sim ? "" : "on"}"${on ? "" : " disabled"}>Board</button>` +
    `<button type="button" data-act="preview" data-v="sim" class="${sim ? "on" : ""}">Simulated</button></span>`;
  const sub = sim
    ? "The board's own screen, simulated with the app's data. Clicks act as touches; shortcuts don't run"
    : "Tap the picture to press the board";
  return board + card("Preview", sub, row("Show", seg, on ? "" : "Board offline: showing the simulator") + `<div class="swr swsl">${body}</div>`);
}

function displayHtml(): string {
  const d = S.display;
  const nightOn = d.night !== "";
  const [from, to] = nightOn ? d.night.split("-") : ["22:00", "07:00"];
  const brightness = card("Brightness", "", slider("brightness", "Brightness", 1, 100, d.brightness));
  const clock = card("Clock and charts", "", [
    row("Show seconds", tgl(`data-k="clockSeconds"`, d.clock_seconds), "Home clock on the board"),
    row("Chart history", select(`data-k="chartHours"`, [["1", "Last hour"], ["24", "Last 24 hours"], ["168", "Last 7 days"]], String(d.chart_hours)),
      "Stats and Internet charts on the board. Saved on this PC, so they survive restarts"),
  ].join(""));
  const night = card("Night", "Dims the board after dark", [
    row(`${icon("moon", 14)}Night mode`, tgl(`data-k="nightOn"`, nightOn)),
    ...(nightOn
      ? [
          row("From", `<input type="time" data-k="nightFrom" value="${esc(from)}">`),
          row("Until", `<input type="time" data-k="nightTo" value="${esc(to)}">`),
        ]
      : []),
    row("Night style", select(`data-k="nightStyle"`, [["off", "Screen off"], ["clock", "Clock"], ["dim", "Dim only"]], d.night_style),
      d.night_style === "dim" ? "Pages stay as they are, just darker" : ""),
    slider("nightBrightness", "Night brightness", d.night_style === "dim" ? 1 : 0, 100,
      d.night_style === "dim" && d.night_brightness < 1 ? 20 : d.night_brightness,
      d.night_style === "dim" ? "How bright the board stays at night" : "0 = screen off or clock"),
  ].join(""));
  const pc = card("PC and navigation", "", [
    row("When the PC sleeps", select(`data-k="pcSleep"`, [["off", "Screen off"], ["clock", "Clock"], ["none", "Do nothing"]], d.pc_sleep)),
    row("Navigation bar", select(`data-k="nav"`, [["left", "Left"], ["right", "Right"], ["bottom", "Bottom"]], S.pages.nav),
      "The board restarts when this changes"),
  ].join(""));
  return brightness + clock + night + pc + locationHtml();
}

// ---- Calendar & GitHub: .ics feeds and the GitHub token, statuses asked every 3 s while open ----
let feedStatus: FeedStatus[] = [];
let gh: GithubStatus | null = null;
let ghNote = "";
/** "Sign in with GitHub": the code to enter while waiting. */
let ghCode: { user_code: string; uri: string } | null = null;
/** "Sign in with Microsoft": the page address while waiting, and the last message. */
let msUrl = "";
let msNote = "";

function feedStatusText(i: number): string {
  const f = S.calendar.feeds[i];
  if (!f.url.trim()) return "Paste the calendar's .ics link";
  const name = f.name.trim() || `Calendar ${i + 1}`;
  const st = feedStatus.find(s => s.name === name);
  if (!st) return "Reading the calendar";
  return st.ok ? `${icon("check", 12)}${st.events === 1 ? "1 event" : `${st.events} events`} in the next ${S.calendar.days} days`
    : `${icon("x", 12)}${esc(st.error)}`;
}

function calendarHtml(): string {
  const c = S.calendar;
  const feeds = c.feeds.map((f, i) => [
    row(`<input class="txt" data-k="calName" data-i="${i}" value="${esc(f.name)}" placeholder="Calendar ${i + 1}" spellcheck="false">`,
      `<input type="color" data-k="calColor" data-i="${i}" value="${esc(f.color || FEED_COLOR)}" title="Color on the board">` +
        `<button type="button" class="ib" data-act="cal-remove" data-i="${i}" title="Remove">${icon("trash", 13)}</button>`),
    row(`<input class="txt" type="password" data-k="calUrl" data-i="${i}" value="${esc(f.url)}" placeholder="https://outlook.office365.com/owa/calendar/.../calendar.ics" autocomplete="off" spellcheck="false">`,
      "", feedStatusText(i)),
  ].join(""));
  const cal = card("Calendars", "Events for the board's Calendar page and reminders", [
    ...feeds,
    row("", `<button type="button" data-act="cal-add">${icon("plus", 13)}Add calendar</button>`),
    row("Days ahead", `<input class="num" type="number" min="1" max="60" data-k="calDays" value="${c.days}">`),
    text("Outlook: outlook.office.com > Settings > Calendar > Shared calendars > Publish a calendar, then copy the ICS link (no Publish option? Use Sign in with Microsoft below). Google: Calendar settings > Secret address in iCal format."),
    text("The link lets anyone read the calendar: it stays on this PC and is never logged."),
  ].join(""));
  const g = gh;
  const ghState = !g ? row("Status", "", "Checking")
    : !g.token_saved && !g.signed_in ? row(`${icon("x", 14)}Not connected`, "", g.error ? esc(g.error) : "Sign in to show your contributions")
    : g.pending ? row("Status", "", "Asking GitHub")
    : g.ok ? row(`${icon("check", 14)}Connected`, "", esc(`${g.login} - ${g.total} contributions in the last year`))
    : row(`${icon("x", 14)}GitHub didn't answer`, "", esc(g.error));
  const signIn = ghCode
    ? row(`<span style="font:600 22px/1.2 ui-monospace,monospace;letter-spacing:2px">${esc(ghCode.user_code)}</span>`,
        `<button type="button" data-act="gh-copy">${icon("copy", 13)}Copy code</button>` +
          `<button type="button" data-act="gh-cancel">${icon("x", 13)}Cancel</button>`,
        `Enter this code on ${esc(ghCode.uri)} (it opened in your browser), then Authorize`)
    : g?.signed_in
      ? row("Signed in with GitHub", `<button type="button" data-act="gh-sign-out">${icon("x", 13)}Sign out</button>`)
      : row("", `<button type="button" data-act="gh-sign-in">${icon("zap", 13)}Sign in with GitHub</button>`,
          "Read-only. The sign-in stays in Windows Credential Manager");
  const github = card("GitHub", "Your contribution chart on the board's GitHub page", [
    signIn,
    ghState,
    row("Or use a token", `<input class="txt" type="password" data-k="ghToken" value="" placeholder="${g?.token_saved ? "Saved (type to replace, empty to remove)" : "github_pat_..."}" autocomplete="off" spellcheck="false">`,
      "github.com/settings/personal-access-tokens: a fine-grained token, no permissions needed. Used when not signed in"),
    ...(ghNote ? [text(ghNote)] : []),
  ].join(""));
  return cal + outlookHtml() + github;
}

/** Outlook (work) via "Sign in with Microsoft", for calendars that can't be published. */
function outlookHtml(): string {
  const c = S.calendar;
  const name = c.ms_name.trim() || "Outlook";
  const st = feedStatus.find(s => s.name === name);
  const status = !st ? "Reading the calendar"
    : st.ok ? `${icon("check", 12)}${st.events === 1 ? "1 event" : `${st.events} events`} in the next ${c.days} days`
    : `${icon("x", 12)}${esc(st.error)}`;
  const setup = [
    row("Application (client) ID", `<input class="txt" data-k="msClient" value="${esc(c.ms_client_id)}" placeholder="00000000-0000-0000-0000-000000000000" spellcheck="false">`,
      "From your app registration in entra.microsoft.com (see README, Calendars)"),
    row("Work email or domain", `<input class="txt" data-k="msTenant" value="${esc(c.ms_tenant)}" placeholder="you@company.com" spellcheck="false">`),
  ];
  const rows = c.ms_signed_in
    ? [
        row(`<input class="txt" data-k="msName" value="${esc(c.ms_name)}" placeholder="Outlook" spellcheck="false">`,
          `<input type="color" data-k="msColor" value="${esc(c.ms_color || FEED_COLOR)}" title="Color on the board">` +
            `<button type="button" data-act="ms-sign-out">${icon("x", 13)}Sign out</button>`, status),
      ]
    : msUrl
      ? [
          ...setup,
          row("Waiting for Microsoft", `<button type="button" data-act="ms-cancel">${icon("x", 13)}Cancel</button>`,
            "Sign in with your work account in the browser window that opened. No window? Open this address:"),
          `<div class="swr"><input class="txt" readonly value="${esc(msUrl)}" spellcheck="false" style="width:100%"></div>`,
        ]
      : [
          ...setup,
          row("", `<button type="button" data-act="ms-sign-in">${icon("zap", 13)}Sign in with Microsoft</button>`,
            "Reads your calendar only. The sign-in stays in Windows Credential Manager"),
        ];
  return card("Outlook (work)", "For a work calendar that can't be published as a link", [...rows, ...(msNote ? [text(esc(msNote))] : [])].join(""));
}

async function ghSignIn() {
  ghNote = "";
  try {
    ghCode = await api.ghSignIn();
  } catch (err) {
    ghNote = String(err);
  }
  render(true);
}

async function msSignIn() {
  msNote = "";
  const val = (k: string) => (document.querySelector<HTMLInputElement>(`[data-k="${k}"]`)?.value ?? "").trim();
  try {
    msUrl = await api.msSignIn(val("msClient"), val("msTenant"));
  } catch (err) {
    msNote = String(err);
  }
  render(true);
}

function saveFeeds(feeds: CalendarFeed[]) {
  void save({ calendar: { feeds } });
}

function pollCalendar() {
  const key = (x: unknown) => JSON.stringify(x);
  const tick = () => {
    if (section !== "calendar") return;
    Promise.all([api.calendarStatus(), api.githubStatus()]).then(([f, g]) => {
      if (key(f) === key(feedStatus) && key(g) === key(gh)) return;
      feedStatus = f;
      gh = g;
      if (section === "calendar") render();
    }).catch(() => undefined);
  };
  setInterval(tick, 3000);
  return tick;
}
const calendarTick = pollCalendar();

const SOUND_NAMES: [string, string][] = [["off", "Off"], ["chime", "Chime"], ["bell", "Bell"], ["beep", "Beep"], ["soft", "Soft"]];
const SOUND_EVENTS: ["alert" | "timer" | "reminder", string, string][] = [
  ["alert", "Claude Code alerts", "When Claude Code waits for you"],
  ["timer", "Timer done", ""],
  ["reminder", "Calendar reminders", "Needs a calendar (Settings > Calendar & GitHub)"],
];

function soundHtml(): string {
  const so = S.sound;
  const spk = card("Speaker", "ESP32-P4 board only (the S3 board has no speaker)", [
    row(`${icon(so.mute ? "volume-x" : "volume-2", 14)}Mute`, tgl(`data-k="soundMute"`, so.mute), "Test still plays when muted"),
    slider("volume", "Volume", 0, 100, so.volume),
  ].join(""));
  const events = card("Sounds", "Which sound each event plays", [
    ...SOUND_EVENTS.map(([k, label, hint]) =>
      row(esc(label),
        select(`data-k="sound" data-ev="${k}"`, SOUND_NAMES, so[k]) +
          `<button type="button" data-act="sound-test" data-ev="${k}" ${so[k] === "off" ? "disabled" : ""}>${icon("volume-2", 13)}Test</button>`,
        esc(hint))),
    row("Remind before", `<input class="num" type="number" min="0" max="120" data-k="soundRemind" value="${so.remind_minutes}"> min`,
      "0 = when the event starts"),
  ].join(""));
  return spk + events;
}

/** POSIX time zones for the board (Kconfig HUB_TZ format). */
const TIME_ZONES: [string, string][] = [
  ["CET-1CEST,M3.5.0,M10.5.0/3", "Vienna, Berlin, Paris"],
  ["GMT0BST,M3.5.0/1,M10.5.0", "London, Dublin"],
  ["EET-2EEST,M3.5.0/3,M10.5.0/4", "Helsinki, Athens"],
  ["EST5EDT,M3.2.0,M11.1.0", "New York"],
  ["CST6CDT,M3.2.0,M11.1.0", "Chicago"],
  ["MST7MDT,M3.2.0,M11.1.0", "Denver"],
  ["PST8PDT,M3.2.0,M11.1.0", "Los Angeles"],
  ["UTC0", "UTC"],
];

/** What the Location card edits: the app's copy once set, else what the board reports. */
function loc(): Location {
  return S.location.enabled ? S.location : (st.location ?? S.location);
}

function saveLocation(change: Partial<Location>) {
  void save({ location: { ...loc(), ...change, enabled: true } });
}

function locationHtml(): string {
  if (!S.location.enabled && !st.location) {
    return card("Location", "Weather, clock and units on the board",
      text(st.state === "connected" ? "This board's firmware doesn't report its location; update it first." : "Connect to the board to see and change its location."));
  }
  const l = loc();
  const known = TIME_ZONES.some(([v]) => v === l.tz);
  const zones: [string, string][] = known ? TIME_ZONES : [...TIME_ZONES, [l.tz, `Other: ${l.tz}`]];
  return card("Location", "Weather, clock and units on the board", [
    row("City", `<input class="txt" data-k="locCity" value="${esc(l.city)}" spellcheck="false">`, "Shown on the weather page"),
    row("Latitude", `<input class="txt" data-k="locLat" value="${esc(l.lat)}" placeholder="48.2082" spellcheck="false">`),
    row("Longitude", `<input class="txt" data-k="locLon" value="${esc(l.lon)}" placeholder="16.3738" spellcheck="false">`,
      "Decimal degrees, for the weather forecast"),
    row("Time zone", select(`data-k="locTz"`, zones, l.tz)),
    row("24-hour clock", tgl(`data-k="locH24"`, l.h24)),
    row("Fahrenheit and mph", tgl(`data-k="locF"`, l.fahrenheit)),
    text(S.location.enabled ? "The board restarts when these change." : "These are the board's current values; changing one makes the app manage them."),
  ].join(""));
}

/** Enabled pages in navigation order, Home first. */
function onList(): Page[] {
  const rest = S.pages.enabled.filter((p): p is Page => p !== "home" && (ALL_PAGES as string[]).includes(p));
  return ["home", ...new Set(rest)];
}

function movePage(p: Page, d: number) {
  const list = onList();
  const i = list.indexOf(p);
  const j = i + d;
  if (i < 1 || j < 1 || j >= list.length) return;
  [list[i], list[j]] = [list[j], list[i]];
  void save({ pages: { enabled: list } });
}

function pagesHtml(): string {
  const on = onList();
  const off = ALL_PAGES.filter(p => !on.includes(p));
  const rows = [...on, ...off].map(p => {
    const idx = on.indexOf(p);
    const isOn = idx >= 0;
    const move = isOn && p !== "home"
      ? `<span class="updown">` +
        `<button type="button" class="ib" data-act="page-up" data-page="${p}" title="Move up"${idx <= 1 ? " disabled" : ""}>${icon("chevron-up", 12)}</button>` +
        `<button type="button" class="ib" data-act="page-down" data-page="${p}" title="Move down"${idx === on.length - 1 ? " disabled" : ""}>${icon("chevron-down", 12)}</button>` +
        `</span>`
      : "";
    return row(esc(PAGE_LABELS[p]), tgl(`data-k="pageOn" data-page="${p}"`, isOn, p === "home") + move);
  }).join("");
  const free = WIDGETS.filter(w => !S.home.grid.some(c => c.widget === w));
  const addOpts: [string, string][] = [["", "Choose..."], ...free.map(w => [w, WIDGET_LABELS[w]] as [string, string])];
  const presetOpts: [string, string][] = [
    ["", "Choose..."],
    ...BUILTIN_PRESETS.map((p, i) => [`b${i}`, p.name] as [string, string]),
    ...S.home.presets.map((p, i) => [`u${i}`, `${p.name} (yours)`] as [string, string]),
  ];
  const own = presetSel.startsWith("u");
  const home = card("Home screen", "5 columns by 3 rows. Drag a block to move it, its corner to resize it.", [
    row("Layout",
      select(`data-k="homePreset"`, presetOpts, presetSel) +
      `<button type="button" data-act="preset-apply"${presetSel ? "" : " disabled"}>Apply</button>` +
      (own ? `<button type="button" class="ib" data-act="preset-delete" title="Delete this preset">${icon("x", 13)}</button>` : ""),
      "Presets replace the blocks and the header setting"),
    row("Show header", tgl(`data-k="homeHeader"`, S.home.header)),
    `<div class="grid-wrap">${gridHtml()}</div>`,
    sizeRow(),
    row("Preview", "", "The simulated board, updated when you drop or resize a block"),
    `<div class="swr swsl">${simHtml()}</div>`,
    row("Add a block", select(`data-k="homeAdd"`, addOpts, "", free.length === 0), esc(homeNote)),
    row("Save as preset",
      `<input class="txt" data-k="presetName" placeholder="Name" maxlength="24" value="${esc(presetName)}" spellcheck="false">` +
      `<button type="button" data-act="preset-save">${icon("plus", 13)}Save</button>`),
    row("Reset", `<button type="button" data-act="home-reset">${icon("refresh-cw", 13)}Reset to default</button>`,
      "Default blocks with the header on"),
  ].join(""));
  return card("Pages", "Enabled pages keep this order in the navigation bar", rows) + home;
}

function gridHtml(): string {
  const occupied = new Set<string>();
  const blocks = S.home.grid.map((c, i) => {
    for (let y = c.y; y < c.y + c.h; y++) for (let x = c.x; x < c.x + c.w; x++) occupied.add(`${x},${y}`);
    const label = WIDGET_LABELS[c.widget as Widget] ?? c.widget;
    const sz = HOME_SIZES[c.widget as Widget];
    const resizable = sz && (sz.w[0] < sz.w[1] || sz.h[0] < sz.h[1]);
    return `<div class="hblock${i === selBlock ? " sel" : ""}" data-act="grid-select" data-idx="${i}" style="grid-column:${c.x + 1} / span ${c.w};grid-row:${c.y + 1} / span ${c.h}">` +
      `<span>${esc(label)}<small>${c.w} x ${c.h}</small></span>` +
      `<button type="button" class="ib" data-act="grid-remove" data-idx="${i}" title="Remove">${icon("x", 11)}</button>` +
      (resizable ? `<div class="hres" title="Drag to resize"></div>` : "") + `</div>`;
  }).join("");
  const cells: string[] = [];
  for (let y = 0; y < GRID_ROWS; y++) {
    for (let x = 0; x < GRID_COLS; x++) {
      if (!occupied.has(`${x},${y}`)) cells.push(`<div class="hcell" style="grid-column:${x + 1};grid-row:${y + 1}"></div>`);
    }
  }
  return `<div class="hgrid">${cells.join("")}${blocks}</div>`;
}

/** First spot (top-left first) where the widget's default size (smallest w and h) fits. */
function findSpot(w: Widget): Cell | null {
  const { w: [cw], h: [ch] } = HOME_SIZES[w];
  for (let y = 0; y <= GRID_ROWS - ch; y++) {
    for (let x = 0; x <= GRID_COLS - cw; x++) {
      const clash = S.home.grid.some(c => c.x < x + cw && x < c.x + c.w && c.y < y + ch && y < c.y + c.h);
      if (!clash) return { widget: w, x, y, w: cw, h: ch };
    }
  }
  return null;
}

function removeCell(i: number) {
  homeNote = "";
  selBlock = -1;
  void save({ home: { grid: S.home.grid.filter((_, j) => j !== i) } });
}

// ---- Home builder: selection, resize, presets ----
let selBlock = -1;
let presetSel = "";
let presetName = "";

/** Whether block `i` can sit at `c`: inside the grid, no overlap with the other blocks. */
function fits(i: number, c: Cell): boolean {
  if (c.x < 0 || c.y < 0 || c.x + c.w > GRID_COLS || c.y + c.h > GRID_ROWS) return false;
  return S.home.grid.every((o, j) => j === i || !(o.x < c.x + c.w && c.x < o.x + o.w && o.y < c.y + c.h && c.y < o.y + o.h));
}

/** Whether block `i` can take size w x h at its spot. */
function sizeFits(i: number, w: number, h: number): boolean {
  return fits(i, { ...S.home.grid[i], w, h });
}

// ---- Home builder: drag a block to move it, its corner handle to resize it ----
type Drag = { i: number; el: HTMLElement; grid: HTMLElement; resize: boolean; col: number; row: number; orig: Cell; next: Cell };
let drag: Drag | null = null;
/** A drag ends with a click on the block; that click must not toggle the selection. */
let dragEndedAt = 0;

/** The grid cell under the pointer, clamped to the grid. */
function cellAt(grid: HTMLElement, e: PointerEvent): [number, number] {
  const r = grid.getBoundingClientRect();
  const col = Math.floor(((e.clientX - r.left) / r.width) * GRID_COLS);
  const row = Math.floor(((e.clientY - r.top) / r.height) * GRID_ROWS);
  return [Math.max(0, Math.min(GRID_COLS - 1, col)), Math.max(0, Math.min(GRID_ROWS - 1, row))];
}

const clampN = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

main.addEventListener("pointerdown", e => {
  const t = e.target as HTMLElement;
  if (drag || e.button !== 0 || t.closest(".ib")) return;
  const el = t.closest<HTMLElement>(".hblock");
  const grid = el?.closest<HTMLElement>(".hgrid");
  if (!el || !grid) return;
  const i = Number(el.dataset.idx);
  const c = S.home.grid[i];
  if (!c) return;
  const [col, row] = cellAt(grid, e);
  drag = { i, el, grid, resize: !!t.closest(".hres"), col, row, orig: { ...c }, next: { ...c } };
  el.setPointerCapture(e.pointerId);
  el.classList.add("dragging");
  e.preventDefault();
});

main.addEventListener("pointermove", e => {
  if (!drag) return;
  // A re-render (settings changed elsewhere) replaced the grid: give up this drag.
  if (!drag.el.isConnected) {
    drag = null;
    return;
  }
  const [col, row] = cellAt(drag.grid, e);
  const dc = col - drag.col, dr = row - drag.row;
  const o = drag.orig;
  let n: Cell;
  if (drag.resize) {
    const sz = HOME_SIZES[o.widget as Widget];
    n = {
      ...o,
      w: clampN(o.w + dc, sz.w[0], Math.min(sz.w[1], GRID_COLS - o.x)),
      h: clampN(o.h + dr, sz.h[0], Math.min(sz.h[1], GRID_ROWS - o.y)),
    };
  } else {
    n = { ...o, x: clampN(o.x + dc, 0, GRID_COLS - o.w), y: clampN(o.y + dr, 0, GRID_ROWS - o.h) };
  }
  if (n.x === drag.next.x && n.y === drag.next.y && n.w === drag.next.w && n.h === drag.next.h) return;
  drag.next = n;
  drag.el.style.gridColumn = `${n.x + 1} / span ${n.w}`;
  drag.el.style.gridRow = `${n.y + 1} / span ${n.h}`;
  drag.el.classList.toggle("bad", !fits(drag.i, n));
  const small = drag.el.querySelector("small");
  if (small) small.textContent = `${n.w} x ${n.h}`;
});

function endDrag(cancel: boolean) {
  const d = drag;
  if (!d) return;
  drag = null;
  d.el.classList.remove("dragging");
  const { orig: o, next: n } = d;
  const moved = n.x !== o.x || n.y !== o.y || n.w !== o.w || n.h !== o.h;
  if (!moved) return;   // a plain click: selects the block
  dragEndedAt = Date.now();
  // The grid changed under the drag (another window, a preset): don't save a stale block.
  const cur = S.home.grid[d.i];
  const same = d.el.isConnected && cur && cur.widget === o.widget && cur.x === o.x && cur.y === o.y && cur.w === o.w && cur.h === o.h;
  if (!cancel && same && fits(d.i, n)) {
    selBlock = d.i;
    homeNote = "";
    void save({ home: { grid: S.home.grid.map((c, j) => (j === d.i ? n : c)) } });
  } else {
    render(true);       // back to where it was
  }
}

main.addEventListener("pointerup", () => endDrag(false));
main.addEventListener("pointercancel", () => endDrag(true));

function sizeRow(): string {
  const c = S.home.grid[selBlock];
  if (!c) return row("Size", `<span class="muted">Click a block</span>`);
  const sz = HOME_SIZES[c.widget as Widget];
  if (!sz) return "";
  const opts = (lo: number, hi: number, fit: (v: number) => boolean) => {
    const o: [string, string][] = [];
    for (let v = lo; v <= hi; v++) if (fit(v)) o.push([String(v), String(v)]);
    return o;
  };
  const ws = opts(sz.w[0], sz.w[1], v => sizeFits(selBlock, v, c.h));
  const hs = opts(sz.h[0], sz.h[1], v => sizeFits(selBlock, c.w, v));
  const label = WIDGET_LABELS[c.widget as Widget] ?? c.widget;
  return row(`Size of ${esc(label)}`,
    `<span class="muted">Width</span>` + select(`data-k="blockW"`, ws, String(c.w), ws.length < 2) +
    `<span class="muted">Height</span>` + select(`data-k="blockH"`, hs, String(c.h), hs.length < 2),
    "Only sizes that fit next to the other blocks");
}

function resizeBlock(w: number, h: number) {
  const grid = S.home.grid.map((c, j) => (j === selBlock ? { ...c, w, h } : c));
  void save({ home: { grid } });
}

function presetFromSel(): HomePreset | undefined {
  const i = Number(presetSel.slice(1));
  return presetSel.startsWith("b") ? BUILTIN_PRESETS[i] : S.home.presets[i];
}

function applyPreset(p: HomePreset) {
  selBlock = -1;
  homeNote = "";
  void save({ home: { header: p.header, grid: p.grid.map(c => ({ ...c })) } });
}

function savePreset() {
  const name = presetName.trim();
  if (!name) { homeNote = "Give the preset a name first."; render(true); return; }
  const mine: HomePreset = { name, header: S.home.header, grid: S.home.grid.map(c => ({ ...c })) };
  const rest = S.home.presets.filter(p => p.name !== name);  // same name replaces
  const presets = [...rest, mine];
  presetName = "";
  presetSel = `u${presets.length - 1}`;
  homeNote = `Saved "${name}".`;
  void save({ home: { presets } });
}

function deletePreset() {
  const i = Number(presetSel.slice(1));
  presetSel = "";
  void save({ home: { presets: S.home.presets.filter((_, j) => j !== i) } });
}

// ---- Shortcuts: tile list + editor for the selected tile ----
let selTile = 0;

function saveTiles(tiles: Tile[]) {
  void save({ tiles });
}

function shortcutsHtml(): string {
  const tiles = S.tiles;
  if (selTile >= tiles.length) selTile = Math.max(0, tiles.length - 1);
  const list = tiles.map((t, i) =>
    `<button type="button" class="tile${i === selTile ? " sel" : ""}" data-act="tile-select" data-idx="${i}" style="--tc:${esc(t.color)}">` +
    `<span>${esc(t.label || "(no label)")}</span><small>${esc(TILE_ACTIONS.find(a => a[0] === t.action)?.[1] ?? t.action)}</small></button>`,
  ).join("");
  const add = tiles.length < MAX_TILES
    ? `<button type="button" class="tile add" data-act="tile-add">${icon("plus", 14)}Add</button>` : "";
  const grid = card("Tiles", `The Shortcuts page on the board. Up to ${MAX_TILES}; 2, 4, 6, 8, 12 or 16 fill an even grid.`,
    `<div class="tiles">${list}${add}</div>` +
    row("Reset", `<button type="button" data-act="tiles-reset">${icon("refresh-cw", 13)}Example tiles</button>`,
      "Mic, sound, volume, screenshot, terminal, downloads, lock"));
  const t = tiles[selTile];
  if (!t) return grid;
  const act = TILE_ACTIONS.find(a => a[0] === t.action) ?? TILE_ACTIONS[0];
  const needsTarget = !["lock", "mic"].includes(t.action);
  const stateVal = t.state === null ? "auto" : t.state === "" ? "none" : t.state;
  const editor = card(`Tile ${selTile + 1}`, "Changes apply right away", [
    row("Label", `<input class="txt" data-k="tileLabel" value="${esc(t.label)}" maxlength="30" spellcheck="false">`),
    row("Icon", select(`data-k="tileIcon"`, TILE_ICONS.map(n => [n, n.replace(/_/g, " ")] as [string, string]), t.icon)),
    row("Colour", `<input type="color" class="color" data-k="tileColor" value="${esc(t.color)}">`),
    row("Action", select(`data-k="tileAction"`, TILE_ACTIONS.map(a => [a[0], a[1]] as [string, string]), t.action)),
    ...(needsTarget ? [row("Target", `<input class="txt" data-k="tileTarget" value="${esc(t.target)}" spellcheck="false">`, esc(act[2]))] : []),
    row("Lights up", select(`data-k="tileState"`, [["auto", "Automatic"], ["mic_mute", "While the mic is muted"],
      ["speaker_mute", "While the sound is muted"], ["none", "Never"]], stateVal)),
    row("Position",
      `<span class="updown">` +
      `<button type="button" class="ib" data-act="tile-move" data-d="-1" title="Move left"${selTile === 0 ? " disabled" : ""}>${icon("chevron-up", 12)}</button>` +
      `<button type="button" class="ib" data-act="tile-move" data-d="1" title="Move right"${selTile === tiles.length - 1 ? " disabled" : ""}>${icon("chevron-down", 12)}</button></span>` +
      `<button type="button" data-act="tile-remove">${icon("x", 13)}Remove</button>`),
  ].join(""));
  return grid + editor;
}

function tilePatch(change: Partial<Tile>) {
  saveTiles(S.tiles.map((t, i) => (i === selTile ? { ...t, ...change } : t)));
}

// ---- Games: Steam, presets, your own games ----
let selGame = -1;

function gamesHtml(): string {
  const g = S.games;
  const sources = card("Sources", "What the board's Games page shows", [
    row("Steam", tgl(`data-k="gamesSteam"`, g.steam), "Playing now, recent games and launching them. Needs Steam installed"),
    row("Popular games", tgl(`data-k="gamesPresets"`, g.presets), "League, VALORANT, Fortnite, Minecraft and more, without setup"),
  ].join(""));
  const presets = g.presets
    ? card("Popular games", "Turn off the ones you don't want",
      GAME_PRESETS.map(n => row(esc(n), tgl(`data-k="presetGame" data-name="${esc(n)}"`, !g.presets_disabled.includes(n)))).join(""))
    : "";
  const list = g.custom.map((c, i) =>
    row(esc(c.name || "(no name)"),
      `<button type="button" data-act="game-edit" data-idx="${i}">${icon(i === selGame ? "chevron-up" : "chevron-down", 13)}${i === selGame ? "Close" : "Edit"}</button>` +
      `<button type="button" class="ib" data-act="game-remove" data-idx="${i}" title="Remove">${icon("x", 12)}</button>`,
      esc(c.process.join(", ") || c.title)) + (i === selGame ? gameEditor(c) : "")).join("");
  const own = card("Your games", "Games that aren't on Steam: found by program name or window title",
    (list || text("None yet.")) +
    row("Add a game", `<button type="button" data-act="game-add">${icon("plus", 13)}Add</button>`));
  return sources + own + presets;
}

function gameEditor(c: CustomGame): string {
  const f = (k: string, label: string, v: string, hint: string) =>
    row(label, `<input class="txt" data-k="${k}" value="${esc(v)}" spellcheck="false">`, hint);
  return `<div class="gedit">` + [
    f("gameName", "Name", c.name, "Shown on the board; a popular game's name replaces it"),
    f("gameProcess", "Program", c.process.join(", "), "File name from Task Manager > Details, e.g. javaw.exe (comma for several)"),
    f("gameMatch", "Command line has", c.match, "Optional, e.g. minecraft for javaw.exe"),
    f("gameTitle", "Window title has", c.title, "Optional; with a title the program is optional"),
    row("Picture",
      `<input class="txt" data-k="gameImage" value="${esc(c.image)}" spellcheck="false">` +
      `<button type="button" data-act="game-image">Browse...</button>`,
      "Optional: shown on the board instead of the initials"),
    f("gameLaunch", "Start with", c.launch, "Optional: a program, shortcut, folder or URL"),
  ].join("") + `</div>`;
}

function gamePatch(change: Partial<CustomGame>) {
  void save({ games: { custom: S.games.custom.map((c, i) => (i === selGame ? { ...c, ...change } : c)) } });
}

// Claude Code wiring, fetched when the AI section opens.
// ---- Media: Plex server (Sign in with Plex) ----
/** The sign-in page while waiting for approval ("" = not signing in). */
let plexUrl = "";
let plexNote = "";
let plexServers: PlexServer[] | null = null;
let plexLoading = false;
/** The last "media_server" message the app sent the board: is the server reachable? */
type ServerState = { configured: boolean; online?: boolean; version?: string; library?: string; streams?: number; error?: string };
let plexState: ServerState | null = null;

function serverState(f: SimFrame): ServerState | null {
  if (!("json" in f)) return null;
  const j = f.json as { type?: string };
  return j && j.type === "media_server" ? (j as ServerState) : null;
}

/** Keeps plexState current; redraws the Media section when what it shows changes. */
function watchServer() {
  const key = (x: ServerState | null) => JSON.stringify(x && [x.configured, x.online, x.version, x.library, x.streams, x.error]);
  const take = (f: SimFrame) => {
    const next = serverState(f);
    if (!next) return;
    const changed = key(next) !== key(plexState);
    plexState = next;
    if (changed && section === "media") render();
  };
  api.simState().then(fs => fs.forEach(take)).catch(() => undefined);
  void api.onSimFrame(take);
}

function plexStatusRow(): string {
  const p = plexState;
  if (!S.media_server.server_id) return "";
  if (!p || !p.configured) return row("Status", "", "Checking the server");
  if (p.online) {
    const playing = p.streams ? `${p.streams} playing` : "Nothing playing";
    return row(`${icon("check", 14)}Connected`, "", esc([p.version, p.library, playing].filter(Boolean).join(" - ")));
  }
  return row(`${icon("x", 14)}Can't reach the server`, "", esc(p.error || "No answer yet; the app tries again every minute"));
}

function loadPlexServers() {
  if (plexLoading) return;
  plexLoading = true;
  api.plexServers()
    .then(list => { plexServers = list; })
    .catch(err => { plexServers = []; plexNote = String(err); })
    .finally(() => { plexLoading = false; if (section === "media") render(true); });
}

function mediaHtml(): string {
  const m = S.media_server;
  if (!m.signed_in) {
    const rows = plexUrl
      ? [
          row("Waiting for Plex", `<button type="button" data-act="plex-cancel">${icon("x", 13)}Cancel</button>`,
            "Approve Desk Hub in the browser window that opened. No window? Open this address:"),
          `<div class="swr"><input class="txt" readonly value="${esc(plexUrl)}" spellcheck="false" style="width:100%"></div>`,
        ]
      : [row("Plex", `<button type="button" data-act="plex-sign-in">${icon("zap", 13)}Sign in with Plex</button>`,
          "Shows what plays on your Plex server on the board's Plex page")];
    return card("Media server", "Plex now playing on the board", [...rows, ...(plexNote ? [text(plexNote)] : [])].join(""));
  }
  if (!plexServers) loadPlexServers();
  const list = plexServers ?? [];
  const known = list.some(s => s.id === m.server_id);
  const options: [string, string][] = [
    ...(m.server_id && !known ? [[m.server_id, m.server_name || "Saved server"] as [string, string]] : []),
    ...(m.server_id ? [] : [["", "Pick a server"] as [string, string]]),
    ...list.map(s => [s.id, s.owned ? s.name : `${s.name} (shared)`] as [string, string]),
  ];
  const server = plexServers === null
    ? row("Server", "", "Loading your servers")
    : row("Server", select(`data-k="plexServer"`, options, m.server_id),
        list.length ? "Everyone playing on it shows; the board picks one and counts the rest" : "No Plex Media Server on this account");
  return card("Media server", "Plex now playing on the board", [
    row("Signed in to Plex", `<button type="button" data-act="plex-sign-out">${icon("x", 13)}Sign out</button>`),
    server,
    plexStatusRow(),
    ...(plexNote ? [text(plexNote)] : []),
  ].join(""));
}

async function plexSignIn() {
  plexNote = "";
  try {
    plexUrl = await api.plexSignIn();
  } catch (err) {
    plexNote = String(err);
  }
  render(true);
}

let claude: ClaudeState | null = null;
let claudeNote = "";
let claudeLoading = false;

function aiHtml(): string {
  const a = S.ai;
  if (!claude && !claudeLoading) {
    claudeLoading = true;
    api.claudeStatus()
      .then(c => { claude = c; claudeNote = ""; })
      .catch(err => { claude = { statusline: false, hooks: false, cli: false }; claudeNote = String(err); })
      .finally(() => { claudeLoading = false; render(true); });
  }
  const linked = !!claude && claude.statusline && claude.hooks;
  const usage = card("Usage", "What the board's AI page shows", [
    row("Agent", select(`data-k="aiAgent"`, [["auto", "Most recently used"], ["claude", "Claude Code"], ["codex", "Codex"]], a.agent)),
    row("Live limits", tgl(`data-k="aiLive"`, a.live_limits), "Reads the 5-hour and weekly limits from Anthropic with your Claude Code sign-in (no tokens used)"),
  ].join(""));
  const cc = card("Claude Code", "Limits and waiting alerts come from Claude Code", [
    row(linked ? "Connected" : "Not connected",
      `<button type="button" data-act="claude">${icon(linked ? "x" : "zap", 13)}${linked ? "Disconnect" : "Connect"}</button>`,
      claude && !claude.cli ? "deskhub-cli is missing: reinstall Desk Hub" :
        "Adds Desk Hub's status line and hooks to Claude Code's settings (a backup is kept)"),
    ...(claudeNote ? [text(esc(claudeNote))] : []),
  ].join(""));
  const r = remote;
  const server = card("Claude Code on a server", "Alerts and usage from a deskhub-relay, e.g. on a home server", [
    row("Relay address", `<input class="txt" data-k="aiRemoteUrl" value="${esc(a.remote_url)}" placeholder="http://192.168.1.10:8791" spellcheck="false">`,
      "Empty = off. Plain HTTP: use it on your own network only"),
    row("Token", `<input class="txt" type="password" data-k="aiRemoteToken" value="" placeholder="${r?.token_saved ? "Saved (type to replace)" : "DESKHUB_RELAY_TOKEN"}" autocomplete="off" spellcheck="false">`,
      "Kept in Windows Credential Manager"),
    ...(a.remote_url ? [remoteStatusRow()] : []),
    row("Show on PC opens", `<input class="txt" data-k="aiRemoteOpen" value="${esc(a.remote_open)}" placeholder="Termius.exe" spellcheck="false">`,
      a.remote_open.trim()
        ? "A server's alert shows Show on PC: it brings this app to the front (or starts it). No Allow / Deny on the board"
        : "Empty: Allow / Deny on the board for a server's permission requests. A program (Termius.exe), its full path or a URL"),
    ...(remoteNote ? [text(remoteNote)] : []),
  ].join(""));
  const alerts = card("Alerts", "Shows on the board when Claude Code waits for you", [
    row("Waiting alerts", tgl(`data-k="aiAlerts"`, a.alerts)),
    row("Idle alert after", `<input class="num" type="number" min="0" max="3600" data-k="aiIdle" value="${a.idle_alert_seconds}"> s`,
      "An idle session alerts after this many seconds"),
  ].join(""));
  const log = card("Recent alerts", "What made the board alert, newest first. Updates every 3 s", [
    ...(alertLog.length ? alertLog.map(alertLogRow) : [text("No alerts yet")]),
    row("Log file", `<button type="button" data-act="alert-log-open">${icon("list", 13)}Open folder</button>`,
      "alert_log.jsonl in .deskhub: every hook event and alert, the newest 2000 lines"),
    ...(alertLogNote ? [text(alertLogNote)] : []),
  ].join(""));
  return usage + cc + server + alerts + log;
}

// Settings > AI > Recent alerts: the alert log, asked with the server status every 3 s.
let alertLog: AlertLogEntry[] = [];
let alertLogNote = "";
const ALERT_KINDS: Record<string, string> = {
  shown: "Alert shown", cleared: "Alert cleared", dismissed: "Dismissed", show: "Show tapped", allow: "Allowed", deny: "Denied",
};

function alertLogRow(e: AlertLogEntry): string {
  const time = new Date(e.t * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  const what = e.kind === "event" ? (e.cause || e.state) : (ALERT_KINDS[e.kind] ?? e.kind);
  const where = e.project ? `${e.source} - ${e.project}` : e.source;
  const why = e.kind === "event" ? e.state : (e.cause || e.state);
  const hint = [where, why, e.text].filter(Boolean).join(" · ");
  return row(esc(`${time}  ${what}`), "", esc(hint));
}

// Claude Code on a server: the poller's status, asked every 3 s while the AI section is open.
let remote: RemoteStatus | null = null;
let remoteNote = "";

function remoteStatusRow(): string {
  const r = remote;
  if (!r) return row("Status", "", "Checking the server");
  if (!r.token_saved) return row(`${icon("x", 14)}No token`, "", "Enter the relay's token");
  if (r.ok) {
    const n = r.sessions === 1 ? "1 session" : `${r.sessions} sessions`;
    return row(`${icon("check", 14)}Connected`, "", esc(`${r.host} - ${n}`));
  }
  return row(`${icon("x", 14)}Can't reach the relay`, "", esc(r.error || "No answer yet"));
}

function pollRemote() {
  const key = (x: RemoteStatus | null) => JSON.stringify(x);
  setInterval(() => {
    if (section !== "ai") return;
    api.alertLog(50).then(next => {
      if (JSON.stringify(next) === JSON.stringify(alertLog)) return;
      alertLog = next;
      if (section === "ai") render(true);
    }).catch(() => undefined);
    api.remoteStatus().then(next => {
      if (key(next) === key(remote)) return;
      remote = next;
      if (section === "ai") render(true);
    }).catch(() => undefined);
  }, 3000);
}
pollRemote();

async function toggleClaude() {
  const linked = !!claude && claude.statusline && claude.hooks;
  try {
    claude = await api.claudeConnect(!linked);
    claudeNote = "";
  } catch (err) {
    claudeNote = String(err);
  }
  render(true);
}

function appearanceHtml(): string {
  return card("Theme", "Applies at once", [
    row("Colour theme", select(`data-k="theme"`, THEMES, S.appearance.theme)),
    row("Board colours", select(`data-k="boardTheme"`, BOARD_THEMES, S.appearance.board_theme),
      "The board restarts once when its colours change"),
  ].join(""));
}

function connectionHtml(): string {
  return card("Board", "Where Desk Hub finds the board", [
    row("Host", `<input class="txt" data-k="host" value="${esc(S.board.host)}" placeholder="deskhub.local" spellcheck="false">`,
      "Name or IP address of the board"),
    row("Token",
      `<input class="txt" type="${showToken ? "text" : "password"}" data-k="token" value="${esc(S.board.token)}" autocomplete="off" spellcheck="false">` +
      `<button type="button" class="ib" data-act="eye" title="${showToken ? "Hide" : "Show"} token">${icon(showToken ? "eye-off" : "eye", 14)}</button>`,
      "Must match the token on the board"),
    row("Pair", `<button type="button" data-act="pair"${pairCode ? " disabled" : ""}>${icon("zap", 13)}Pair with the board</button>`,
      pairCode ? "" : "Gets the token from the board: you tap Allow on it"),
    ...(pairCode ? [row("Code", `<span class="paircode">${esc(pairCode)}</span>`,
      "Tap Allow on the board if it shows this same code (within a minute)")] : []),
    ...(pairNote ? [text(pairNote)] : []),
    row("Config file", `<button type="button" data-act="import">${icon("refresh-cw", 13)}Import from config.toml</button>`,
      S.imported ? "Already imported" : "Copies host and token from the Python companion's config"),
  ].join("")) + internetHtml() + usbHtml() + firmwareHtml();
}

/** The board's Internet page: latency, data used and speed tests run by the app. */
function internetHtml(): string {
  const n = S.internet;
  return card("Internet", "The board's Network page", [
    row("Show public IP", tgl(`data-k="netIp"`, n.show_ip), "Looked up at speed.cloudflare.com every 30 minutes"),
    row("Automatic speed test", `every <input class="num" type="number" min="0" max="168" step="1" data-k="netHours" value="${n.speedtest_hours}"> h`,
      "0 = only when you tap Test on the board. Each test uses about 35 MB"),
  ].join(""));
}

// Pairing (Board card): the code shown while the board's prompt is open.
let pairCode = "";
let pairNote = "";

async function pair() {
  pairNote = "";
  try {
    pairCode = await api.boardPairCode();
    render(true);
    await api.boardPair(pairCode);
    pairNote = "Paired. The token is saved.";
  } catch (err) {
    pairNote = `Not paired: ${String(err).replace(/^Error: /, "")}`;
  }
  pairCode = "";
  render(true);
}

// First setup over the USB cable (USB setup card).
let usbBoard: UsbBoard | null = null;
let usbNets: UsbNetwork[] = [];
let usbSsid = "";
let usbBusy = "";
let usbNote = "";

function usbHtml(): string {
  const b = usbBoard;
  const rows = [
    row("Board on USB",
      `<button type="button" data-act="usb-find"${usbBusy ? " disabled" : ""}>${icon("refresh-cw", 13)}${b ? "Find again" : "Find board"}</button>`,
      usbBusy === "find" ? "Looking on the USB ports..."
        : b ? esc(`${b.port}: ${b.version}${b.variant ? ` (${b.variant})` : ""}. ${b.token_saved ? "Token saved." : "The board has no token yet."}`)
        : "Plug the board into this PC with a USB data cable"),
  ];
  if (b) {
    rows.push(row("Wi-Fi now", `<span>${esc(b.ssid ? `${b.ssid}${b.ip ? ` (${b.ip})` : ""}` : "Not connected")}</span>`));
    const opts: [string, string][] = usbNets.map(n => [n.ssid, `${n.ssid}${n.secure ? "" : " (open)"}`]);
    rows.push(row("Network",
      (opts.length ? select(`data-k="usbSsid"`, opts, usbSsid, !!usbBusy) : `<span class="muted">${usbBusy === "scan" ? "Scanning..." : "No networks yet"}</span>`) +
      `<button type="button" class="ib" data-act="usb-scan" title="Scan again"${usbBusy ? " disabled" : ""}>${icon("refresh-cw", 13)}</button>`,
      "Networks the board sees"));
    rows.push(row("Password",
      `<input class="txt" type="password" data-k="usbPass" autocomplete="off" spellcheck="false" placeholder="Empty for an open network"${usbBusy ? " disabled" : ""}>`));
    rows.push(row("", `<button type="button" data-act="usb-wifi"${usbBusy || !usbSsid ? " disabled" : ""}>Set up Wi-Fi</button>`,
      usbBusy === "wifi" ? "Joining (up to 40 seconds)..." : "The board saves the network and joins it"));
  }
  if (usbNote) rows.push(text(usbNote));
  return card("USB setup", "First setup with a cable: no typing on the board", rows.join(""));
}

async function usbRun(kind: string, job: () => Promise<void>) {
  usbBusy = kind;
  usbNote = "";
  render(true);
  try {
    await job();
  } catch (err) {
    usbNote = String(err).replace(/^Error: /, "");
  }
  usbBusy = "";
  render(true);
}

const usbScan = () => usbRun("scan", async () => {
  if (!usbBoard) return;
  usbNets = await api.usbScan(usbBoard.port);
  if (!usbNets.some(n => n.ssid === usbSsid)) usbSsid = usbNets[0]?.ssid ?? "";
});

function usbFind() {
  void usbRun("find", async () => {
    usbBoard = null;
    usbNets = [];
    usbBoard = await api.usbFind();
  }).then(() => { if (usbBoard) void usbScan(); });
}

function usbWifi() {
  const board = usbBoard;
  const pass = main.querySelector<HTMLInputElement>('[data-k="usbPass"]')?.value ?? "";
  if (!board || !usbSsid) return;
  if (pass && (pass.length < 8 || pass.length > 64)) {
    usbNote = "Wi-Fi passwords have 8 to 64 characters (or none for an open network).";
    render(true);
    return;
  }
  void usbRun("wifi", async () => {
    const j = await api.usbWifi(board.port, usbSsid, pass);
    board.ssid = usbSsid;
    board.ip = j.ip;
    usbNote = `Connected to ${usbSsid}${j.ip ? ` as ${j.ip}` : ""}. Desk Hub finds the board there now.`;
  });
}

function firmwareHtml(): string {
  const connected = st.state === "connected";
  if (connected && fwCheckFor !== st.firmware) void refreshFwCheck();
  const c = fwCheck;
  const hint = fwBusy ? "Tap Install on the board within a minute"
    : !connected ? "Connect to the board first"
    : c?.blocked ?? c?.error ?? (c?.update_available ? `Version ${c.latest} is ready; the board asks before installing`
      : c?.latest ? "The board is up to date" : "Checking GitHub...");
  return card("Firmware", "Board updates from GitHub releases", [
    row("Board", `<span>${esc(st.firmware ?? "-")}${st.variant ? ` <span class="muted">(${esc(st.variant)})</span>` : ""}</span>`),
    row("Newest release", `<span>${esc(c?.latest ?? "-")}</span>` +
      `<button type="button" class="ib" data-act="fw-recheck" title="Check again"${fwChecking || !connected ? " disabled" : ""}>${icon("refresh-cw", 13)}</button>`),
    row(fwBusy ? "Updating..." : "Update",
      `<button type="button" data-act="firmware-github"${fwBusy || !connected || !c?.update_available ? " disabled" : ""}>${icon("refresh-cw", 13)}Update board</button>`,
      hint),
    row("From a file",
      `<button type="button" data-act="firmware"${fwBusy || !connected ? " disabled" : ""}>Choose deskhub.bin</button>`,
      "A local build (out/<board>/deskhub.bin, not the merged image)"),
    ...(fwNote ? [text(esc(fwNote))] : []),
  ].join(""));
}

// Newest release for the connected board; checked again whenever its firmware version changes.
let fwCheck: FirmwareCheck | null = null;
let fwCheckFor: string | null | undefined;
let fwChecking = false;

async function refreshFwCheck() {
  fwCheckFor = st.firmware;
  fwChecking = true;
  try {
    fwCheck = await api.firmwareCheck();
  } catch (err) {
    fwCheck = { latest: null, update_available: false, blocked: null, error: String(err) };
  }
  fwChecking = false;
  if (section === "connection") render(true);
}

// Firmware upload state (Connection section).
let fwBusy = false;
let fwNote = "";

async function updateFirmware(fromGithub = false) {
  fwBusy = true;
  fwNote = "";
  render(true);
  try {
    const answer = fromGithub ? await api.boardFirmwareGithub() : await api.boardFirmware();
    fwNote = answer.includes("installed") ? "Installed. The board restarts and reconnects in a few seconds." : answer;
  } catch (err) {
    const msg = String(err);
    fwNote = msg.includes("No file chosen") ? "" : `Not installed: ${msg}`;
  }
  fwBusy = false;
  render(true);
}

/** Start with Windows, as Windows reports it. Busy: a change is on its way (one at a time). */
let autostart = true;
let autostartBusy = false;
let autostartNote = "";

function aboutHtml(): string {
  return card("Desk Hub", "Board companion for Windows", [
    row("Version", `<span>${esc(version)}</span>`, TAURI ? esc(aboutUpdate) : ""),
    row("Start with Windows", tgl(`data-k="autostart"`, autostart, autostartBusy),
      autostartNote || "Hidden in the tray, so the board stays connected"),
    TAURI ? row("Updates", `<button type="button" data-act="update-check"${updChecking ? " disabled" : ""}>${icon("refresh-cw", 13)}Check now</button>`,
      "New versions download in the background; the button at the top installs them") : "",
    text("Replaces the Python companion for media, PC stats, Claude usage, shortcuts and games."),
    text("Printers still need the Python companion. Don't run both at once."),
  ].join(""));
}

// ---- events ----
function stepSlider(k: SliderKey, d: number) {
  const spec = SLIDER_SPEC[k];
  const v = Math.min(spec.max, Math.max(spec.min, Math.round((sliderValue(k) + d * 5) / 5) * 5));
  setSliderLocal(k, v);
  syncSlider(k);
  saveLater(k, sliderPatch(k, v));
}

async function importConfig() {
  try {
    S = await api.importConfig();
  } catch (err) {
    // Only when no file was found: let the user pick it. Other errors (a broken file)
    // are not about the location.
    const path = String(err).includes("No config.toml found") ? await api.pickConfig() : null;
    if (path) {
      try {
        S = await api.importConfig(path);
      } catch (err2) {
        console.error("import_config failed", err2);
      }
    } else {
      console.error("import_config failed", err);
    }
  }
  applyTheme();
  render(true);
}

async function act(el: HTMLElement) {
  switch (el.dataset.act) {
    case "page-up": movePage(el.dataset.page as Page, -1); return;
    case "page-down": movePage(el.dataset.page as Page, 1); return;
    case "grid-remove": removeCell(Number(el.dataset.idx)); return;
    case "grid-select": {
      const i = Number(el.dataset.idx);
      selBlock = selBlock === i ? -1 : i;
      render(true);
      return;
    }
    case "preset-apply": { const p = presetFromSel(); if (p) applyPreset(p); return; }
    case "preset-delete": deletePreset(); return;
    case "preset-save": savePreset(); return;
    case "home-reset": applyPreset(BUILTIN_PRESETS[0]); return;
    case "cal-add":
      saveFeeds([...S.calendar.feeds, { name: "", url: "", color: FEED_COLOR }]);
      render(true);
      return;
    case "cal-remove":
      saveFeeds(S.calendar.feeds.filter((_, i) => i !== Number(el.dataset.i)));
      render(true);
      return;
    case "sound-test": {
      const ev = el.dataset.ev as "alert" | "timer" | "reminder";
      api.soundTest(S.sound[ev]).catch(() => undefined);
      return;
    }
    case "preview":
      preview = el.dataset.v === "sim" ? "sim" : "live";
      render(true);
      return;
    case "tile-select": selTile = Number(el.dataset.idx); render(true); return;
    case "tile-add": {
      const tiles = [...S.tiles, { label: "New tile", icon: "apps", color: TILE_COLORS.neutral, action: "keys" as TileAction, target: "", state: null }];
      selTile = tiles.length - 1;
      saveTiles(tiles);
      return;
    }
    case "tile-remove": saveTiles(S.tiles.filter((_, i) => i !== selTile)); return;
    case "tile-move": {
      const to = selTile + Number(el.dataset.d);
      if (to < 0 || to >= S.tiles.length) return;
      const tiles = [...S.tiles];
      [tiles[selTile], tiles[to]] = [tiles[to], tiles[selTile]];
      selTile = to;
      saveTiles(tiles);
      return;
    }
    case "tiles-reset": selTile = 0; saveTiles(EXAMPLE_TILES.map(t => ({ ...t }))); return;
    case "game-edit": { const i = Number(el.dataset.idx); selGame = selGame === i ? -1 : i; render(true); return; }
    case "game-remove": {
      const i = Number(el.dataset.idx);
      selGame = -1;
      void save({ games: { custom: S.games.custom.filter((_, j) => j !== i) } });
      return;
    }
    case "game-add": {
      const custom = [...S.games.custom, { name: "", process: [], match: "", title: "", image: "", launch: "" }];
      selGame = custom.length - 1;
      void save({ games: { custom } });
      return;
    }
    case "step": stepSlider(el.dataset.slider as SliderKey, Number(el.dataset.d)); return;
    case "eye": showToken = !showToken; render(true); return;
    case "claude": await toggleClaude(); return;
    case "alert-log-open":
      api.alertLogOpen().then(() => { alertLogNote = ""; }, err => { alertLogNote = String(err); }).finally(() => render(true));
      return;
    case "plex-sign-in": await plexSignIn(); return;
    case "ms-sign-in": await msSignIn(); return;
    case "gh-sign-in": await ghSignIn(); return;
    case "gh-copy": if (ghCode) void navigator.clipboard.writeText(ghCode.user_code).catch(() => {}); return;
    case "gh-cancel": ghCode = null; await api.ghSignInCancel(); render(true); return;
    case "gh-sign-out":
      ghNote = "";
      try {
        await api.ghSignOut();
        gh = await api.githubStatus();
      } catch (err) {
        ghNote = String(err);
      }
      render(true);
      return;
    case "ms-cancel": msUrl = ""; await api.msSignInCancel(); render(true); return;
    case "ms-sign-out": msNote = ""; await api.msSignOut(); render(true); return;
    case "plex-cancel": plexUrl = ""; await api.plexSignInCancel(); render(true); return;
    case "plex-sign-out":
      plexServers = null; plexNote = "";
      try { await api.plexSignOut(); } catch (err) { plexNote = String(err); render(true); }
      return;
    case "import": await importConfig(); return;
    case "update-check": checkUpdates(); return;
    case "firmware": await updateFirmware(); return;
    case "pair": await pair(); return;
    case "usb-find": usbFind(); return;
    case "usb-scan": void usbScan(); return;
    case "usb-wifi": usbWifi(); return;
    case "firmware-github": await updateFirmware(true); return;
    case "fw-recheck": render(true); await refreshFwCheck(); return;
    case "game-image": {
      const path = await api.pickImage();
      if (path) gamePatch({ image: path });
      return;
    }
  }
}

main.addEventListener("click", e => {
  const t = e.target as HTMLElement;
  if (Date.now() - dragEndedAt < 250 && t.closest(".hblock") && !t.closest(".ib")) return;
  if (t.id === "shot") {
    tap(e as MouseEvent, t as HTMLImageElement);
    return;
  }
  const el = t.closest<HTMLElement>("[data-act]");
  if (el) void act(el);
});

main.addEventListener("input", e => {
  const t = e.target as HTMLInputElement;
  if (t.type !== "range" || !t.dataset.slider) return;
  const k = t.dataset.slider as SliderKey;
  const v = Number(t.value);
  setSliderLocal(k, v);
  syncSlider(k);
  saveLater(k, sliderPatch(k, v));
});

main.addEventListener("change", e => {
  const t = e.target as HTMLInputElement | HTMLSelectElement;
  const k = t.dataset.k;
  if (!k) return;
  const on = (t as HTMLInputElement).checked === true;
  switch (k) {
    case "plexServer": {
      const srv = plexServers?.find(s => s.id === t.value);
      if (srv) api.plexSelectServer(srv.id, srv.name).catch(err => { plexNote = String(err); render(true); });
      return;
    }
    case "nightOn":
      void save({ display: { night: on ? "22:00-07:00" : "" } });
      return;
    case "nightFrom":
    case "nightTo": {
      const from = main.querySelector<HTMLInputElement>('[data-k="nightFrom"]')?.value ?? "";
      const to = main.querySelector<HTMLInputElement>('[data-k="nightTo"]')?.value ?? "";
      if (from && to) void save({ display: { night: `${from}-${to}` } });
      return;
    }
    case "nightStyle":
      void save({ display: { night_style: t.value as Settings["display"]["night_style"] } });
      return;
    case "pcSleep":
      void save({ display: { pc_sleep: t.value as Settings["display"]["pc_sleep"] } });
      return;
    case "nav":
      void save({ pages: { nav: t.value as Settings["pages"]["nav"] } });
      return;
    case "boardTheme":
      void save({ appearance: { board_theme: t.value as Settings["appearance"]["board_theme"] } });
      return;
    case "theme":
      S.appearance.theme = t.value as Settings["appearance"]["theme"];
      applyTheme();
      void save({ appearance: { theme: S.appearance.theme } });
      return;
    case "locCity": {
      const city = t.value.trim();
      if (city) saveLocation({ city });
      else render(true);
      return;
    }
    case "locLat":
    case "locLon": {
      const v = t.value.trim();
      const max = k === "locLat" ? 90 : 180;
      if (/^-?\d{1,3}(\.\d+)?$/.test(v) && Math.abs(Number(v)) <= max) saveLocation(k === "locLat" ? { lat: v } : { lon: v });
      else render(true);
      return;
    }
    case "locTz": saveLocation({ tz: t.value }); return;
    case "host": {
      const host = t.value.trim();
      if (host) void save({ board: { host } });
      else render(true);
      return;
    }
    case "token":
      void save({ board: { token: t.value } });
      return;
    case "usbSsid": usbSsid = t.value; render(true); return;
    case "usbPass": return;
    case "pageOn": {
      const p = t.dataset.page as Page;
      const list = onList();
      void save({ pages: { enabled: on ? [...list, p] : list.filter(x => x !== p) } });
      return;
    }
    case "homeHeader":
      void save({ home: { header: on } });
      return;
    case "tileLabel": tilePatch({ label: t.value.slice(0, 30) }); return;
    case "tileIcon": tilePatch({ icon: t.value }); return;
    case "tileColor": tilePatch({ color: t.value }); return;
    case "tileAction": tilePatch({ action: t.value as TileAction, target: "" }); return;
    case "tileTarget": tilePatch({ target: t.value.trim() }); return;
    case "tileState": tilePatch({ state: t.value === "auto" ? null : t.value === "none" ? "" : t.value }); return;
    case "gamesSteam": void save({ games: { steam: on } }); return;
    case "gamesPresets": void save({ games: { presets: on } }); return;
    case "presetGame": {
      const n = t.dataset.name ?? "";
      const off = S.games.presets_disabled.filter(x => x !== n);
      void save({ games: { presets_disabled: on ? off : [...off, n] } });
      return;
    }
    case "gameName": gamePatch({ name: t.value.trim() }); return;
    case "gameProcess": gamePatch({ process: t.value.split(",").map(p => p.trim()).filter(Boolean) }); return;
    case "gameMatch": gamePatch({ match: t.value.trim() }); return;
    case "gameTitle": gamePatch({ title: t.value.trim() }); return;
    case "gameImage": gamePatch({ image: t.value.trim() }); return;
    case "gameLaunch": gamePatch({ launch: t.value.trim() }); return;
    case "homePreset":
      presetSel = t.value;
      render(true);
      return;
    case "presetName":
      presetName = t.value;
      return;
    case "blockW":
    case "blockH": {
      const c = S.home.grid[selBlock];
      if (c) resizeBlock(k === "blockW" ? Number(t.value) : c.w, k === "blockH" ? Number(t.value) : c.h);
      return;
    }
    case "locH24": saveLocation({ h24: on }); return;
    case "autostart":
      if (autostartBusy) return;
      autostartBusy = true;
      autostartNote = "";
      void api.autostartSet(on)
        .then(v => { autostart = v; }, err => { autostartNote = `Windows refused: ${err}`; })
        .finally(() => { autostartBusy = false; render(true); });
      return;
    case "locF": saveLocation({ fahrenheit: on }); return;
    case "clockSeconds":
      void save({ display: { clock_seconds: on } });
      return;
    case "chartHours":
      void save({ display: { chart_hours: Number(t.value) } });
      return;
    case "aiAgent":
      void save({ ai: { agent: t.value as Settings["ai"]["agent"] } });
      return;
    case "aiLive":
      void save({ ai: { live_limits: on } });
      return;
    case "aiAlerts":
      void save({ ai: { alerts: on } });
      return;
    case "aiRemoteUrl": {
      const v = t.value.trim().replace(/\/+$/, "");
      remoteNote = v && !/^https?:\/\/[^\s/]+$/.test(v) ? "Use an address like http://192.168.1.10:8791" : "";
      if (!remoteNote) void save({ ai: { remote_url: v } });
      else render(true);
      return;
    }
    case "aiRemoteOpen": void save({ ai: { remote_open: t.value.trim() } }); return;
    case "aiRemoteToken":
      void api.remoteTokenSet(t.value)
        .then(() => { remoteNote = ""; t.value = ""; return api.remoteStatus(); })
        .then(s => { remote = s; }, err => { remoteNote = String(err); })
        .finally(() => render(true));
      return;
    case "calName": case "calUrl": case "calColor": {
      const i = Number(t.dataset.i);
      const field = k === "calName" ? "name" : k === "calUrl" ? "url" : "color";
      saveFeeds(S.calendar.feeds.map((f, j) => (j === i ? { ...f, [field]: t.value.trim() } : f)));
      return;
    }
    case "netIp": void save({ internet: { show_ip: on } }); return;
    case "netHours": {
      const v = Math.max(0, Math.min(168, Number(t.value) || 0));
      void save({ internet: { speedtest_hours: v } });
      return;
    }
    case "msClient": void save({ calendar: { ms_client_id: t.value.trim() } }); return;
    case "msTenant": void save({ calendar: { ms_tenant: t.value.trim() } }); return;
    case "msName": void save({ calendar: { ms_name: t.value.trim() } }); return;
    case "msColor": void save({ calendar: { ms_color: t.value } }); return;
    case "calDays": {
      const v = Math.max(1, Math.min(60, Math.round(Number(t.value) || 7)));
      void save({ calendar: { days: v } });
      return;
    }
    case "ghToken": {
      const token = t.value;
      void api.githubTokenSet(token)
        .then(() => {
          ghNote = "";
          t.value = "";
          // A new token shows the GitHub page on the board.
          if (token.trim() && !S.pages.enabled.includes("github")) void save({ pages: { enabled: [...S.pages.enabled, "github"] } });
          return api.githubStatus();
        })
        .then(s => { gh = s; }, err => { ghNote = String(err); })
        .finally(() => render(true));
      return;
    }
    case "soundMute": void save({ sound: { mute: on } }); return;
    case "sound": {
      const ev = t.dataset.ev as "alert" | "timer" | "reminder";
      void save({ sound: { [ev]: t.value as SoundName } });
      return;
    }
    case "soundRemind": {
      const v = Math.max(0, Math.min(120, Math.round(Number(t.value) || 0)));
      void save({ sound: { remind_minutes: v } });
      return;
    }
    case "aiIdle": {
      const v = Math.max(0, Math.min(3600, Math.round(Number(t.value) || 0)));
      void save({ ai: { idle_alert_seconds: v } });
      return;
    }
    case "homeAdd": {
      const w = t.value as Widget;
      if (!w) return;
      const cell = findSpot(w);
      homeNote = cell ? "" : "No free space. Remove a block first.";
      if (cell) void save({ home: { grid: [...S.home.grid, cell] } });
      else render(true);
      return;
    }
  }
});

nav.addEventListener("click", e => {
  const b = (e.target as HTMLElement).closest<HTMLElement>("[data-sec]");
  if (!b) return;
  section = b.dataset.sec as SectionId;
  if (section === "ai") claude = null; // re-read Claude Code's settings each visit
  if (section === "calendar") calendarTick();
  renderNav();
  render(true);
  main.scrollTop = 0;
});

document.getElementById("minBtn")?.addEventListener("click", () => void win.minimize());
document.getElementById("closeBtn")?.addEventListener("click", () => void win.hide());
document.getElementById("grip")?.addEventListener("mousedown", e => {
  if (e.button !== 0) return;
  e.preventDefault();
  void win.startResize();
});

/** App updates (update.ts): the title bar button shows once a new version is downloaded. */
async function startUpdates() {
  if (!TAURI) return;
  const btn = document.getElementById("updBtn") as HTMLButtonElement;
  const { startUpdater } = await import("./update");
  const up = await startUpdater(s => {
    btn.hidden = !s.ready && !s.installing;
    btn.disabled = s.installing;
    btn.textContent = s.installing ? "Installing..." : `Update to ${s.version ?? ""}`;
    btn.title = s.error ?? "Installs the new version and restarts Desk Hub";
    const when = s.checked ? ` (checked ${new Date(s.checked * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })})` : "";
    aboutUpdate = s.checking ? "Checking for updates..."
      : s.ready ? `Version ${s.version} is ready: use the button at the top.`
      : s.error ? `Update check failed: ${s.error}`
      : s.checked ? `Up to date${when}` : "Checks every 10 minutes";
    updChecking = s.checking;
    if (section === "about") render(true);
  });
  checkUpdates = () => void up.check();
  btn.addEventListener("click", () => void up.install());
}
let aboutUpdate = "Checks every 10 minutes";
let updChecking = false;
let checkUpdates: () => void = () => undefined;

async function boot() {
  fillIcons(document);
  S = await api.getSettings();
  st = await api.getStatus();
  version = await appVersion();
  autostart = await api.autostartGet().catch(err => {
    autostartNote = `Couldn't read the Windows setting: ${err}`;
    return false;
  });
  applyTheme();
  setDot();
  renderNav();
  render(true);
  void startUpdates();
  await onStatus(s => {
    st = s;
    setDot();
    if (section === "status") render(true);
  });
  watchServer();
  await onGh(o => {
    ghCode = null;
    ghNote = o.state === "done" ? "" : o.message;
    // A sign-in shows the GitHub page on the board.
    if (o.state === "done" && !S.pages.enabled.includes("github")) void save({ pages: { enabled: [...S.pages.enabled, "github"] } });
    void api.githubStatus().then(s => { gh = s; }).finally(() => { if (section === "calendar") render(true); });
  });
  await onMs(o => {
    msUrl = "";
    msNote = o.message;
    if (section === "calendar") render(true);
  });
  await onPlex(o => {
    plexUrl = "";
    plexNote = o.state === "done" ? "" : o.message;
    plexServers = null;
    if (section === "media") render(true);
  });
  await onSettings(s => {
    // A grid or preset list changed elsewhere: drop selections that may now point at
    // a different block or preset.
    if (s.home.grid[selBlock]?.widget !== S.home.grid[selBlock]?.widget) selBlock = -1;
    if (presetSel.startsWith("u") &&
        s.home.presets[Number(presetSel.slice(1))]?.name !== S.home.presets[Number(presetSel.slice(1))]?.name) presetSel = "";
    S = s;
    applyTheme();
    render();
  });
  window.setInterval(refreshShot, 1500);
  document.addEventListener("visibilitychange", refreshShot);
}

void boot();
