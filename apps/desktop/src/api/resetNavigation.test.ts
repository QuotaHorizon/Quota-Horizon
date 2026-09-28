import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mock = vi.hoisted(() => ({ invoke: vi.fn(async () => undefined), emit: vi.fn(async () => undefined), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mock.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ emit: mock.emit, listen: mock.listen }));

beforeEach(() => {
  vi.resetModules(); vi.clearAllMocks();
  vi.stubGlobal("window", { __TAURI_INTERNALS__: {}, localStorage: { getItem: () => null }, location: { protocol: "tauri:" } });
  vi.stubGlobal("document", { querySelector: () => null });
});
afterEach(() => vi.unstubAllGlobals());

it("opens the dashboard before requesting the reset page without invoking an account operation", async () => {
  const api = await import("./backend");
  await api.showResetIntelligenceFromPopover();
  expect(mock.invoke).toHaveBeenCalledExactlyOnceWith("show_dashboard_from_bubble", {});
  expect(mock.emit).toHaveBeenCalledExactlyOnceWith("open-reset-intelligence");
  expect(mock.invoke.mock.invocationCallOrder[0]).toBeLessThan(mock.emit.mock.invocationCallOrder[0]);
});

it("opens the exact work-plan page without switching or refreshing an account", async () => {
  const api = await import("./backend");
  await api.showCapacityPlanFromPopover();
  expect(mock.invoke).toHaveBeenCalledExactlyOnceWith("show_dashboard_from_bubble", {});
  expect(mock.emit).toHaveBeenCalledExactlyOnceWith("open-capacity-plan");
});

it("keeps plan IPC local and sends a context/revision, not an account fingerprint", async () => {
  const api = await import("./backend");
  await api.getCapacityDemandPlan("synthetic-context");
  expect(mock.invoke).toHaveBeenCalledExactlyOnceWith("capacity_get_demand_plan", { historyContextId: "synthetic-context" });
  vi.clearAllMocks();
  const request = { historyContextId: "synthetic-context", expectedRevision: 2, enabled: false, demand: { horizonEnd: "2026-09-28T04:00:00Z", demandKind: "maintain_recent_pace" as const, plannedCodexActiveHours: null } };
  await api.updateCapacityDemandPlan(request);
  expect(mock.invoke).toHaveBeenCalledExactlyOnceWith("capacity_update_demand_plan", { request });
});

it("does not expose plan reads or edits to a browser-only host", async () => {
  vi.stubGlobal("window", { localStorage: { getItem: () => null }, location: { protocol: "https:" } });
  const api = await import("./backend");
  await expect(api.getCapacityDemandPlan("synthetic-context")).rejects.toThrow("native application");
  expect(mock.invoke).not.toHaveBeenCalled();
});

it("does not dispatch navigation when opening the dashboard fails", async () => {
  mock.invoke.mockRejectedValueOnce(new Error("window unavailable"));
  const api = await import("./backend");
  await expect(api.showResetIntelligenceFromPopover()).rejects.toThrow();
  expect(mock.emit).not.toHaveBeenCalled();
});

it("disposes a late subscription and cannot navigate after unmount", async () => {
  let resolve!: (unlisten: () => void) => void;
  mock.listen.mockReturnValue(new Promise((done) => { resolve = done; }));
  const api = await import("./backend");
  const onOpen = vi.fn(); const unlisten = vi.fn();
  const dispose = api.subscribeToOpenResetIntelligence(onOpen);
  const callback = mock.listen.mock.calls[0][1] as () => void;
  callback(); expect(onOpen).toHaveBeenCalledTimes(1);
  dispose(); callback(); resolve(unlisten);
  await Promise.resolve();
  expect(onOpen).toHaveBeenCalledTimes(1); expect(unlisten).toHaveBeenCalledTimes(1);
});
