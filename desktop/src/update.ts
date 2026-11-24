// App updates: the Rust side (src-tauri/src/updates.rs) checks GitHub Releases every 10 minutes and
// downloads new versions, also while the window is hidden in the tray. This is only the UI's view
// of it: status, "update" events, "Check now" and install.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface UpdateState {
  current: string;
  /** The newer version found; downloaded once `ready`. */
  version: string | null;
  ready: boolean;
  checking: boolean;
  installing: boolean;
  error: string | null;
  /** Unix seconds of the last finished check. */
  checked: number | null;
}

export async function startUpdater(onChange: (s: UpdateState) => void) {
  await listen<UpdateState>("update", e => onChange(e.payload));
  onChange(await invoke<UpdateState>("update_status"));
  return {
    install: () => invoke("update_install").catch(() => undefined),
    check: () => invoke<UpdateState>("update_check").then(onChange),
  };
}
