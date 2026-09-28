import { describe, expect, it } from "vitest";
import type { CodexThreadDetailPage, CodexThreadTimelineItem } from "../../types";
import { mergeThreadDetailPage } from "./detailPages";

function page(ids: string[], offset: number, previousOffset: number | null, nextOffset: number | null): CodexThreadDetailPage {
  return {
    summary: { sessionId: "logical", title: "Fixture", cwd: "/fixture", startedAt: null, updatedAt: null,
      originator: "codex", source: "cli", cliVersion: "fixture", modelProvider: "openai", sizeBytes: 10,
      lineCount: 5, eventCount: 5, toolCallCount: 0, userPromptExcerpt: "", latestAgentMessageExcerpt: "", resumeCommand: null },
    items: ids.map((id): CodexThreadTimelineItem => ({ id, kind: "message:assistant", timestamp: null,
      text: id, toolName: null, toolInput: null, toolOutput: null, toolStatus: null, truncated: false })),
    total: 5, offset, previousOffset, nextOffset, revision: "frozen",
  };
}

describe("captured session pagination", () => {
  it("prepends older records without duplicating overlapping pages or losing the newest boundary", () => {
    const latest = page(["3", "4", "5"], 2, 0, null);
    const merged = mergeThreadDetailPage(latest, page(["1", "2", "3"], 0, null, 3), true);
    expect(merged.items.map((item) => item.id)).toEqual(["1", "2", "3", "4", "5"]);
    expect(merged.previousOffset).toBeNull();
    expect(merged.nextOffset).toBeNull();
    expect(merged.offset).toBe(0);
    expect(latest.items).toHaveLength(3);
  });

  it("keeps the earlier boundary when appending a later page", () => {
    const merged = mergeThreadDetailPage(page(["2", "3"], 1, 0, 3), page(["4", "5"], 3, 1, null), false);
    expect(merged.items.map((item) => item.id)).toEqual(["2", "3", "4", "5"]);
    expect(merged.previousOffset).toBe(0);
    expect(merged.offset).toBe(1);
    expect(merged.nextOffset).toBeNull();
  });

  it("rejects a late response from another session or newer revision", () => {
    const current = page(["3", "4", "5"], 2, 0, null);
    const incoming = page(["1", "2"], 0, null, 2);
    expect(mergeThreadDetailPage(current, { ...incoming, revision: "newer" }, true)).toBe(current);
    expect(mergeThreadDetailPage(current, { ...incoming, summary: { ...incoming.summary, sessionId: "different" } }, true)).toBe(current);
  });
});
