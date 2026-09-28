import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ResetChanges } from "./ResetChanges";
import { resetSourceChanges } from "./resetChangesModel";
import { publicEvidenceTime } from "./presentation";
import { calendarEventTime, exactPublicTime } from "./resetCalendarModel";
import type { PublicResetTimeline, PublicRevisionChange } from "./types";

const now = Date.parse("2026-09-26T03:00:00Z");
type Signal = PublicRevisionChange["current"];
function signal(revision = 1): Signal {
  return {
    signalId: "internal_signal", revision, evidenceFamilyId: "internal_family", eventId: null,
    kind: "global_banked_reset_grant", scope: "unknown", semantics: "explicit_future_reset",
    title: "Synthetic announcement", summary: "We will load a banked reset into all accounts tomorrow.",
    recordedAt: `2026-09-23T0${revision}:00:00Z`, occurredAt: null,
    source: { canonicalUrl: "https://x.com/thsottiaux/status/123456", author: "Tibo", review: "indirect",
      sourceClass: "official_social", publishedAt: "2026-09-22T18:23:37Z", collectedAt: "2026-09-23T01:00:00Z",
      contentSha256: "a".repeat(64), parserVersion: "internal_parser", discoveredVia: ["https://codex-reset.com/api/timeline"] },
  };
}
function timeline(items: PublicRevisionChange[]): PublicResetTimeline {
  const latest = new Map<string, Signal>();
  for (const { current } of items) if (!latest.has(current.signalId) || latest.get(current.signalId)!.revision < current.revision) latest.set(current.signalId, current);
  return { sources: [], entries: [...latest.values()].map((current) => ({ signal: current, disposition: "needs_review" })),
    revisionCount: items.length, evidenceFamilyCount: latest.size, changes: { items, totalCount: items.length } };
}
function render(data: PublicResetTimeline | null, language: "zh" | "en" = "zh") {
  return renderToStaticMarkup(<ResetChanges timeline={data} language={language} now={now} />);
}

describe("public reset revision changes", () => {
  it("shows a real promise-to-rollout transition without claiming account receipt", () => {
    const before = signal(); const after = signal(2);
    after.semantics = "possible_signal";
    after.summary = "We are loading a banked reset into all accounts of our Plus, Pro and Business users.";
    const data = timeline([{ previous: null, current: before }, { previous: before, current: after }]);
    expect(resetSourceChanges(data, now)).toMatchObject([{ kind: "progress", previousStage: "announced", stage: "rollout" }, { kind: "first" }]);
    const html = render(data);
    expect(html).toContain("重置预告"); expect(html).toContain("宣布开始发卡");
    expect(html).toContain("宣布开始发卡");
    expect(html).toContain(before.summary); expect(html).toContain(after.summary);
    expect(html).not.toContain("已核实公告");
  });

  it("does not mistake a repeated check, parser revision, or whitespace for progress", () => {
    const before = signal(); const after = signal(2);
    after.source.parserVersion = "new_internal_parser"; after.source.contentSha256 = "b".repeat(64);
    after.source.collectedAt = after.recordedAt;
    after.summary = `${before.summary.replace(/ /gu, "  ")}\n`;
    expect(resetSourceChanges(timeline([{ previous: before, current: after }]), now)).toEqual([]);
    const empty = render(timeline([]));
    expect(empty).toContain("暂无重置相关更新");
  });

  it("collapses identical transitions from mirror feeds without replacing first discovery time", () => {
    const first = signal(); const mirror = structuredClone(first);
    mirror.signalId = "mirror_signal"; mirror.recordedAt = "2026-09-24T01:00:00Z";
    mirror.source.discoveredVia = ["https://quotaresets.com/api/v1/events.json"];
    mirror.summary = "A shorter rendering of the same announcement.";
    const data = timeline([{ previous: null, current: mirror }, { previous: null, current: first }]);
    expect(resetSourceChanges(data, now)).toMatchObject([{ current: { signalId: first.signalId }, recordedAt: Date.parse(first.recordedAt) }]);
    expect(resetSourceChanges(data, now)[0].sources).toHaveLength(2);
    expect(render(data)).toContain("相同原文已合并转载");
  });

  it("retains a genuinely repeated state cycle on the same source", () => {
    const first = signal(); const retracted = signal(2); retracted.semantics = "retracted";
    const returned = signal(3); const secondRetraction = signal(4); secondRetraction.semantics = "retracted";
    const data = timeline([{ previous: first, current: retracted }, { previous: retracted, current: returned }, { previous: returned, current: secondRetraction }]);
    expect(resetSourceChanges(data, now).map((item) => item.current.revision)).toEqual([4, 3, 2]);
  });

  it("labels old first observations as backfills and separates local discovery from publication", () => {
    const old = signal(); old.recordedAt = "2026-09-26T01:00:00Z";
    const data = timeline([{ previous: null, current: old }]);
    expect(resetSourceChanges(data, now)[0]).toMatchObject({ backfilled: true, kind: "first" });
    const html = render(data);
    expect(html).toContain("历史补录"); expect(html).toContain("按本机收录时间排列");
    expect(html).toContain(publicEvidenceTime(old.recordedAt, "zh"));
    expect(html).toContain(calendarEventTime(exactPublicTime(old.source.publishedAt), "zh"));
    expect(html).toContain("原帖发布时间");
  });

  it("retains unknown publication time and never invents occurrence time", () => {
    const item = signal(); item.source.publishedAt = null;
    const html = render(timeline([{ previous: null, current: item }]));
    expect(html).toContain("原帖发布时间：—");
    expect(html).not.toContain("来源标注的发生/确认时间");
    expect(resetSourceChanges(timeline([{ previous: null, current: item }]), now)[0].backfilled).toBe(false);
  });

  it("attributes tracker confirmations and exposes subsequent corrections across mirrors", () => {
    const first = signal(); const confirmed = signal(2); confirmed.semantics = "confirmed_reset";
    confirmed.occurredAt = "2026-09-23T01:30:00Z";
    const corrected = signal(3); corrected.signalId = "mirror_corrected"; corrected.revision = 1; corrected.semantics = "corrected";
    const data = timeline([{ previous: first, current: confirmed }, { previous: null, current: corrected }]);
    expect(resetSourceChanges(data, now).every((item) => item.withdrawn)).toBe(true);
    const html = render(data);
    expect(html).toContain("追踪站报告已发生"); expect(html).toContain("旧预告已失效");
    expect(html).toContain("追踪记录已更正"); expect(html).not.toContain("已核实公告");
    // A later restored revision removes the warning; historical entries remain.
    const restored = structuredClone(corrected); restored.revision = 2; restored.semantics = "explicit_future_reset";
    data.entries.push({ signal: restored, disposition: "needs_review" });
    expect(resetSourceChanges(data, now).every((item) => !item.withdrawn)).toBe(true);
  });

  it("keeps reported grant and full-reset classifications separate", () => {
    const first = signal(); first.semantics = "confirmed_reset"; first.kind = "global_full_reset";
    const after = signal(2); after.semantics = "confirmed_reset";
    const data = timeline([{ previous: first, current: after }]);
    expect(resetSourceChanges(data, now)[0].kind).toBe("kind");
    const html = render(data); expect(html).toContain("事件类型调整"); expect(html).toContain("额度重置"); expect(html).toContain("重置卡");
  });

  it("shows time corrections and wording updates without claiming another occurrence", () => {
    const before = signal(); const timed = signal(2);
    timed.announcementTiming = { expectedOn: "2026-09-24", timeZone: "America/Los_Angeles", sourceUrl: "https://quotaresets.com/api/v1/events.json" };
    const wording = structuredClone(timed); wording.revision = 3; wording.summary += " More details soon.";
    const data = timeline([{ previous: before, current: timed }, { previous: timed, current: wording }]);
    expect(resetSourceChanges(data, now).map((item) => item.kind)).toEqual(["text", "timing"]);
    const html = render(data); expect(html).toContain("收录内容更新"); expect(html).toContain("时间依据更新");
    expect(html).toContain("来源时间已调整"); expect(html).toContain("追踪站预计日期");
  });

  it("rejects invalid/future chronology and unrelated personal or policy records", () => {
    const future = signal(); future.recordedAt = "2026-09-30T00:00:00Z";
    const invalid = signal(); invalid.recordedAt = "2026-09-23";
    const personal = signal(); personal.scope = "personal";
    const policy = signal(); policy.kind = "quota_policy_change";
    const context = signal(); context.kind = "unclassified"; context.semantics = "context_only";
    for (const item of [future, invalid, personal, policy, context]) expect(resetSourceChanges(timeline([{ previous: null, current: item }]), now)).toEqual([]);
    const other = signal(); other.signalId = "another_signal";
    expect(resetSourceChanges(timeline([{ previous: other, current: signal(2) }]), now)).toEqual([]);
    expect(resetSourceChanges(timeline([{ previous: signal(3), current: signal(2) }]), now)).toEqual([]);
  });

  it("compares instants instead of timestamp spellings and reveals title-only changes", () => {
    const before = signal(); const after = signal(2);
    after.source.publishedAt = "2026-09-23T02:23:37+08:00";
    expect(resetSourceChanges(timeline([{ previous: before, current: after }]), now)).toEqual([]);
    after.title = "Synthetic changed title";
    const html = render(timeline([{ previous: before, current: after }]));
    expect(html).toContain(before.title); expect(html).toContain(after.title); expect(html).toContain("状态未变");
  });

  it("bounds the default rendering and discloses incomplete history", () => {
    const items = Array.from({ length: 140 }, (_, i) => {
      const item = signal(); item.signalId = `id_${i}`; item.evidenceFamilyId = `family_${i}`;
      return { previous: null, current: item };
    });
    const data = timeline(items.slice(0, 128)); data.changes!.totalCount = 140;
    const html = render(data);
    expect(html.match(/<li\b/gu)).toHaveLength(3);
    expect(html).toContain("展开近期变化（24 条）");
    expect(html).toContain("较早版本已存档");
  });

  it("escapes content, omits unsafe links and renders readable labels in both languages", () => {
    const item = signal(); item.summary = "<script>not executed</script>"; item.source.canonicalUrl = "javascript:alert(1)";
    const data = timeline([{ previous: null, current: item }]);
    for (const language of ["zh", "en"] as const) {
      const html = render(data, language);
      expect(html).toContain("&lt;script&gt;");
      for (const hidden of ["<script>", "javascript:", "internal_signal", "internal_family", "internal_parser", "global_banked_reset_grant", "explicit_future_reset"]) expect(html).not.toContain(hidden);
    }
    expect(render(data, "en")).toContain("First collected");
    const legacy = timeline([]); delete legacy.changes;
    expect(render(legacy)).toBe(""); expect(render(null)).toBe("");
  });
});
