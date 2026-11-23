// Settings window: boot, sidebar sections, rendering, live preview, events.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles.css";
import "./settings.css";
import { fillIcons, icon } from "./icons";
import {
  api, win, onStatus, onSettings, appVersion, ALL_PAGES, PAGE_LABELS, WIDGETS, WIDGET_LABELS, HOME_SIZES, GRID_COLS, GRID_ROWS,
  type Page, type Settings, type SettingsPatch, type Status, type Widget,
} from "./api";
import { card, row, tgl, text, slider, select, esc } from "./ui";

type SectionId = "status" | "display" | "pages" | "appearance" | "connection" | "about";
type SliderKey = "brightness" | "nightBrightness";
type Cell = Settings["home"]["grid"][number];

const SECTIONS: [SectionId, string, string][] = [
  ["status", "gauge", "Status"],
  ["display", "sun", "Display"],
  ["pages", "layout-grid", "Pages"],
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
  return brightness + night + pc;
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
  const home = card("Home screen", "5 columns by 3 rows. Blocks can't overlap.", [
    row("Show header", tgl(`data-k="homeHeader"`, S.home.header)),
    `<div class="grid-wrap">${gridHtml()}</div>`,
    row("Add a block", select(`data-k="homeAdd"`, addOpts, "", free.length === 0), esc(homeNote)),
  ].join(""));
  return card("Pages", "Enabled pages keep this order in the navigation bar", rows) + home;
}

function gridHtml(): string {
  const occupied = new Set<string>();
  const blocks = S.home.grid.map((c, i) => {
    for (let y = c.y; y < c.y + c.h; y++) for (let x = c.x; x < c.x + c.w; x++) occupied.add(`${x},${y}`);
    const label = WIDGET_LABELS[c.widget as Widget] ?? c.widget;
    return `<div class="hblock" style="grid-column:${c.x + 1} / span ${c.w};grid-row:${c.y + 1} / span ${c.h}">` +
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
  void save({ home: { grid: S.home.grid.filter((_, j) => j !== i) } });
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
    console.error("import_config failed", err);
  }
  applyTheme();
  render(true);
}

async function act(el: HTMLElement) {
  switch (el.dataset.act) {
    case "page-up": movePage(el.dataset.page as Page, -1); return;
    case "page-down": movePage(el.dataset.page as Page, 1); return;
    case "grid-remove": removeCell(Number(el.dataset.idx)); return;
    case "step": stepSlider(el.dataset.slider as SliderKey, Number(el.dataset.d)); return;
    case "eye": showToken = !showToken; render(true); return;
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
