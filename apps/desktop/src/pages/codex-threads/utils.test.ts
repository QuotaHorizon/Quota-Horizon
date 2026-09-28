import { describe, expect, it } from "vitest";
import type { CodexThreadEntry, CodexThreadKind, CodexThreadStatus } from "../../types";
import { canArchiveThreads, filterThreads, relativeTime, retainVisibleThreadSelection, threadActivityDate } from "./utils";

function thread(
  sessionId: string,
  status: CodexThreadStatus,
  sessionKind: CodexThreadKind = "conversation",
): CodexThreadEntry {
  return {
    sessionId,
    sessionKind,
    status,
    title: sessionId,
    cwd: "/tmp/project",
    updatedAt: 1,
    sizeBytes: 10,
    matchExcerpt: null,
    accountId: null,
    accountEmail: null,
    accountActive: false,
  };
}

describe("Codex thread status controls", () => {
  const threads = [
    thread("active-main", "active"),
    thread("archived-main", "archived"),
    thread("active-agent", "active", "subagent"),
  ];

  it("keeps active as a distinct filter while allowing archived inspection", () => {
    expect(filterThreads(threads, "all", "active").map((item) => item.sessionId))
      .toEqual(["active-main", "active-agent"]);
    expect(filterThreads(threads, "conversation", "archived").map((item) => item.sessionId))
      .toEqual(["archived-main"]);
  });

  it("enables batch archive only when every selected session is active", () => {
    expect(canArchiveThreads(threads, new Set(["active-main", "active-agent"]))).toBe(true);
    expect(canArchiveThreads(threads, new Set(["active-main", "archived-main"]))).toBe(false);
    expect(canArchiveThreads(threads, new Set())).toBe(false);
  });

  it("drops selections hidden by status or kind filters", () => {
    const selected = new Set(["active-main", "archived-main"]);
    const retained = retainVisibleThreadSelection(
      filterThreads(threads, "all", "archived"),
      selected,
    );

    expect([...retained]).toEqual(["archived-main"]);
  });

  it("labels activity as recency, not an ambiguous duration", () => {
    const now = Date.parse("2026-09-07T00:00:00Z");
    expect(relativeTime(now / 1000 - 10, "zh", now)).toBe("刚刚");
    expect(relativeTime(now / 1000 - 120, "zh", now)).toBe("2 分钟前");
    expect(relativeTime(now / 1000 - 7200, "en", now)).toBe("2h ago");
    expect(threadActivityDate(now / 1000, "zh")).toMatch(/\d{2}:\d{2}/);
  });
});
