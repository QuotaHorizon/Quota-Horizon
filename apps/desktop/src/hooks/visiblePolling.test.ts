import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createVisiblePoller } from "./visiblePolling";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const config = { active: true, key: "20-weeks", intervalMs: 60_000 };

describe("visible UI polling", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); });

  it("does not read or create a timer until the page is visible", async () => {
    const read = vi.fn(async () => 1);
    const poller = createVisiblePoller({ read, onValue: vi.fn(), onError: vi.fn() });
    poller.configure({ ...config, active: false });
    await poller.refresh();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(read).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
    poller.configure(config);
    await vi.advanceTimersByTimeAsync(0);
    expect(read).toHaveBeenCalledTimes(1);
    poller.dispose();
  });

  it("polls after completion, stops while hidden and refreshes on return", async () => {
    const read = vi.fn(async () => 1);
    const poller = createVisiblePoller({ read, onValue: vi.fn(), onError: vi.fn() });
    poller.configure(config);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(read).toHaveBeenCalledTimes(2);
    poller.configure({ ...config, active: false });
    await vi.advanceTimersByTimeAsync(120_000);
    expect(read).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
    poller.configure(config);
    expect(read).toHaveBeenCalledTimes(3);
    poller.dispose();
  });

  it("coalesces a burst of refreshes behind one slow read", async () => {
    const first = deferred<number>();
    const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(2);
    const onValue = vi.fn();
    const poller = createVisiblePoller({ read, onValue, onError: vi.fn() });
    poller.configure(config);
    for (let i = 0; i < 20; i++) void poller.refresh();
    await vi.advanceTimersByTimeAsync(180_000);
    expect(read).toHaveBeenCalledTimes(1);
    first.resolve(1);
    await vi.advanceTimersByTimeAsync(0);
    expect(read).toHaveBeenCalledTimes(2);
    expect(onValue.mock.calls).toEqual([[1], [2]]);
    expect(vi.getTimerCount()).toBe(1);
    poller.dispose();
  });

  it("rejects old-range results and reads only the newest pending range", async () => {
    const first = deferred<string>();
    let currentKey = "old";
    const read = vi.fn().mockImplementationOnce(() => first.promise).mockImplementation(async () => currentKey);
    const onValue = vi.fn();
    const poller = createVisiblePoller({ read, onValue, onError: vi.fn() });
    poller.configure({ ...config, key: currentKey });
    for (currentKey of ["middle", "latest"]) poller.configure({ ...config, key: currentKey });
    first.resolve("old");
    await vi.advanceTimersByTimeAsync(0);
    expect(read).toHaveBeenCalledTimes(2);
    expect(onValue.mock.calls).toEqual([["latest"]]);
    poller.dispose();
  });

  it("does not publish a pending result after hide, even across a quick reopen", async () => {
    const first = deferred<number>();
    const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(2);
    const onValue = vi.fn();
    const poller = createVisiblePoller({ read, onValue, onError: vi.fn() });
    poller.configure(config);
    poller.configure({ ...config, active: false });
    poller.configure(config);
    first.resolve(1);
    await vi.advanceTimersByTimeAsync(0);
    expect(read).toHaveBeenCalledTimes(2);
    expect(onValue.mock.calls).toEqual([[2]]);
    poller.dispose();
  });

  it("discards pending event refreshes when the page becomes hidden", async () => {
    const first = deferred<number>();
    const read = vi.fn(() => first.promise);
    const onValue = vi.fn();
    const poller = createVisiblePoller({ read, onValue, onError: vi.fn() });
    poller.configure(config);
    void poller.refresh();
    poller.configure({ ...config, active: false });
    first.resolve(1);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(read).toHaveBeenCalledTimes(1);
    expect(onValue).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
    poller.dispose();
  });

  it("keeps the last value on failure and automatically retries", async () => {
    const onError = vi.fn();
    const onValue = vi.fn();
    const read = vi.fn().mockResolvedValueOnce(7).mockRejectedValueOnce(new Error("offline")).mockResolvedValue(8);
    const poller = createVisiblePoller({ read, onValue, onError });
    poller.configure(config);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(onValue.mock.calls).toEqual([[7]]);
    expect(onError).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(onValue.mock.calls).toEqual([[7], [8]]);
    poller.dispose();
  });

  it("suppresses stale errors and all callbacks after disposal", async () => {
    const first = deferred<number>();
    const onError = vi.fn();
    const onValue = vi.fn();
    const onBusy = vi.fn();
    const poller = createVisiblePoller({ read: () => first.promise, onValue, onError, onBusy });
    poller.configure(config);
    poller.dispose();
    poller.dispose();
    first.reject(new Error("late"));
    await vi.advanceTimersByTimeAsync(120_000);
    poller.configure(config);
    await poller.refresh();
    expect(onValue).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
    expect(onBusy.mock.calls).toEqual([[true]]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("changes intervals without an extra read and normalizes bad preferences", async () => {
    const read = vi.fn(async () => 1);
    const poller = createVisiblePoller({ read, onValue: vi.fn(), onError: vi.fn() });
    poller.configure(config);
    await vi.advanceTimersByTimeAsync(0);
    poller.configure({ ...config, intervalMs: -1 });
    await vi.advanceTimersByTimeAsync(999);
    expect(read).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(read).toHaveBeenCalledTimes(2);
    poller.configure({ ...config, intervalMs: NaN });
    await vi.advanceTimersByTimeAsync(59_999);
    expect(read).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(read).toHaveBeenCalledTimes(3);
    poller.dispose();
  });
});
