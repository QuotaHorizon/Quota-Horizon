import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { DashboardNavigation } from "../../components/dashboard/DashboardNavigation";
import { ResetNoticeChip } from "../../components/CapacityPopover/ResetNoticeChip";
import { translate, type Translate } from "../../i18n";
import { LiveSourcesView } from "./LiveSourcesPanel";
import { unreadResetProgress } from "./resetChangesModel";
import { createPublicSourcesStore } from "./publicSourcesStore";
import type { PublicReadReceipt, PublicResetTimeline, PublicRevisionChange } from "./types";

vi.mock("../../api/backend", () => ({ getPublicResetTimeline: vi.fn(), refreshPublicResetTimeline: vi.fn() }));

const now = Date.parse("2026-09-26T04:00:00Z");
const t: Translate = (key, values) => translate("zh", key, values);
function signal(revision = 1): PublicRevisionChange["current"] {
  return { signalId: "synthetic-source", revision, evidenceFamilyId: "synthetic-family", eventId: null,
    kind: "global_full_reset", scope: "unknown", semantics: "explicit_future_reset", title: "Synthetic statement", summary: "A reset is expected later today.",
    recordedAt: `2026-09-26T0${revision}:00:00Z`, occurredAt: null,
    source: { canonicalUrl: "https://x.com/thsottiaux/status/123", author: "Synthetic source", review: "indirect", sourceClass: "official_social",
      publishedAt: "2026-09-26T00:00:00Z", collectedAt: "2026-09-26T01:00:00Z", contentSha256: "a".repeat(64), parserVersion: "fixture", discoveredVia: [] } };
}
function timeline(items: PublicRevisionChange[] = [{ previous: null, current: signal(), unread: true }], readVersion = 0): PublicResetTimeline {
  return { sources: [], entries: items.map(({ current }) => ({ signal: current, disposition: "needs_review" })), revisionCount: items.length,
    evidenceFamilyCount: 1, changes: { items, totalCount: items.length, readVersion } };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

describe("public reset reading state", () => {
  it("counts progress and corrections, not historical imports, wording edits, or mirrors", () => {
    const first = signal(); const wording = { ...signal(2), summary: "A reset is expected later today!" };
    const corrected = { ...signal(3), semantics: "corrected" as const };
    const items: PublicRevisionChange[] = [{ previous: null, current: first, unread: true }, { previous: first, current: wording, unread: true },
      { previous: wording, current: corrected, unread: true }];
    const mirror = structuredClone(items[2]); mirror.current.signalId = "mirror"; mirror.previous!.signalId = "mirror";
    const old = signal(); old.signalId = "old"; old.evidenceFamilyId = "old-family"; old.source.publishedAt = "2026-09-20T00:00:00Z";
    const data = timeline([...items, mirror, { previous: null, current: old, unread: true }]);
    expect(unreadResetProgress(data, now)).toHaveLength(2);
    expect(unreadResetProgress(data, now)[0].stage).toBe("corrected");
  });

  it("does not relight a reviewed claim when another site first mirrors it", () => {
    const original = signal(); const mirror = { ...signal(), signalId: "mirror", recordedAt: "2026-09-26T02:00:00Z" };
    expect(unreadResetProgress(timeline([{ previous: null, current: original, unread: false }, { previous: null, current: mirror, unread: true }]), now)).toEqual([]);
  });

  it("retains a later repeated correction on the same source", () => {
    const first = signal(); const corrected = { ...signal(2), semantics: "corrected" as const }; const returned = signal(3);
    const correctedAgain = { ...signal(4), semantics: "corrected" as const };
    const data = timeline([{ previous: first, current: corrected, unread: false }, { previous: corrected, current: returned, unread: false }, { previous: returned, current: correctedAgain, unread: true }]);
    expect(unreadResetProgress(data, now)).toHaveLength(1);
  });

  it("never invents unread state for an older backend or out-of-time evidence", () => {
    expect(unreadResetProgress(timeline([{ previous: null, current: signal() }]), now)).toEqual([]);
    expect(unreadResetProgress(timeline(), now - 86_400_000)).toEqual([]);
  });

  it("shows local read actions and failure recovery without claiming an official reset", () => {
    const html = renderToStaticMarkup(<LiveSourcesView state={{ timeline: timeline(), busy: false, failed: false, readFailed: true }} language="zh" now={now} onMarkRead={() => undefined} />);
    expect(html).not.toContain("1 条新进展"); expect(html).toContain("全部已读");
    expect(html).toContain("已读状态保存失败"); expect(html).not.toContain("已核实公告");
    expect(html.indexOf("全部已读")).toBeLessThan(html.indexOf("第三方重置预测"));
    expect(html).not.toContain("情报有什么变化"); expect(html).not.toContain("公开来源状态");
    for (const section of ["概览", "重置历史", "详细资料"]) expect(html).toContain(section);
    const busy = renderToStaticMarkup(<LiveSourcesView state={{ timeline: timeline(), busy: false, failed: false, readBusy: true }} language="en" now={now} onMarkRead={() => undefined} />);
    expect(busy).toContain("Saving"); expect(busy).toContain('disabled=""');
  });

  it("keeps a labelled badge in expanded and collapsed navigation and in the menu", () => {
    for (const collapsed of [false, true]) {
      const html = renderToStaticMarkup(<DashboardNavigation page="accounts" collapsed={collapsed} onPageChange={() => undefined} t={t} resetUpdatesCount={2} />);
      expect(html).toContain('aria-label="重置情报 · 有新动态"');
      expect(html).toContain('class="reset-unread-badge"');
      expect(html).not.toContain('aria-hidden="true">2</em>');
    }
    const chip = renderToStaticMarkup(<ResetNoticeChip now={now} language="zh" newUpdates={2} onOpen={() => undefined} />);
    expect(chip).toContain("有新动态"); expect(chip).not.toContain("2 条"); expect(chip).toContain("查看详情");
  });

  it("keeps routine type/time edits and tracker leads quiet", () => {
    const first = signal(); const kind = { ...signal(2), kind: "global_banked_reset_grant" as const };
    const time = { ...kind, revision: 3, source: { ...kind.source, publishedAt: "2026-09-26T00:30:00Z" } };
    expect(unreadResetProgress(timeline([{ previous: first, current: kind, unread: true }, { previous: kind, current: time, unread: true }]), now)).toEqual([]);
    const lead = { ...signal(), semantics: "possible_signal" as const };
    expect(unreadResetProgress(timeline([{ previous: null, current: lead, unread: true }]), now)).toHaveLength(1);
    lead.source = { ...lead.source, canonicalUrl: "https://example.org/tracker" };
    expect(unreadResetProgress(timeline([{ previous: null, current: lead, unread: true }]), now)).toEqual([]);
  });

  it("queues another seen item while an acknowledgment is pending, without swallowing it", async () => {
    const response = deferred<PublicResetTimeline>();
    const api = { load: async () => timeline(), refresh: vi.fn(async () => timeline()),
      markRead: vi.fn().mockImplementationOnce(() => response.promise).mockResolvedValue(timeline()) };
    const store = createPublicSourcesStore(api);
    const pending = store.markReceipts([{ signalId: "a", revision: 1 }]);
    await Promise.resolve();
    expect(store.markReceipts([{ signalId: "b", revision: 2 }])).toBe(pending);
    store.markReceipts([{ signalId: "a", revision: 1 }]);
    response.resolve(timeline()); await pending;
    expect(api.markRead.mock.calls).toEqual([[[{ signalId: "a", revision: 1 }]], [[{ signalId: "b", revision: 2 }]]]);
    expect(api.refresh).not.toHaveBeenCalled();
  });

  it("bounds receipt batches and never sends an unseen revision", async () => {
    const api = { load: async () => timeline(), refresh: async () => timeline(), markRead: vi.fn(async (_receipts: PublicReadReceipt[]) => timeline()) };
    const store = createPublicSourcesStore(api);
    await store.markReceipts(Array.from({ length: 130 }, (_, index) => ({ signalId: `seen-${index}`, revision: 1 })));
    expect(api.markRead.mock.calls.map(([receipts]) => receipts.length)).toEqual([128, 2]);
  });

  it("persists only supplied public revisions, coalesces read actions and makes no network refresh", async () => {
    const first = timeline(); const saved = timeline([{ ...first.changes!.items[0], unread: false }], 1);
    const pending = deferred<PublicResetTimeline>(); let cache = first;
    const api = { load: vi.fn(async () => cache), refresh: vi.fn(async () => cache), markRead: vi.fn(() => pending.promise) };
    const store = createPublicSourcesStore(api); await store.reload();
    const action = store.markRead(first); expect(store.markRead(first)).toBe(action);
    expect(store.getSnapshot().readBusy).toBe(true);
    cache = saved; pending.resolve(saved); await action;
    expect(api.markRead).toHaveBeenCalledExactlyOnceWith([{ signalId: "synthetic-source", revision: 1 }]);
    expect(store.getSnapshot()).toMatchObject({ readBusy: false, readFailed: false, timeline: saved });
    expect(api.refresh).not.toHaveBeenCalled();
  });

  it("does not let a stale window reload resurrect already read progress", async () => {
    let cache = timeline([{ previous: null, current: signal(), unread: false }], 2);
    const store = createPublicSourcesStore({ load: async () => cache, refresh: async () => cache });
    await store.reload(); cache = timeline(); await store.reload();
    expect(store.getSnapshot().timeline?.changes?.readVersion).toBe(2);
    expect(unreadResetProgress(store.getSnapshot().timeline, now)).toEqual([]);
  });

  it("reconciles a collection that overtakes the read acknowledgment snapshot", async () => {
    const first = timeline(); const response = deferred<PublicResetTimeline>(); let cache = first;
    const store = createPublicSourcesStore({ load: async () => cache, refresh: async () => cache, markRead: () => response.promise });
    await store.reload(); const action = store.markRead(first);
    const corrected = { ...signal(2), semantics: "corrected" as const };
    cache = timeline([...first.changes!.items, { previous: signal(), current: corrected, unread: true }]);
    await store.reload();
    cache = timeline([{ ...first.changes!.items[0], unread: false }, { previous: signal(), current: corrected, unread: true }], 1);
    response.resolve(timeline([{ ...first.changes!.items[0], unread: false }], 1)); await action;
    expect(store.getSnapshot().timeline?.revisionCount).toBe(2);
    expect(store.getSnapshot().timeline?.changes?.readVersion).toBe(1);
    expect(unreadResetProgress(store.getSnapshot().timeline, now)).toMatchObject([{ stage: "corrected" }]);
  });

  it("leaves unread content in place when saving fails, then permits retry", async () => {
    let cache = timeline(); let fails = true;
    const store = createPublicSourcesStore({ load: async () => cache, refresh: async () => cache, markRead: async () => {
      if (fails) throw new Error("not user-facing");
      cache = timeline([{ ...cache.changes!.items[0], unread: false }], 1); return cache;
    } });
    await store.reload(); await store.markRead(cache);
    expect(store.getSnapshot().readFailed).toBe(true); expect(unreadResetProgress(store.getSnapshot().timeline, now)).toHaveLength(1);
    fails = false; await store.markRead(cache);
    expect(store.getSnapshot().readFailed).toBe(false); expect(unreadResetProgress(store.getSnapshot().timeline, now)).toEqual([]);
  });

  it("re-reads when a native completion arrives during an older disk read", async () => {
    const older = deferred<PublicResetTimeline>(); const newer = timeline([{ previous: null, current: signal(), unread: false }], 1);
    let changed!: () => void;
    const load = vi.fn().mockImplementationOnce(() => older.promise).mockResolvedValue(newer);
    const store = createPublicSourcesStore({ load, refresh: async () => newer, subscribe: (callback) => { changed = callback; return () => undefined; } });
    const unsubscribe = store.subscribe(() => undefined);
    const pending = store.reload(); changed(); older.resolve(timeline()); await pending;
    expect(load).toHaveBeenCalledTimes(2); expect(store.getSnapshot().timeline).toBe(newer); unsubscribe();
  });
});
