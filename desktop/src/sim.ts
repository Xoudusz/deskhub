// Board simulator: the board's own UI (firmware/sim, LVGL compiled to WASM) on a canvas,
// fed the same frames the app sends the board. One instance, shared by every view: the
// canvas moves into whichever placeholder (`[data-sim]`) is on screen.
import { api, SimFrame } from "./api";

const W = 800;
const H = 480;
const WEATHER_MS = 15 * 60 * 1000;

/** The emscripten module (deskhub_sim.js, built by firmware/sim/build.sh). */
interface SimModule {
  HEAPU8: Uint8Array;
  _malloc(n: number): number;
  _free(p: number): void;
  _sim_init(): void;
  _sim_step(): number;
  _sim_pixels(): number;
  _sim_take_dirty(): number;
  _sim_pointer(x: number, y: number, down: number): void;
  _sim_show_home(): void;
  _sim_json(p: number): void;
  _sim_binary(p: number, n: number): number;
  _sim_weather_url(): number;
  _sim_weather(p: number): number;
  UTF8ToString(p: number): string;
  stringToNewUTF8(s: string): number;
}
type Factory = (opts: object) => Promise<SimModule>;

export type SimState = "idle" | "loading" | "running" | "missing" | "failed";

let state: SimState = "idle";
let factory: Factory | null = null;
let mod: SimModule | null = null;
let canvas: HTMLCanvasElement | null = null;
let ctx: CanvasRenderingContext2D | null = null;
let image: ImageData | null = null;
let frameReq = 0;
let weatherTimer = 0;
let listening = false;
let restarting = false;
const onChange = new Set<(s: SimState) => void>();

export function simState(): SimState {
  return state;
}

/** Called when the simulator starts, fails or is missing (e.g. to re-render a note). */
export function onSimState(cb: (s: SimState) => void): void {
  onChange.add(cb);
}

function setState(s: SimState) {
  state = s;
  onChange.forEach(cb => cb(s));
}

const isRestart = (e: unknown) => String(e).includes("deskhub-sim-restart");

function feedOne(m: SimModule, f: SimFrame) {
  if ("json" in f) {
    const p = m.stringToNewUTF8(JSON.stringify(f.json));
    try { m._sim_json(p); } finally { m._free(p); }
  } else {
    const bytes = Uint8Array.from(atob(f.bin), c => c.charCodeAt(0));
    const p = m._malloc(bytes.length);
    try {
      m.HEAPU8.set(bytes, p);
      m._sim_binary(p, bytes.length);
    } finally {
      m._free(p);
    }
  }
}

/**
 * A fresh instance fed the app's current state. A theme, navigation side or location
 * that differs from the instance's saved one makes the UI "restart": start over (its NVS
 * lives in JS, so the next instance already has the new value).
 */
/** Loads deskhub_sim.js (a classic script: files in public/ can't be imported in dev). */
function loadFactory(): Promise<Factory> {
  const g = globalThis as unknown as { createDeskhubSim?: Factory };
  if (g.createDeskhubSim) return Promise.resolve(g.createDeskhubSim);
  return new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = "/sim/deskhub_sim.js";
    s.onload = () => (g.createDeskhubSim ? resolve(g.createDeskhubSim) : reject(new Error("simulator missing")));
    s.onerror = () => reject(new Error("simulator missing"));
    document.head.appendChild(s);
  });
}

async function boot(): Promise<SimModule> {
  factory ??= await loadFactory();
  for (let tries = 0; tries < 4; tries++) {
    const m = await factory({ print: () => undefined, printErr: (t: string) => console.warn("sim:", t) });
    try {
      m._sim_init();
      for (const f of await api.simState()) feedOne(m, f);
      return m;
    } catch (e) {
      if (!isRestart(e)) throw e;
    }
  }
  throw new Error("simulator keeps restarting");
}

/**
 * Frames that arrive while an instance boots: the snapshot may already be on its way, so they
 * are fed after it (feeding a frame twice is harmless: the board takes the latest state).
 */
let pending: SimFrame[] = [];

/** Feeds the queued frames to the new instance. */
function flushPending() {
  const queued = pending;
  pending = [];
  for (const f of queued) feed(f);
}

async function restart() {
  if (restarting) return;
  restarting = true;
  mod = null;   // the old instance threw mid-call: never step it again
  try {
    mod = await boot();
    if (wantHome) showHome();
    void refreshWeather();
  } catch (e) {
    console.error("simulator failed", e);
    mod = null;
    pending = [];
    setState("failed");
  } finally {
    restarting = false;
  }
  flushPending();
  if (mod && !frameReq) frameReq = requestAnimationFrame(draw);
}

function feed(f: SimFrame) {
  if (!mod || restarting) {
    if (state === "loading" || restarting) pending.push(f);
    return;
  }
  try {
    feedOne(mod, f);
  } catch (e) {
    if (isRestart(e)) void restart();
    else console.error("simulator frame failed", e);
  }
}

async function resync() {
  if (!mod || restarting) return;
  try {
    for (const f of await api.simState()) feedOne(mod, f);
  } catch (e) {
    if (isRestart(e)) void restart();
  }
}

/** Weather: the board fetches Open-Meteo itself; the simulator asks the app's webview to. */
async function refreshWeather() {
  window.clearTimeout(weatherTimer);
  weatherTimer = window.setTimeout(refreshWeather, WEATHER_MS);
  const m = mod;
  if (!m) return;
  try {
    const url = m.UTF8ToString(m._sim_weather_url());
    const body = await (await fetch(url)).text();
    if (m !== mod) return;
    const p = m.stringToNewUTF8(body);
    try { m._sim_weather(p); } finally { m._free(p); }
  } catch {
    // Offline or blocked: the weather widget keeps "Loading weather".
  }
}

function draw() {
  frameReq = 0;
  if (!mod || !canvas?.isConnected) return;   // off screen: stop until the next mount
  try {
    mod._sim_step();
    if (mod._sim_take_dirty() && ctx && image) {
      const at = mod._sim_pixels();
      image.data.set(mod.HEAPU8.subarray(at, at + W * H * 4));
      ctx.putImageData(image, 0, 0);
    }
  } catch (e) {
    if (isRestart(e)) void restart();
    else console.error("simulator step failed", e);
  }
  frameReq = requestAnimationFrame(draw);
}

/** LVGL reads the pointer every ~30 ms: a quicker click is held this long so it registers. */
const MIN_PRESS_MS = 80;
let pressedAt = 0;
let releaseTimer = 0;

function pointer(e: PointerEvent, down: boolean | null) {
  if (!mod || !canvas) return;
  const m = mod;
  const r = canvas.getBoundingClientRect();
  const x = Math.round(((e.clientX - r.left) / r.width) * W);
  const y = Math.round(((e.clientY - r.top) / r.height) * H);
  const pressed = down ?? (e.buttons & 1) === 1;
  if (down === null && !pressed && releaseTimer) return;   // a release is already on its way
  if (down === true) {
    window.clearTimeout(releaseTimer);
    pressedAt = performance.now();
  }
  if (down === false) {
    const left = MIN_PRESS_MS - (performance.now() - pressedAt);
    if (left > 0) {
      window.clearTimeout(releaseTimer);
      releaseTimer = window.setTimeout(() => {
        releaseTimer = 0;
        if (m === mod) m._sim_pointer(x, y, 0);
      }, left);
      return;
    }
  }
  m._sim_pointer(x, y, pressed ? 1 : 0);
}

function makeCanvas(): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = W;
  c.height = H;
  c.className = "simcanvas";
  c.addEventListener("pointerdown", e => { c.setPointerCapture(e.pointerId); pointer(e, true); });
  c.addEventListener("pointermove", e => pointer(e, null));
  c.addEventListener("pointerup", e => pointer(e, false));
  c.addEventListener("pointercancel", e => pointer(e, false));
  ctx = c.getContext("2d");
  image = ctx ? ctx.createImageData(W, H) : null;
  return c;
}

/** The next start or mount shows Home (the Home builder's preview). */
let wantHome = false;
/** The section the canvas was last mounted for: a re-render of the same one keeps the page. */
let lastPlace = "";

function showHome() {
  if (!mod) return;
  try {
    mod._sim_show_home();
  } catch (e) {
    if (isRestart(e)) void restart();
  }
}

/**
 * Moves the simulator into the first `[data-sim]` placeholder under `root`; starts it on first
 * use. `home`: switch to the Home page when it moves here (not on every re-render).
 */
export function mountSim(root: ParentNode, place: string, home = false): void {
  const host = root.querySelector<HTMLElement>("[data-sim]");
  if (!host) {
    lastPlace = "";
    return;
  }
  canvas ??= makeCanvas();
  if (place !== lastPlace) {
    lastPlace = place;
    wantHome = home;
    if (home) showHome();
  }
  host.appendChild(canvas);
  if (state === "idle") {
    state = "loading";   // no callbacks: this runs inside the caller's render
    void (async () => {
      try {
        if (!listening) {
          listening = true;
          // Listen first, so nothing published during boot is missed (it is queued).
          await api.onSimFrame(feed);
          await api.onSimResync(() => void resync());
        }
        mod = await boot();
        if (wantHome) showHome();
        setState("running");
        flushPending();
        void refreshWeather();
      } catch (e) {
        const missing = String(e).includes("simulator missing");
        console.error("simulator failed to start", e);
        pending = [];
        setState(missing ? "missing" : "failed");
      }
      if (!frameReq) frameReq = requestAnimationFrame(draw);
    })();
  } else if (!frameReq) {
    frameReq = requestAnimationFrame(draw);
  }
}
