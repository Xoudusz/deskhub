// Settings window: boot, sidebar sections, rendering, live preview, events.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles.css";
import "./settings.css";
import { fillIcons, icon } from "./icons";
import {
  api, win, onStatus, onSettings, appVersion, ALL_PAGES, PAGE_LABELS, WIDGETS, WIDGET_LABELS, HOME_SIZES, GRID_COLS, GRID_ROWS,
  type Page, type Settings, type SettingsPatch, type Status, type Widget, type ClaudeState, type HomePreset, BUILTIN_PRESETS,
} from "./api";
import { card, row, tgl, text, slider, select, esc } from "./ui";

type SectionId = "status" | "display" | "pages" | "ai" | "appearance" | "connection" | "about";
type SliderKey = "brightness" | "nightBrightness";
type Cell = Settings["home"]["grid"][number];

const SECTIONS: [SectionId, string, string][] = [
  ["status", "gauge", "Status"],
  ["display", "sun", "Display"],
  ["pages", "layout-grid", "Pages"],
  ["ai", "sparkles", "AI agents"],
  ["appearance", "palette", "Appearance"],
  ["connection", "wifi", "Connection"],
  ["about", "info", "About"],
];
const THEMES: [string, string][] = [
  ["purple", "Purple"], ["blue", "Blue"], ["graphite", "Graphite"], ["midnight", "Midnight"], ["teal", "Teal"], ["rose", "Rose"],
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
  main.scrollTop = top;
  if (section === "status") refreshShot();
}

function renderNav() {
  nav.innerHTML =
    SECTIONS.map(([id, ic, label]) => `<button type="button" data-sec="${id}" class="${id === section ? "on" : ""}">${icon(ic, 15)}${label}</button>`).join("") +
    `<span class="ver">Desk Hub ${esc(version)}</span>`;
}

async function save(patch: SettingsPatch) {
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
function refreshShot() {
  const img = document.getElementById("shot") as HTMLImageElement | null;
  if (!img || shotBusy || section !== "status" || st.state !== "connected" || document.visibilityState !== "visible") return;
  shotBusy = true;
  api.boardScreenshot()
    .then(url => { img.src = url; })
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
  const body = on
    ? `<div class="prev" id="prev"><img id="shot" alt="Board screen" draggable="false"></div>`
    : `<div class="prev offline">Board offline</div>`;
  return board + card("Live preview", "Tap the picture to press the board", `<div class="swr swsl">${body}</div>`);
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
    row("Night style", select(`data-k="nightStyle"`, [["off", "Screen off"], ["clock", "Clock"]], d.night_style)),
    slider("nightBrightness", "Night brightness", 0, 100, d.night_brightness, "0 = screen off or clock"),
  ].join(""));
  const pc = card("PC and navigation", "", [
    row("When the PC sleeps", select(`data-k="pcSleep"`, [["off", "Screen off"], ["clock", "Clock"], ["none", "Do nothing"]], d.pc_sleep)),
    row("Navigation bar", select(`data-k="nav"`, [["left", "Left"], ["right", "Right"], ["bottom", "Bottom"]], S.pages.nav),
      "The board restarts when this changes"),
  ].join(""));
  return brightness + clock + night + pc;
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
  const home = card("Home screen", "5 columns by 3 rows. Click a block to resize it.", [
    row("Layout",
      select(`data-k="homePreset"`, presetOpts, presetSel) +
      `<button type="button" data-act="preset-apply"${presetSel ? "" : " disabled"}>Apply</button>` +
      (own ? `<button type="button" class="ib" data-act="preset-delete" title="Delete this preset">${icon("x", 13)}</button>` : ""),
      "Presets replace the blocks and the header setting"),
    row("Show header", tgl(`data-k="homeHeader"`, S.home.header)),
    `<div class="grid-wrap">${gridHtml()}</div>`,
    sizeRow(),
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
    return `<div class="hblock${i === selBlock ? " sel" : ""}" data-act="grid-select" data-idx="${i}" style="grid-column:${c.x + 1} / span ${c.w};grid-row:${c.y + 1} / span ${c.h}">` +
      `<span>${esc(label)}<small>${c.w} x ${c.h}</small></span>` +
      `<button type="button" class="ib" data-act="grid-remove" data-idx="${i}" title="Remove">${icon("x", 11)}</button></div>`;
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

/** Whether block `i` can take size w x h at its spot: inside the grid, no overlap. */
function sizeFits(i: number, w: number, h: number): boolean {
  const c = S.home.grid[i];
  if (c.x + w > GRID_COLS || c.y + h > GRID_ROWS) return false;
  return S.home.grid.every((o, j) => j === i || !(o.x < c.x + w && c.x < o.x + o.w && o.y < c.y + h && c.y < o.y + o.h));
}

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
    row("Live limits", tgl(`data-k="aiLive"`, a.live_limits), "Reads the 5-hour and weekly limits from Anthropic. Coming in a later update"),
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
  return card("Theme", "Applies at once", row("Colour theme", select(`data-k="theme"`, THEMES, S.appearance.theme)));
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
  ].join(""));
}

function aboutHtml(): string {
  return card("Desk Hub", "Board companion for Windows", [
    row("Version", `<span>${esc(version)}</span>`),
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
    case "step": stepSlider(el.dataset.slider as SliderKey, Number(el.dataset.d)); return;
    case "eye": showToken = !showToken; render(true); return;
    case "claude": await toggleClaude(); return;
    case "import": await importConfig(); return;
  }
}

main.addEventListener("click", e => {
  const t = e.target as HTMLElement;
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
    case "theme":
      S.appearance.theme = t.value as Settings["appearance"]["theme"];
      applyTheme();
      void save({ appearance: { theme: S.appearance.theme } });
      return;
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

async function boot() {
  fillIcons(document);
  S = await api.getSettings();
  st = await api.getStatus();
  version = await appVersion();
  applyTheme();
  setDot();
  renderNav();
  render(true);
  await onStatus(s => {
    st = s;
    setDot();
    if (section === "status") render(true);
  });
  await onSettings(s => {
    S = s;
    applyTheme();
    render();
  });
  window.setInterval(refreshShot, 1500);
  document.addEventListener("visibilitychange", refreshShot);
}

void boot();
