import { applyThemeColor, DEFAULT_THEME_COLOR } from "./theme";
import { isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";

const THEME_MODE_KEY = "codex-switch:theme-mode";
const THEME_MODE_EVENT = "quota-horizon-theme-mode-changed";

export type ThemeMode = "light" | "dark";

export function loadThemeMode(): ThemeMode {
  return window.localStorage.getItem(THEME_MODE_KEY) === "dark" ? "dark" : "light";
}

export function applyThemeMode(mode: ThemeMode) {
  const root = document.documentElement;
  root.dataset.theme = mode;
  root.style.colorScheme = mode;
  applyThemeColor(root.style.getPropertyValue("--green") || DEFAULT_THEME_COLOR);
}

export function persistThemeMode(mode: ThemeMode) {
  window.localStorage.setItem(THEME_MODE_KEY, mode);
  window.dispatchEvent(new Event(THEME_MODE_EVENT));
  if (isTauri()) void emit(THEME_MODE_EVENT).catch(() => undefined);
}

export function isThemeModeStorageEvent(event: StorageEvent) {
  return event.key === THEME_MODE_KEY || event.key === null;
}

/** All webviews, including the persistent menu-bar popover, share this setting.
 * Read storage on notification so delayed events cannot restore an older mode. */
export function subscribeToThemeMode(onChange: (mode: ThemeMode) => void) {
  let disposed = false;
  const sync = () => { if (!disposed) onChange(loadThemeMode()); };
  const storage = (event: StorageEvent) => { if (isThemeModeStorageEvent(event)) sync(); };
  const visible = () => { if (document.visibilityState === "visible") sync(); };
  window.addEventListener(THEME_MODE_EVENT, sync);
  window.addEventListener("storage", storage);
  window.addEventListener("focus", sync);
  document.addEventListener("visibilitychange", visible);
  const unlisteners: (() => void)[] = [];
  if (isTauri()) {
    for (const event of [THEME_MODE_EVENT, "capacity-popover-opened"]) {
      void listen(event, sync).then((unlisten) => {
        if (disposed) unlisten();
        else unlisteners.push(unlisten);
      }).catch(() => undefined);
    }
  }
  sync();
  return () => {
    disposed = true;
    window.removeEventListener(THEME_MODE_EVENT, sync);
    window.removeEventListener("storage", storage);
    window.removeEventListener("focus", sync);
    document.removeEventListener("visibilitychange", visible);
    unlisteners.splice(0).forEach((unlisten) => unlisten());
  };
}
