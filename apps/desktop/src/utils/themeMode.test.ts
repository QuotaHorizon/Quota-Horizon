import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { loadThemeMode, persistThemeMode, subscribeToThemeMode } from "./themeMode";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: vi.fn(() => false) }));
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn(async () => {}), listen: vi.fn() }));

let storage: Map<string, string>;
let windowTarget: EventTarget;
let documentTarget: EventTarget;
const key = "codex-switch:theme-mode";
beforeEach(() => {
  vi.clearAllMocks(); vi.mocked(isTauri).mockReturnValue(false);
  storage = new Map(); windowTarget = new EventTarget(); documentTarget = new EventTarget();
  vi.stubGlobal("window", Object.assign(windowTarget, { localStorage: {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
  } }));
  vi.stubGlobal("document", Object.assign(documentTarget, { visibilityState: "visible" }));
});
afterEach(() => vi.unstubAllGlobals());

describe("theme synchronization across app windows", () => {
  it("updates existing subscribers for both light/dark changes and restores the saved mode on startup", () => {
    storage.set(key, "dark");
    const change = vi.fn(); const stop = subscribeToThemeMode(change);
    expect(change).toHaveBeenLastCalledWith("dark");
    persistThemeMode("light"); expect(change).toHaveBeenLastCalledWith("light");
    persistThemeMode("dark"); expect(change).toHaveBeenLastCalledWith("dark");
    expect(loadThemeMode()).toBe("dark");
    stop(); change.mockClear(); persistThemeMode("light"); expect(change).not.toHaveBeenCalled();
  });
  it("catches up a hidden window on focus and handles changes from another webview", () => {
    const change = vi.fn(); const stop = subscribeToThemeMode(change);
    storage.set(key, "dark"); windowTarget.dispatchEvent(new Event("focus"));
    expect(change).toHaveBeenLastCalledWith("dark");
    storage.set(key, "light");
    windowTarget.dispatchEvent(Object.assign(new Event("storage"), { key }));
    expect(change).toHaveBeenLastCalledWith("light");
    storage.set(key, "dark"); documentTarget.dispatchEvent(new Event("visibilitychange"));
    expect(change).toHaveBeenLastCalledWith("dark");
    change.mockClear(); windowTarget.dispatchEvent(Object.assign(new Event("storage"), { key: "unrelated" }));
    expect(change).not.toHaveBeenCalled();
    storage.clear(); windowTarget.dispatchEvent(Object.assign(new Event("storage"), { key: null }));
    expect(change).toHaveBeenLastCalledWith("light"); stop();
  });
  it("resyncs native popovers without relying on WebKit storage events and releases late listeners", async () => {
    vi.mocked(isTauri).mockReturnValue(true);
    const callbacks = new Map<string, () => void>();
    const complete: ((unlisten: () => void) => void)[] = [];
    vi.mocked(listen).mockImplementation((name, callback) => {
      callbacks.set(String(name), () => callback({} as never));
      return new Promise((resolve) => complete.push(resolve));
    });
    const change = vi.fn(); const stop = subscribeToThemeMode(change);
    persistThemeMode("dark"); expect(emit).toHaveBeenCalledWith("quota-horizon-theme-mode-changed");
    storage.set(key, "light"); callbacks.get("capacity-popover-opened")!();
    expect(change).toHaveBeenLastCalledWith("light");
    storage.set(key, "dark"); callbacks.get("quota-horizon-theme-mode-changed")!();
    expect(change).toHaveBeenLastCalledWith("dark");
    stop(); change.mockClear(); callbacks.get("capacity-popover-opened")!();
    expect(change).not.toHaveBeenCalled();
    const unlisten = vi.fn(); complete.forEach((resolve) => resolve(unlisten));
    await Promise.resolve(); expect(unlisten).toHaveBeenCalledTimes(2);
  });
});
