import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { UpcomingResetView } from "./UpcomingReset";
import { resetCommitmentExcerpt, resetDeliveryExcerpt, sourceDateBounds, upcomingResetNotices } from "./upcomingResetModel";
import type { PublicResetTimeline, PublicTimelineEntry } from "./types";

const now = Date.parse("2026-09-12T06:30:00Z");
const fullText = `${"Synthetic background explanation. ".repeat(12)}\nA reset is landing by midnight today.`;
function timeline(): PublicResetTimeline {
  const urls = ["https://codex-reset.com/api/feed", "https://codex-reset.com/api/timeline", "https://quotaresets.com/api/v1/events.json"];
  const sourceIds = ["codex_reset_posts", "codex_reset", "quotaresets"] as const;
  const entries: PublicTimelineEntry[] = urls.map((url, index) => ({ disposition: "needs_review", signal: {
    signalId: `internal_${index}`, revision: 1, evidenceFamilyId: "one_post", eventId: index === 2 ? "one_reset" : null,
    kind: "global_full_reset", semantics: "possible_signal", scope: "unknown", recordedAt: "2026-09-12T06:25:00Z", occurredAt: null,
    title: "Source text", summary: index === 0 ? fullText : "A short snippet without the commitment.",
    source: { canonicalUrl: "https://x.com/thsottiaux/status/123456", author: "Tibo", review: "indirect", sourceClass: "official_social",
      publishedAt: "2026-09-12T03:20:36Z", collectedAt: "2026-09-12T06:25:00Z", contentSha256: "a".repeat(64), parserVersion: "synthetic",
      discoveredVia: [url] },
    announcementTiming: index === 2 ? { expectedOn: "2026-09-11", timeZone: "America/Los_Angeles", sourceUrl: url } : null,
  } }));
  return { entries, revisionCount: 3, evidenceFamilyCount: 1, sources: urls.map((url, index) => ({ sourceId: sourceIds[index], sourceUrl: url,
    lastSuccessAt: "2026-09-12T06:25:00Z", lastAttemptAt: "2026-09-12T06:25:00Z", issue: null, acceptedRecords: 1, rejectedRecords: 0, skippedRecords: 0 })) };
}

describe("near-term reset announcements", () => {
  it("surfaces an in-progress banked grant without inventing a full reset or account receipt", () => {
    const data = timeline();
    data.entries.forEach((entry) => { entry.signal.kind = "global_banked_reset_grant"; entry.signal.announcementTiming = null; });
    data.entries[0].signal.summary = "Synthetic release introduction. We are loading a banked reset into all accounts of our Plus, Pro and Business users.";
    expect(upcomingResetNotices(data, now)[0]).toMatchObject({ state: "announced_delivery", kind: "grant", timing: null });
    const html = renderToStaticMarkup(<UpcomingResetView timeline={data} now={now} language="zh" />);
    expect(html).toContain("Tibo 宣布正在发放重置卡");
    expect(html).toContain("可用卡数见「当前账号」");
    expect(html).not.toContain("具体时间待确认");
    for (const text of ["We are not loading a banked reset.", "Are we loading a banked reset?", "We may be granting a banked reset."]) expect(resetDeliveryExcerpt(text)).toBeNull();
  });
  it("recognizes a promised Tuesday reset and combines its independent timing hint", () => {
    const data = timeline();
    data.entries[0].signal.summary = "Ladies and gentlemen... start... your... ENGINES. We are almost Tuesday and I promised a reset for Tuesday. Among some other things. See you soon.";
    data.entries.forEach((entry) => { entry.signal.source.publishedAt = "2026-09-22T04:31:32Z"; });
    data.entries[2].signal.announcementTiming!.expectedOn = "2026-09-22";
    const current = Date.parse("2026-09-23T01:00:00Z");
    const result = upcomingResetNotices(data, current);
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({ state: "upcoming", trackerCount: 2, timing: { end: Date.parse("2026-09-23T07:00:00Z") } });
    expect(result[0].excerpt).toContain("I promised a reset for Tuesday");
    expect(result[0].state).not.toBe("reported");
  });
  it("finds a commitment after a long introduction instead of relying on the 160-character snippet", () => {
    expect(resetCommitmentExcerpt(fullText)).toBe("A reset is landing by midnight today.");
    expect(resetCommitmentExcerpt(fullText.slice(0, 160))).toBeNull();
    const result = upcomingResetNotices(timeline(), now);
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({ kind: "unknown", trackerCount: 2, state: "upcoming", stale: false,
      timing: { end: Date.parse("2026-09-12T07:00:00Z"), deadline: true } });
  });
  it("uses IANA date bounds through DST, not permanent PST or the user's own midnight", () => {
    const spring = sourceDateBounds("2026-03-08", "America/Los_Angeles")!;
    const autumn = sourceDateBounds("2026-11-01", "America/Los_Angeles")!;
    expect(spring.end - spring.start).toBe(23 * 3_600_000);
    expect(autumn.end - autumn.start).toBe(25 * 3_600_000);
    for (const [date, zone] of [["2026-02-30", "UTC"], ["2026-09-11", "Not/AZone"], ["bad", "UTC"]]) expect(sourceDateBounds(date, zone)).toBeNull();
  });
  it("does not upgrade wishes, questions, possible language, negation, or already-completed resets", () => {
    for (const text of ["We will not reset usage today.", "Will you reset today?", "A reset may be coming.", "I hope a reset is coming.", "All reset for everyone.", "No reset is landing today."]) expect(resetCommitmentExcerpt(text)).toBeNull();
    expect(resetCommitmentExcerpt("We will grant a banked reset today.")).not.toBeNull();
  });
  it("keeps inferred timing explicit, shows the converted time, and does not show a fake probability", () => {
    const html = renderToStaticMarkup(<UpcomingResetView timeline={timeline()} now={now} language="zh" />);
    expect(html).toContain("Tibo 提及重置 · 类型尚未明确");
    expect(html).toContain("约 30 分钟");
    expect(html).toContain("追踪站预计最晚时间");
    expect(html).toContain("America/Los_Angeles");
    expect(html).toContain("2 个追踪网站收录 · 同一条原始发言");
    expect(html).not.toContain("100%"); expect(html).not.toContain("83%");
    for (const code of ["possible_signal", "internal_0", "one_post"]) expect(html).not.toContain(code);
  });
  it("does not declare a reset when a reference deadline elapses", () => {
    const later = Date.parse("2026-09-12T07:10:00Z");
    expect(upcomingResetNotices(timeline(), later)[0].state).toBe("elapsed");
    const html = renderToStaticMarkup(<UpcomingResetView timeline={timeline()} now={later} language="zh" />);
    expect(html).toContain("预计时间已过");
    expect(html).not.toContain("距离参考期限");
  });
  it("leaves unknown and conflicting time interpretations without a countdown", () => {
    const data = timeline(); data.entries[2].signal.announcementTiming = null;
    expect(upcomingResetNotices(data, now)[0].timing).toBeNull();
    expect(renderToStaticMarkup(<UpcomingResetView timeline={data} now={now} language="zh" />)).toContain("具体时间待确认");
    const other = structuredClone(timeline().entries[2]); other.signal.signalId = "other";
    other.signal.announcementTiming!.timeZone = "Asia/Shanghai";
    const conflicting = timeline(); conflicting.entries.push(other);
    expect(upcomingResetNotices(conflicting, now)[0]).toMatchObject({ timing: null, conflictingTiming: true });
  });
  it("does not convert every expected date into a deadline", () => {
    const data = timeline(); data.entries[0].signal.summary = "We will reset quota tomorrow.";
    expect(upcomingResetNotices(data, now)[0].timing?.deadline).toBe(false);
    const html = renderToStaticMarkup(<UpcomingResetView timeline={data} now={now} language="zh" />);
    expect(html).toContain("追踪站预计日期"); expect(html).not.toContain("距离参考期限");
  });
  it("keeps corrections, tracker confirmations and actual account receipt separate", () => {
    const data = timeline(); data.entries[2].signal.semantics = "confirmed_reset"; data.entries[2].signal.occurredAt = "2026-09-12T06:29:00Z";
    expect(upcomingResetNotices(data, now)[0].state).toBe("reported");
    expect(renderToStaticMarkup(<UpcomingResetView timeline={data} now={now} language="zh" />)).toContain("追踪站报告已发生");
    const correction = structuredClone(data.entries[2]); correction.signal.revision = 2; correction.disposition = "retracted"; data.entries.push(correction);
    expect(upcomingResetNotices(data, now)[0].state).toBe("withdrawn");
  });
  it("does not promote wrong authors, personal records, future dates, stale sources or multiple relay endpoints", () => {
    const data = timeline(); data.sources[0].issue = "upstream_stale";
    expect(upcomingResetNotices(data, now)[0]).toMatchObject({ stale: true, trackerCount: 2 });
    expect(renderToStaticMarkup(<UpcomingResetView timeline={data} now={now} language="zh" />)).toContain("来源待更新，显示上次资料");
    for (const entry of data.entries) entry.signal.source.canonicalUrl = "https://x.com/other/status/123456";
    expect(upcomingResetNotices(data, now)).toEqual([]);
    const personal = timeline(); personal.entries.forEach((entry) => { entry.signal.scope = "personal"; });
    expect(upcomingResetNotices(personal, now)).toEqual([]);
    expect(upcomingResetNotices(timeline(), Date.parse("2026-09-12T01:00:00Z"))).toEqual([]);
    expect(upcomingResetNotices(timeline(), now + 4 * 86_400_000)).toEqual([]);
  });
});
