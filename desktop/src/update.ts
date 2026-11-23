// App updates from GitHub Releases (plugins.updater in src-tauri/tauri.conf.json, like
// aion2-craft): checks every 10 minutes, downloads a new version in the background, and the
// title bar shows "Update" once it is ready; a click installs it (passive NSIS) and restarts.
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

const EVERY = 10 * 60_000;

export interface UpdateState {
  available: Update | null;
  /** Downloaded and signature-checked: a click only installs. */
  ready: boolean;
  installing: boolean;
  error: string | null;
}

export function startUpdater(onChange: (s: UpdateState) => void) {
  const st: UpdateState = { available: null, ready: false, installing: false, error: null };
  let busy = false;

  async function run() {
    if (busy || st.installing || st.ready) return;
    busy = true;
    try {
      const found = await check();
      st.error = null;
      if (found && (!st.available || found.version !== st.available.version)) {
        st.available?.close().catch(() => undefined);
        st.available = found;
        await found.download();
        st.ready = true;
      } else if (found) {
        found.close().catch(() => undefined);
      }
    } catch (e) {
      st.error = String(e);
    }
    busy = false;
    onChange(st);
  }

  async function install() {
    const u = st.available;
    if (!u || !st.ready || st.installing) return;
    st.installing = true;
    onChange(st);
    try {
      await u.install();
      await relaunch();
    } catch (e) {
      st.error = String(e);
      st.installing = false;
      st.ready = false;
      onChange(st);
    }
  }

  void run();
  window.setInterval(() => void run(), EVERY);
  return { install, check: run };
}
