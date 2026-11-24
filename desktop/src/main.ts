// Settings window: boot, sidebar sections, rendering, live preview, events.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles.css";
import "./settings.css";
import { fillIcons, icon } from "./icons";
import {
  api, win, TAURI, onStatus, onSettings, appVersion, ALL_PAGES, PAGE_LABELS, WIDGETS, WIDGET_LABELS, HOME_SIZES, GRID_COLS, GRID_ROWS,
  type Page, type Settings, type SettingsPatch, type Status, type Widget, type ClaudeState, type HomePreset, BUILTIN_PRESETS,
  type Tile, type TileAction, type CustomGame, type FirmwareCheck, type Location, MAX_TILES, TILE_ICONS, TILE_ACTIONS, GAME_PRESETS, EXAMPLE_TILES,
} from "./api";
import { card, row, tgl, text, slider, select, esc } from "./ui";
import { mountSim, onSimState, simState } from "./sim";

type SectionId = "status" | "display" | "pages" | "shortcuts" | "games" | "ai" | "appearance" | "connection" | "about";
type SliderKey = "brightness" | "nightBrightness";
type Cell = Settings["home"]["grid"][number];

const SECTIONS: [SectionId, string, string][] = [
  ["status", "gauge", "Status"],
  ["display", "sun", "Display"],
  ["pages", "layout-grid", "Pages"],
  ["shortcuts", "zap", "Shortcuts"],
  ["games", "gamepad", "Games"],
  ["ai", "sparkles", "AI agents"],
  ["appearance", "palette", "Appearance"],
  ["connection", "wifi", "Connection"],
  ["about", "info", "About"],
];
const THEMES: [string, string][] = [
  ["midnight", "Midnight (default)"], ["purple", "Purple"], ["blue", "Blue"], ["graphite", "Graphite"], ["teal", "Teal"], ["rose", "Rose"],
];
const BOARD_THEMES: [string, string][] = [
  ["same", "Same as app"], ["classic", "Classic amber"], ...THEMES.map(([k, v]): [string, string] => [k, v.replace(" (default)", "")]),
];
const STATE_TEXT: Record<Status["state"], string> = { connected: "Connected", searching: "Searching", refused: "Refused", stopped: "Stopped" };
const DOT: Record<Status["state"], string> = { connected: "good", searching: "warn", refused: "bad", stopped: "" };
const SLIDER_SPEC: Record<SliderKey, { label: string; min: number; max: number }> = {
  brightness: { label: "Brightness", min: 1, max: 100 },
  nightBrightness: { label: "Night brightness", min: 0, max: 100 },
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

const sliderValue = (k: SliderKey) => (k === "brightness" ? S.display.brightness : S.display.night_brightness);
const setSliderLocal = (k: SliderKey, v: number) => {
  if (k === "brightness") S.display.brightness = v;
  else S.display.night_brightness = v;
};
const sliderPatch = (k: SliderKey, v: number): SettingsPatch =>
  k === "brightness" ? { display: { brightness: v } } : { display: { night_brightness: v } };

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
    case "ai": return aiHtml();
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
  const clock = card("Clock", "", row("Show seconds", tgl(`data-k="clockSeconds"`, d.clock_seconds), "Home clock on the board"));
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
  const alerts = card("Alerts", "Shows on the board when Claude Code waits for you", [
    row("Waiting alerts", tgl(`data-k="aiAlerts"`, a.alerts)),
    row("Idle alert after", `<input class="num" type="number" min="0" max="3600" data-k="aiIdle" value="${a.idle_alert_seconds}"> s`,
      "An idle session alerts after this many seconds"),
  ].join(""));
  return usage + cc + alerts;
}

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
    row("Config file", `<button type="button" data-act="import">${icon("refresh-cw", 13)}Import from config.toml</button>`,
      S.imported ? "Already imported" : "Copies host and token from the Python companion's config"),
  ].join("")) + firmwareHtml();
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
    text("Calendar, printers, internet and GitHub still need the Python companion. Don't run both at once."),
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
    case "preview":
      preview = el.dataset.v === "sim" ? "sim" : "live";
      render(true);
      return;
    case "tile-select": selTile = Number(el.dataset.idx); render(true); return;
    case "tile-add": {
      const tiles = [...S.tiles, { label: "New tile", icon: "apps", color: "#2b2f3a", action: "keys" as TileAction, target: "", state: null }];
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
    case "import": await importConfig(); return;
    case "update-check": checkUpdates(); return;
    case "firmware": await updateFirmware(); return;
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
    case "aiAgent":
      void save({ ai: { agent: t.value as Settings["ai"]["agent"] } });
      return;
    case "aiLive":
      void save({ ai: { live_limits: on } });
      return;
    case "aiAlerts":
      void save({ ai: { alerts: on } });
      return;
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
