import { describe, expect, it } from "vitest";
import type { DesktopHistoryEnvelope } from "../../../../capacity-preview/src/status";
import { settleHistory, visibleHistory, type HistoryView } from "./historyState";

const history: DesktopHistoryEnvelope = {
  schemaVersion: "1.0", historyContextId: "account-a-context", status: "available", reasonCode: "history_available",
  points: [{ snapshotId: "sample", capturedAt: "2026-09-07T00:00:00Z", limitId: "codex:primary", label: null, windowMinutes: 10080,
    usedPercent: 72, remainingPercent: 28, resetsAt: "2026-09-10T00:00:00Z", availability: "complete", compatibility: "tested" }],
};
const view: HistoryView = { key: "a", history, issue: null };

describe("local history updates", () => {
  it("retains the same chart while the next same-account request is pending", () => {
    expect(visibleHistory(view, "a")).toBe(view);
    expect(settleHistory(view, "a", history.historyContextId, structuredClone(history)).history).toBe(history);
  });
  it("retains the last result on a transient failure, with an explicit stale notice", () => {
    const next = settleHistory(view, "a", history.historyContextId, { ...history, status: "failed", reasonCode: "history_query_failed", points: [] });
    expect(next.history).toBe(history);
    expect(next.issue).toBe("history_query_failed");
  });
  it("hides old-account data immediately, before the new account fetch finishes", () => {
    expect(visibleHistory(view, "b").history).toBeNull();
    const next = settleHistory(view, "b", "account-b-context", history);
    expect(next.history).toBeNull();
    expect(next.issue).toBe("history_context_unavailable");
  });
  it("rejects a query that switched context before a matching status arrived", () => {
    expect(settleHistory(view, "a", history.historyContextId, { ...history, historyContextId: "account-b-context" }).history).toBeNull();
  });
  it("clears protected history on authorization loss and accepts a deliberate empty result", () => {
    const denied = { ...history, status: "binding_required" as const, reasonCode: "history_keychain_denied", points: [] };
    expect(settleHistory(view, "a", history.historyContextId, denied).history).toEqual(denied);
    expect(settleHistory(view, "a", history.historyContextId, { ...history, points: [] }).history?.points).toEqual([]);
  });
  it("replaces new data without mutating the previous view or manufacturing samples", () => {
    const next = { ...history, points: [{ ...history.points[0], snapshotId: "second", capturedAt: "2026-09-07T00:01:00Z", remainingPercent: 27 }, ...history.points] };
    expect(settleHistory(view, "a", history.historyContextId, next).history?.points.map((point) => point.snapshotId)).toEqual(["sample", "second"]);
    expect(view.history?.points).toHaveLength(1);
    expect(next.points[0].snapshotId).toBe("second");
  });
});
