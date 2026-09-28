import { describe, expect, it } from "vitest";
import {
  createAccountRefreshSingleFlight,
  isFreshUsageRefresh,
  isLikelyTransientUsageError,
  runBoundedAccountRefresh,
  type AccountRefreshProgress,
} from "./accountRefreshCoordinator";

describe("account refresh coordinator", () => {
  it("deduplicates targets and never exceeds the concurrency bound", async () => {
    let active = 0;
    let maximumActive = 0;
    const progress: AccountRefreshProgress[] = [];

    const result = await runBoundedAccountRefresh(
      ["one", "two", "one", "three", "four"],
      async () => {
        active += 1;
        maximumActive = Math.max(maximumActive, active);
        await new Promise((resolve) => setTimeout(resolve, 2));
        active -= 1;
      },
      { maxConcurrency: 2, maxAttempts: 1, onProgress: (value) => progress.push(value) },
    );

    expect(maximumActive).toBe(2);
    expect(result.total).toBe(4);
    expect(result.succeededIds).toHaveLength(4);
    expect(result.failures).toEqual([]);
    expect(progress.at(-1)).toEqual({
      total: 4,
      completed: 4,
      succeeded: 4,
      failed: 0,
      activeIds: [],
    });
  });

  it("retries transient failures once without retrying terminal authentication failures", async () => {
    const attempts = new Map<string, number>();
    const result = await runBoundedAccountRefresh(
      ["transient", "terminal"],
      async (id) => {
        attempts.set(id, (attempts.get(id) ?? 0) + 1);
        if (id === "transient" && attempts.get(id) === 1) throw new Error("network timeout");
        if (id === "terminal") throw new Error("HTTP 401 Unauthorized");
      },
      { maxConcurrency: 1, retryDelayMs: 0 },
    );

    expect(attempts.get("transient")).toBe(2);
    expect(attempts.get("terminal")).toBe(1);
    expect(result.succeededIds).toEqual(["transient"]);
    expect(result.failures).toHaveLength(1);
    expect(result.failures[0]).toMatchObject({ id: "terminal", attempts: 1 });
  });

  it("classifies cached snapshots and common transient transport failures", () => {
    const startedAt = Date.parse("2026-09-04T08:00:00Z");

    expect(isFreshUsageRefresh({ fetchedAt: "2026-09-04T08:00:01Z" }, startedAt)).toBe(true);
    expect(isFreshUsageRefresh({ fetchedAt: "2026-09-04T07:59:00Z" }, startedAt)).toBe(false);
    expect(isFreshUsageRefresh({}, startedAt)).toBe(false);
    expect(isFreshUsageRefresh({ fetchedAt: "2026-09-04T08:00:01Z", error: "HTTP 503" }, startedAt)).toBe(false);
    expect(isLikelyTransientUsageError(new Error("Codex request timed out"))).toBe(true);
    expect(isLikelyTransientUsageError(new Error("HTTP 503 Service Unavailable"))).toBe(true);
    expect(isLikelyTransientUsageError(new Error("HTTP 403 Forbidden"))).toBe(false);
  });

  it("coalesces overlapping refreshes for the same account", async () => {
    let calls = 0;
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const refresh = createAccountRefreshSingleFlight(async () => {
      calls += 1;
      await gate;
    });

    const first = refresh("same-account");
    const second = refresh("same-account");
    expect(first).toBe(second);
    expect(calls).toBe(1);
    release();
    await Promise.all([first, second]);

    await refresh("same-account");
    expect(calls).toBe(2);
  });
});
