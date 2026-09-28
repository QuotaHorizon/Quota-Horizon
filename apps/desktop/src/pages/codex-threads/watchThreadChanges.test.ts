import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { watchThreadChanges } from "./watchThreadChanges";

describe("session change detection", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("checks metadata periodically but reloads the list only when it changed", async () => {
    let revision = "first";
    const readRevision = vi.fn(async () => revision);
    const refresh = vi.fn(async () => true);
    const watcher = watchThreadChanges({ readRevision, refresh, canRead: () => true, onError: vi.fn() });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(readRevision).toHaveBeenCalledTimes(2);
    expect(refresh).toHaveBeenCalledTimes(1);
    revision = "second";
    await vi.advanceTimersByTimeAsync(5_000);
    expect(refresh).toHaveBeenCalledTimes(2);
    watcher.dispose();
  });

  it("does no background work while hidden or a search is pending, then refreshes on focus", async () => {
    let visible = false;
    const readRevision = vi.fn(async () => "same");
    const refresh = vi.fn(async () => true);
    const watcher = watchThreadChanges({ readRevision, refresh, canRead: () => visible, onError: vi.fn() });
    await vi.advanceTimersByTimeAsync(15_000);
    expect(readRevision).not.toHaveBeenCalled();
    visible = true;
    await watcher.check(true);
    await watcher.check(true);
    expect(refresh).toHaveBeenCalledTimes(2);
    watcher.dispose();
  });

  it("never overlaps a slow read or refresh", async () => {
    let finish!: () => void;
    const refresh = vi.fn(() => new Promise<boolean>((resolve) => { finish = () => resolve(true); }));
    const watcher = watchThreadChanges({ readRevision: async () => "same", refresh, canRead: () => true, onError: vi.fn() });
    await vi.advanceTimersByTimeAsync(20_000);
    expect(refresh).toHaveBeenCalledTimes(1);
    finish();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(refresh).toHaveBeenCalledTimes(1);
    watcher.dispose();
  });

  it("retries failed list reads without accepting a failed revision", async () => {
    const refresh = vi.fn().mockResolvedValueOnce(false).mockResolvedValue(true);
    const watcher = watchThreadChanges({ readRevision: async () => "same", refresh, canRead: () => true, onError: vi.fn() });
    await vi.advanceTimersByTimeAsync(15_000);
    expect(refresh).toHaveBeenCalledTimes(2);
    watcher.dispose();
  });

  it("does not update a closed page after a delayed native response", async () => {
    let finish!: (value: string) => void;
    const readRevision = () => new Promise<string>((resolve) => { finish = resolve; });
    const refresh = vi.fn(async () => true);
    const watcher = watchThreadChanges({ readRevision, refresh, canRead: () => true, onError: vi.fn() });
    const pending = watcher.check();
    watcher.dispose();
    finish("later");
    await pending;
    expect(refresh).not.toHaveBeenCalled();
  });

  it("recovers after a watcher failure even when the next revision is unchanged", async () => {
    const readRevision = vi.fn().mockResolvedValueOnce("same").mockRejectedValueOnce(new Error("offline")).mockResolvedValue("same");
    const refresh = vi.fn(async () => true);
    const onError = vi.fn();
    const watcher = watchThreadChanges({ readRevision, refresh, canRead: () => true, onError });
    await vi.advanceTimersByTimeAsync(15_000);
    expect(onError).toHaveBeenCalledTimes(1);
    expect(refresh).toHaveBeenCalledTimes(2);
    watcher.dispose();
  });
});
