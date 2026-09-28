import { describe, expect, it, vi } from "vitest";
import type { DesktopStatusEnvelope } from "../../../capacity-preview/src/status";
import { createCapacityStatusStore } from "./capacityStatusStore";

function envelope(sequence: number, context = "account-a"): DesktopStatusEnvelope {
  return {
    schemaVersion: "1.0", sequence, historyContextId: context, lifecycle: "ready",
    candidates: [], selectedExecutableId: "reader", issue: null, persistenceEnabled: true,
    status: {
      schemaVersion: "1.0", capturedAt: "2026-09-26T00:00:00Z", codexVersion: null,
      account: { authMode: "chatgpt", planType: "pro", bindingStatus: "stable" },
      dataStatus: { availability: "complete", freshness: "live", compatibility: "tested", reasonCodes: [] },
      quotaWindows: [{ limitId: "codex:primary", label: null, windowMinutes: 10080,
        remainingPercent: 28, usedPercent: 72, resetsAt: "2026-10-01T00:00:00Z" }],
      resetCredits: { summaryStatus: "available", availableCount: 2, detailsStatus: "complete" },
      usage: { availability: "unsupported", hasSummary: false, reasonCodes: [] }, diagnosticCodes: [],
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function setup() {
  let emit!: (value: DesktopStatusEnvelope) => void;
  const unlisten = vi.fn();
  const api = {
    read: vi.fn(async () => envelope(1)),
    cached: vi.fn(async (): Promise<DesktopStatusEnvelope | null> => envelope(0)),
    refresh: vi.fn(async () => envelope(2)),
    listen: vi.fn(async (listener: typeof emit): Promise<() => void> => { emit = listener; return unlisten; }),
  };
  return { store: createCapacityStatusStore(api), api, emit: (value: DesktopStatusEnvelope) => emit(value), unlisten };
}

describe("shared capacity status", () => {
  it("retains the complete snapshot between pages without detaching account invalidations", async () => {
    const { store, api, emit, unlisten } = setup();
    const firstPage = vi.fn();
    const detach = store.subscribe(firstPage);
    await store.read();
    detach();
    emit(envelope(3));
    expect(unlisten).not.toHaveBeenCalled();
    expect(firstPage).toHaveBeenCalledTimes(1);
    expect(store.getSnapshot()?.status?.resetCredits.availableCount).toBe(2);
    const secondPage = vi.fn();
    store.subscribe(secondPage);
    emit(envelope(4));
    expect(secondPage).toHaveBeenLastCalledWith(envelope(4));
    expect(api.listen).toHaveBeenCalledTimes(1);
  });

  it("never lets a delayed disk snapshot or read roll back a newer refresh event", async () => {
    const { store, api, emit } = setup();
    const delayed = deferred<DesktopStatusEnvelope>();
    api.read.mockReturnValueOnce(delayed.promise);
    const request = store.read();
    emit(envelope(8));
    delayed.resolve(envelope(3));
    expect(await request).toEqual(envelope(8));
    expect(await store.cached()).toEqual(envelope(8));
    expect(store.getSnapshot()?.sequence).toBe(8);
  });

  it("invalidates old account data even with no mounted page and no replacement quota yet", async () => {
    const { store, emit } = setup();
    await store.read();
    const changed = { ...envelope(5, "account-b"), status: null, lifecycle: "idle" as const, persistenceEnabled: false };
    emit(changed);
    expect(store.getSnapshot()).toBe(changed);
    expect((await store.cached())?.status).toBeNull();
  });

  it("coalesces simultaneous reads and refreshes while keeping explicit refresh independent", async () => {
    const { store, api } = setup();
    const delayedRead = deferred<DesktopStatusEnvelope>();
    const delayedRefresh = deferred<DesktopStatusEnvelope>();
    api.read.mockReturnValueOnce(delayedRead.promise);
    api.refresh.mockReturnValueOnce(delayedRefresh.promise);
    const read = store.read();
    expect(store.read()).toBe(read);
    const refresh = store.refresh();
    expect(store.refresh()).toBe(refresh);
    await Promise.resolve();
    delayedRefresh.resolve(envelope(4));
    expect((await refresh).sequence).toBe(4);
    delayedRead.resolve(envelope(1));
    expect((await read).sequence).toBe(4);
    expect(api.read).toHaveBeenCalledTimes(1);
    expect(api.refresh).toHaveBeenCalledTimes(1);
  });

  it("preserves the last observation on transport failure, surfaces the error and permits retry", async () => {
    const { store, api } = setup();
    await store.read();
    api.refresh.mockRejectedValueOnce(new Error("offline"));
    await expect(store.refresh()).rejects.toThrow("offline");
    expect(store.getSnapshot()?.sequence).toBe(1);
    expect((await store.refresh()).sequence).toBe(2);
    expect(api.refresh).toHaveBeenCalledTimes(2);
  });

  it("registers native events before reading and retries failed event registration", async () => {
    const { store, api } = setup();
    const registration = deferred<() => void>();
    api.listen.mockReturnValueOnce(registration.promise);
    const read = store.read();
    await Promise.resolve();
    expect(api.read).not.toHaveBeenCalled();
    registration.reject(new Error("event bridge unavailable"));
    expect((await read).sequence).toBe(1);
    await store.read();
    expect(api.listen).toHaveBeenCalledTimes(2);
  });

  it("replaces a stale snapshot with a newly observed same-account status", async () => {
    const { store, emit } = setup();
    const stale = { ...envelope(1), lifecycle: "stale" as const };
    store.accept(stale);
    store.subscribe(() => undefined);
    const fresh = envelope(2);
    fresh.status!.quotaWindows[0].remainingPercent = 27;
    emit(fresh);
    expect(store.getSnapshot()).toBe(fresh);
    expect(store.getSnapshot()?.status?.quotaWindows[0].remainingPercent).toBe(27);
  });
});
