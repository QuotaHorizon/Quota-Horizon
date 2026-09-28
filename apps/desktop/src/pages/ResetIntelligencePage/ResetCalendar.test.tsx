import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ResetCalendar } from "./ResetCalendar";
import { calendarEventTime, calendarTimeLabel, exactPublicTime, localCalendarDate, resetCalendarMonths, resetCalendarRecords } from "./resetCalendarModel";
import type { PublicResetArchive, PublicResetArchiveEntry, PublicTimelineEntry } from "./types";

const now = new Date(2026, 8, 12, 14, 0).getTime();
function lead(id = "a", kind: PublicTimelineEntry["signal"]["kind"] = "global_full_reset"): PublicTimelineEntry {
  return { disposition: "needs_review", signal: {
    signalId: `signal_${id}`, revision: 1, evidenceFamilyId: `family_${id}`, eventId: null,
    kind, semantics: "confirmed_reset", scope: "unknown", recordedAt: "2026-09-12T03:00:00Z",
    occurredAt: "2026-09-07T19:30:25Z", title: "Synthetic reset notice", summary: "Synthetic source text",
    source: { canonicalUrl: `https://x.com/thsottiaux/status/${id}`, author: "Tibo", review: "indirect", sourceClass: "official_social",
      publishedAt: "2026-09-07T18:00:00Z", collectedAt: "2026-09-12T03:00:00Z", contentSha256: "a".repeat(64),
      parserVersion: "internal_parser", discoveredVia: ["https://quotaresets.com/api/v1/events.json"] },
  } };
}
function reviewed(id = "reviewed", kind: PublicTimelineEntry["signal"]["kind"] = "global_full_reset"): PublicResetArchiveEntry {
  const source = lead(id, kind);
  return { ...source, disposition: "confirmed", title: { zh: "合成已核实事件", en: "Synthetic reviewed event" },
    summary: { zh: "仅供测试", en: "Testing only" }, interpretation: { zh: "不是当前账号到账", en: "Not account receipt" },
    signal: { ...source.signal, scope: "broad_codex", source: { ...source.signal.source, review: "primary_reviewed" } } };
}
function archive(entries: PublicResetArchiveEntry[]): PublicResetArchive {
  return { schemaVersion: 1, mode: "bundled_archive", reviewedAt: "2026-09-12T03:00:00Z", entries,
    evidenceFamilyCount: entries.length, historyComplete: false, forecastStatus: "abstained" };
}

const grantPublishedAt = "2026-09-22T18:23:37Z";
const afterGrant = Date.parse("2026-09-23T04:00:00Z");
function announcedGrant(): PublicTimelineEntry {
  const input = lead("2102463847714247142", "global_banked_reset_grant");
  input.signal.occurredAt = null;
  input.signal.semantics = "possible_signal";
  input.signal.source.publishedAt = grantPublishedAt;
  input.signal.summary = "Synthetic release introduction. We are loading a banked reset into all accounts of our Plus, Pro and Business users.";
  return input;
}

describe("reset calendar evidence and time semantics", () => {
  it("includes an announced card rollout in the default grant calendar without fabricating delivery time", () => {
    const grant = announcedGrant();
    const record = resetCalendarRecords([], [grant], "zh", afterGrant)[0];
    expect(record).toMatchObject({ kind: "grant", mode: "events", timeBasis: "grant_announcement",
      reviewed: false, time: Date.parse(grantPublishedAt), reportedAt: null });
    expect(grant.signal.occurredAt).toBeNull();
    expect(grant.disposition).toBe("needs_review");
    const day = localCalendarDate(new Date(Date.parse(grantPublishedAt)));
    if (process.env.TZ === "Asia/Shanghai") expect(day).toBe("2026-09-23");
    if (process.env.TZ === "America/New_York") expect(day).toBe("2026-09-22");
    const html = renderToStaticMarkup(<ResetCalendar archive={null} timeline={{ sources: [], entries: [grant], revisionCount: 1, evidenceFamilyCount: 1 }}
      now={afterGrant} language="zh" />);
    expect(html).toContain(`${day} · 重置卡发放 · 1 条去重记录 · 已宣布开始发放`);
    expect(html).toContain("发放公告时间");
    expect(html).toContain(calendarEventTime(Date.parse(grantPublishedAt), "zh"));
    expect(html).toContain("Tibo 宣布开始向订阅账号发放重置卡");
    expect(html).not.toContain("grant_announcement");
    expect(html.match(new RegExp(`<button[^>]*aria-label="${day}[^>]*>`, "u"))?.[0]).not.toContain('data-reviewed="true"');
  });

  it("merges the short timeline and full post as one lasting grant record, with no 72-hour expiry", () => {
    const full = announcedGrant();
    const short = structuredClone(full); short.signal.signalId = "short_mirror";
    short.signal.summary = "Synthetic release introduction only.";
    const later = afterGrant + 7 * 86_400_000;
    expect(resetCalendarRecords([], [short, full], "en", later)).toMatchObject([
      { kind: "grant", mode: "events", timeBasis: "grant_announcement", time: Date.parse(grantPublishedAt) },
    ]);
    const en = renderToStaticMarkup(<ResetCalendar archive={null} timeline={{ sources: [], entries: [full], revisionCount: 1, evidenceFamilyCount: 1 }} now={later} language="en" />);
    expect(en).toContain("Rollout announced");
    expect(en).toContain("Rollout announcement time");
  });

  it("keeps future promises, conditional wording, quotes, personal grants and other authors out of rollout records", () => {
    for (const text of [
      "We will load a banked reset into all accounts tomorrow.",
      "We are not loading a banked reset into all accounts.",
      "If approved, we are loading a banked reset into all accounts.",
      "We are loading a banked reset into all accounts tomorrow.",
      "A user said: we are loading a banked reset into all accounts.",
      "We are loading a banked reset into your account.",
    ]) {
      const grant = announcedGrant(); grant.signal.summary = text;
      expect(resetCalendarRecords([], [grant], "en", afterGrant)[0].mode).toBe("announcements");
    }
    const wrongAuthor = announcedGrant(); wrongAuthor.signal.source.canonicalUrl = "https://x.com/other/status/123456";
    const future = announcedGrant(); future.signal.source.publishedAt = "2026-09-24T00:00:00Z";
    const promise = announcedGrant(); promise.signal.semantics = "explicit_future_reset";
    for (const item of [wrongAuthor, future, promise]) expect(resetCalendarRecords([], [item], "en", afterGrant)[0].mode).toBe("announcements");
    const personal = announcedGrant(); personal.signal.scope = "personal";
    expect(resetCalendarRecords([], [personal], "en", afterGrant)).toEqual([]);
  });

  it("withdraws a rollout across mirrors and prefers a later confirmation without mistaking the two times for conflict", () => {
    const grant = announcedGrant();
    const mirror = structuredClone(grant); mirror.signal.signalId = "timeline_mirror";
    const withdrawn = structuredClone(grant); withdrawn.signal.revision = 2; withdrawn.signal.semantics = "retracted";
    expect(resetCalendarRecords([], [grant, mirror, withdrawn], "en", afterGrant)).toMatchObject([{ time: null, withdrawn: true }]);
    mirror.signal.source.publishedAt = "2026-09-22T19:00:00Z";
    expect(resetCalendarRecords([], [grant, mirror], "en", afterGrant)).toMatchObject([{ time: null, conflictingTimes: true }]);
    mirror.signal.source.publishedAt = grantPublishedAt;
    mirror.signal.semantics = "confirmed_reset"; mirror.signal.occurredAt = "2026-09-23T01:00:00Z";
    expect(resetCalendarRecords([], [grant, mirror], "en", afterGrant)).toMatchObject([
      { mode: "events", timeBasis: "tracker_confirmation", time: Date.parse(mirror.signal.occurredAt), conflictingTimes: false },
    ]);
  });

  it("does not count a cached pre-v4 notice as a forced reset or paint it as an occurrence", () => {
    const notice = lead("notice");
    notice.signal.source.publishedAt = "2026-09-22T04:31:32Z";
    notice.signal.occurredAt = null;
    notice.signal.semantics = "possible_signal";
    const html = renderToStaticMarkup(<ResetCalendar archive={null} timeline={{ sources: [], entries: [notice], revisionCount: 1, evidenceFamilyCount: 1 }}
      now={Date.parse("2026-09-23T01:00:00Z")} language="zh" />);
    expect(html).toContain('aria-pressed="true">发生记录');
    expect(html).toContain("2026-09-22 · 未收录记录");
    expect(resetCalendarRecords([], [notice], "zh", Date.parse("2026-09-23T01:00:00Z"))[0]).toMatchObject({kind: "notice", mode: "announcements"});
    const day = html.match(/<button[^>]*aria-label="2026-09-22[^>]*>/u)?.[0];
    expect(day).toBeDefined();
    expect(day).not.toContain('data-reviewed="true"');
  });
  it("keeps occurrence, tracker confirmation and publication times distinct", () => {
    const publicRecord = reviewed();
    const tracker = lead();
    const announcement = lead("upcoming");
    announcement.signal.semantics = "explicit_future_reset";
    const result = resetCalendarRecords([publicRecord], [tracker, announcement], "zh", now);
    expect(result.find((item) => item.reviewed)).toMatchObject({ mode: "events", timeBasis: "occurred" });
    expect(result.find((item) => item.url === tracker.signal.source.canonicalUrl)).toMatchObject({ mode: "events", timeBasis: "tracker_confirmation", reviewed: false });
    expect(result.find((item) => item.url === announcement.signal.source.canonicalUrl)).toMatchObject({ mode: "announcements", timeBasis: "published", time: Date.parse(announcement.signal.source.publishedAt!) });
    expect(calendarTimeLabel("tracker_confirmation", "zh")).toContain("追踪站报告时间");
  });

  it("cannot promote an automatic record by trusting its claimed scope or review flag", () => {
    const input = lead();
    input.disposition = "confirmed"; input.signal.scope = "broad_codex"; input.signal.source.review = "primary_reviewed";
    expect(resetCalendarRecords([], [input], "en", now)[0].reviewed).toBe(false);
  });

  it("does not substitute collection time, infer a time zone, or invent a midnight", () => {
    for (const value of [null, "2026-09-07", "2026-09-07T19:30:00", "garbage", "2026-09-07T19:30:00+99:00", "2026-02-30T00:00:00Z"]) expect(exactPublicTime(value)).toBeNull();
    const input = lead();
    input.signal.occurredAt = "2026-09-07"; input.signal.source.publishedAt = null;
    expect(resetCalendarRecords([], [input], "zh", now)[0]).toMatchObject({ time: null, reviewed: false, timeBasis: "unknown" });
    expect(calendarEventTime(null, "en")).toBe("—");
  });

  it("does not turn an undated reviewed guide and an indirect mirror into a reviewed occurrence", () => {
    const primary = reviewed(); primary.signal.occurredAt = null; primary.signal.source.publishedAt = null;
    const mirror = lead(); mirror.signal.evidenceFamilyId = primary.signal.evidenceFamilyId;
    const records = resetCalendarRecords([primary], [mirror], "zh", now);
    expect(records).toHaveLength(1);
    expect(records[0]).toMatchObject({ reviewed: false, timeBasis: "tracker_confirmation" });
    expect(records[0].sources).toHaveLength(2);
  });

  it("deduplicates transitive original families and explicit event IDs but never different reset kinds", () => {
    const a = lead("a"); const b = lead("b"); const c = lead("c"); const d = lead("d");
    a.signal.eventId = "event_one"; b.signal.eventId = "event_two";
    c.signal.evidenceFamilyId = a.signal.evidenceFamilyId; c.signal.eventId = "event_two";
    d.signal.evidenceFamilyId = b.signal.evidenceFamilyId;
    const grant = lead("grant", "global_banked_reset_grant"); grant.signal.evidenceFamilyId = a.signal.evidenceFamilyId; grant.signal.eventId = "event_one";
    const result = resetCalendarRecords([], [a, b, c, d, grant], "en", now);
    expect(result).toHaveLength(2);
    expect(result.find((item) => item.kind === "reset")?.sources).toHaveLength(4);
    expect(result.find((item) => item.kind === "grant")?.sources).toHaveLength(1);
  });

  it("honors latest revisions and removes mirror withdrawals and conflicting event times from the heatmap", () => {
    const first = lead();
    const revision = structuredClone(first); revision.signal.revision = 2; revision.disposition = "retracted";
    const mirror = lead("mirror"); mirror.signal.evidenceFamilyId = first.signal.evidenceFamilyId;
    expect(resetCalendarRecords([], [first, mirror, revision], "en", now)).toMatchObject([{ time: null, withdrawn: true }]);
    mirror.signal.occurredAt = "2026-09-08T19:30:25Z";
    expect(resetCalendarRecords([], [first, mirror], "zh", now)).toMatchObject([{ time: null, reviewed: false, conflictingTimes: true }]);
  });

  it("excludes personal observations, policy/context, negative signals and future claimed occurrences", () => {
    const personal = lead("personal"); personal.signal.scope = "personal";
    const ui = lead("ui"); ui.signal.source.sourceClass = "provider_ui_observation";
    const negative = lead("negative"); negative.disposition = "negative";
    const policy = lead("policy", "quota_policy_change");
    expect(resetCalendarRecords([], [personal, ui, negative, policy], "zh", now)).toEqual([]);
    const future = lead("future"); future.signal.occurredAt = "2026-12-31T00:00:00Z";
    expect(resetCalendarRecords([], [future], "zh", now)[0]).toMatchObject({ mode: "announcements", reviewed: false });
  });

  it("uses the operating-system time zone for day boundaries and exact displayed timestamps", () => {
    const time = exactPublicTime("2026-09-07T19:30:25Z")!;
    const equivalent = exactPublicTime("2026-09-08T03:30:25+08:00")!;
    expect(time).toBe(equivalent);
    const expectedDay = process.env.TZ === "America/New_York" ? "2026-09-07" : process.env.TZ === "Asia/Shanghai" ? "2026-09-08" : localCalendarDate(new Date(time));
    expect(localCalendarDate(new Date(time))).toBe(expectedDay);
    const label = calendarEventTime(time, "en");
    expect(label).toContain(process.env.TZ === "America/New_York" ? "GMT-04:00" : process.env.TZ === "Asia/Shanghai" ? "GMT+08:00" : "GMT");
    expect(label).toContain(":30:25");
  });

  it("walks three local calendar months across year changes, leap days and DST without duplicated dates", () => {
    const january = resetCalendarMonths(new Date(2026, 0, 4, 12).getTime());
    expect(january.map((month) => month.key)).toEqual(["2025-11", "2025-12", "2026-01"]);
    const spring = resetCalendarMonths(new Date(2024, 2, 20, 12).getTime());
    const days = spring.flatMap((month) => month.days.filter((day) => day != null));
    expect(days).toHaveLength(91);
    expect(new Set(days.map((day) => day.key)).size).toBe(91);
    expect(days.some((day) => day.key === "2024-02-29")).toBe(true);
    expect(days.find((day) => day.key === "2024-03-20")?.future).toBe(false);
    expect(days.find((day) => day.key === "2024-03-21")?.future).toBe(true);
    const before = calendarEventTime(Date.parse("2026-03-08T06:59:00Z"), "en");
    const after = calendarEventTime(Date.parse("2026-03-08T07:01:00Z"), "en");
    if (process.env.TZ === "America/New_York") { expect(before).toContain("GMT-05:00"); expect(after).toContain("GMT-04:00"); }
  });

  it("renders both kinds on a shared day and exposes accessible labels without raw codes or a live clock", () => {
    const primary = reviewed(); const grant = lead("grant", "global_banked_reset_grant");
    const html = renderToStaticMarkup(<ResetCalendar archive={archive([primary])} timeline={{ sources: [], entries: [grant], revisionCount: 1, evidenceFamilyCount: 1 }} now={now} language="zh" />);
    expect(html).toContain('data-activity="both"');
    expect(html).toContain("强制重置记录 / 重置卡发放");
    expect(html).toContain("发生时间已核实");
    expect(html).toContain("追踪站报告时间");
    expect(html).toContain(calendarEventTime(Date.parse(primary.signal.occurredAt!), "zh"));
    expect(html).toContain("本地时区");
    expect(html).not.toContain("电脑当前时间");
    for (const value of ["signal_", "family_", "internal_parser", "tracker_confirmation", "confirmed_reset", "global_banked_reset_grant", "100%", "0%"]) expect(html).not.toContain(value);
    expect(html).toContain("未收录记录");
  });

  it("keeps old and undated records out of three-month counts and bounds initial day detail rendering", () => {
    const entries = Array.from({ length: 20 }, (_, index) => lead(String(index)));
    const old = lead("old"); old.signal.occurredAt = "2026-01-01T00:00:00Z";
    const undated = lead("undated"); undated.signal.occurredAt = null; undated.signal.source.publishedAt = null;
    const html = renderToStaticMarkup(<ResetCalendar archive={null} timeline={{ sources: [], entries: [...entries, old, undated], revisionCount: 22, evidenceFamilyCount: 22 }} now={now} language="zh" />);
    expect(html).toContain("20 条去重记录");
    expect(html).toContain("还有 14 条");
    expect(html.match(/<time/g)).toHaveLength(7);
    expect(html).toContain("1 条日期待核对的资料");
    expect(html).not.toContain("2026-01-01");
  });
});
