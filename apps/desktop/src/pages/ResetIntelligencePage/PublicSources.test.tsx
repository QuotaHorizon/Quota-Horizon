import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { LiveSourcesView, publicSourceLabel } from "./LiveSourcesPanel";
import { PublicResetArchiveView } from "./ArchiveView";
import { createPublicSourcesStore } from "./publicSourcesStore";
import type { PublicResetTimeline } from "./types";

vi.mock("../../api/backend", () => ({ getPublicResetTimeline: vi.fn(), refreshPublicResetTimeline: vi.fn() }));

function timeline(): PublicResetTimeline {
  return {
    sources: [{ sourceId: "quotaresets", sourceUrl: "https://quotaresets.com/api/v1/events.json",
      lastAttemptAt: "2026-09-12T01:01:00Z", lastSuccessAt: "2026-09-12T01:00:00Z", issue: null,
      acceptedRecords: 1, rejectedRecords: 0, skippedRecords: 0 }],
    entries: [{ disposition: "needs_review", signal: {
      signalId: "internal_signal", evidenceFamilyId: "internal_family", revision: 2, eventId: null,
      title: "Synthetic source text", summary: "<script>not executed</script>", kind: "global_full_reset", scope: "unknown",
      recordedAt: "2026-09-12T01:00:00Z", occurredAt: null,
      source: { canonicalUrl: "https://x.com/thsottiaux/status/123", author: "OpenAI", review: "indirect",
        sourceClass: "official_social", publishedAt: null, collectedAt: "2026-09-12T01:00:00Z",
        contentSha256: "a".repeat(64), parserVersion: "internal_parser", discoveredVia: [] },
    } }], revisionCount: 2, evidenceFamilyCount: 1,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

describe("public-source page continuity", () => {
  it("receives native background updates without starting a second collection", async () => {
    const cache = timeline();
    let changed!: () => void;
    let failed!: () => void;
    const stop = vi.fn();
    const api = { load: vi.fn(async () => cache), refresh: vi.fn(async () => cache),
      subscribe: (onChange: () => void, onFailure: () => void) => { changed = onChange; failed = onFailure; return stop; } };
    const store = createPublicSourcesStore(api);
    const unsubscribe = store.subscribe(vi.fn());
    changed(); await store.reload();
    expect(store.getSnapshot().timeline).toEqual(cache);
    expect(api.refresh).not.toHaveBeenCalled();
    failed(); expect(store.getSnapshot().failed).toBe(true);
    changed(); await store.reload(); expect(store.getSnapshot().failed).toBe(false);
    unsubscribe(); expect(stop).toHaveBeenCalledOnce();
  });
  it("does not let an older refresh response roll back newer cached evidence", async () => {
    const newer = { ...timeline(), revisionCount: 8 };
    const store = createPublicSourcesStore({ load: async () => newer, refresh: async () => timeline() });
    await store.reload(); await store.refresh();
    expect(store.getSnapshot().timeline?.revisionCount).toBe(8);
  });
  it("publishes the disk cache before live completion and shares concurrent refreshes", async () => {
    const live = deferred<PublicResetTimeline>();
    const cache = timeline();
    const api = { load: vi.fn(async () => cache), refresh: vi.fn(() => live.promise) };
    const store = createPublicSourcesStore(api);
    const first = store.refresh();
    const second = store.refresh(true);
    expect(first).toBe(second);
    await Promise.resolve();
    expect(store.getSnapshot()).toMatchObject({ timeline: cache, busy: true, failed: false });
    live.resolve({ ...cache, revisionCount: 3 });
    await first;
    expect(api.load).toHaveBeenCalledTimes(1);
    expect(api.refresh).toHaveBeenCalledTimes(1);
    expect(store.getSnapshot().timeline?.revisionCount).toBe(3);
    expect(store.getSnapshot().busy).toBe(false);
  });

  it("retains evidence on failure and page resubscription, then permits a manual retry", async () => {
    const cache = timeline();
    const api = { load: vi.fn(async () => cache), refresh: vi.fn(async () => { throw new Error("private internal detail"); }) };
    const store = createPublicSourcesStore(api);
    const listener = vi.fn();
    const unsubscribe = store.subscribe(listener);
    await store.refresh();
    expect(store.getSnapshot()).toEqual({ timeline: cache, busy: false, failed: true });
    unsubscribe();
    const called = listener.mock.calls.length;
    await store.refresh(true);
    expect(api.load).toHaveBeenCalledTimes(1);
    expect(api.refresh).toHaveBeenLastCalledWith(true);
    expect(listener).toHaveBeenCalledTimes(called);
    expect(store.getSnapshot().timeline).toBe(cache);
  });

  it("can recover when the initial cache read fails", async () => {
    const data = timeline();
    const store = createPublicSourcesStore({ load: async () => { throw new Error("unreadable"); }, refresh: async () => data });
    await store.refresh();
    expect(store.getSnapshot()).toEqual({ timeline: data, busy: false, failed: false });
  });

  it("never exposes issue enums, markup or implementation IDs as visible content", () => {
    const data = timeline();
    data.sources[0].issue = "schema_changed";
    const html = renderToStaticMarkup(<LiveSourcesView initialSection="details" state={{ timeline: data, busy: true, failed: false }} language="zh" now={0} />);
    expect(html).toContain("来源格式已变化");
    expect(html).toContain("上次成功");
    expect(html).toContain("待核实线索");
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
    for (const code of ["schema_changed", "needs_review", "internal_signal", "internal_parser", "internal_family"]) expect(html).not.toContain(code);
    expect(html).not.toContain("100%");
  });

  it("does not promote a confirmation or personal observation in the automatic channel", () => {
    const data = timeline();
    data.entries[0].disposition = "confirmed";
    data.entries[0].signal.scope = "personal";
    const html = renderToStaticMarkup(<LiveSourcesView initialSection="details" state={{ timeline: data, busy: false, failed: true }} language="zh" now={0} />);
    expect(html).toContain("待核实线索");
    expect(html).toContain("个人账号报告");
    expect(html).toContain("追踪网站收录的个人账号报告");
    expect(html).not.toContain("已核实公告");
    expect(html).toContain("显示上次结果");
    expect(html).toContain("Synthetic source text");
  });

  it("bounds initial rendering and distinguishes an empty cache from no reset", () => {
    const data = timeline();
    data.entries = Array.from({ length: 20 }, (_, index) => ({ ...data.entries[0], signal: { ...data.entries[0].signal, signalId: `id${index}` } }));
    const html = renderToStaticMarkup(<LiveSourcesView initialSection="details" state={{ timeline: data, busy: false, failed: false }} language="zh" now={0} />);
    expect(html.match(/<h3>Synthetic source text/g)).toHaveLength(6);
    expect(html).toContain("还有 14 条");
    data.entries = [];
    expect(renderToStaticMarkup(<LiveSourcesView initialSection="details" state={{ timeline: data, busy: false, failed: false }} language="zh" now={0} />)).toContain("暂无此类记录");
  });

  it("uses source success time, not opening time, for freshness", () => {
    const source = timeline().sources[0];
    const now = Date.parse(source.lastSuccessAt!);
    expect(publicSourceLabel(source, "zh", now)).toBe("更新成功");
    expect(publicSourceLabel(source, "zh", now + 900_000)).toBe("等待更新");
    expect(publicSourceLabel(source, "zh", now - 1)).toBe("需核对时间");
    expect(publicSourceLabel({ ...source, lastSuccessAt: null }, "zh", now)).toBe("尚未读取");
    expect(publicSourceLabel({ ...source, rejectedRecords: 2 }, "zh", now)).toBe("部分资料可用");
    expect(publicSourceLabel({ ...source, issue: "request_failed" }, "en", now)).toBe("Connection failed");
  });

  it("keeps the dated archive without unfinished forecast placeholders", () => {
    const html = renderToStaticMarkup(<PublicResetArchiveView language="zh" archive={{
      schemaVersion: 1, mode: "bundled_archive", reviewedAt: "2026-09-07T04:00:00Z", entries: [],
      evidenceFamilyCount: 0, historyComplete: false, forecastStatus: "abstained",
    }} />);
    expect(html).toContain("应用附带资料");
    expect(html).toContain("归档截至");
    expect(html).not.toContain("Horizon 自有预测模型");
    expect(html).not.toContain("实时来源与可回测历史尚未接齐");
    expect(html).not.toContain("0%");
  });
});
