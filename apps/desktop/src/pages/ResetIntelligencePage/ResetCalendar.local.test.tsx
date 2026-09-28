import { execFileSync } from "node:child_process";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { ResetCalendar } from "./ResetCalendar";
import { localCalendarDate, resetCalendarMonths, resetCalendarRecords } from "./resetCalendarModel";
import type { PublicTimelineEntry } from "./types";

it.skipIf(!process.env.HORIZON_PUBLIC_CALENDAR_SMOKE)("renders the existing public-only ledger without reading account/history/keychain data", () => {
  const path = process.env.HORIZON_PUBLIC_CALENDAR_SMOKE!;
  const rows = execFileSync("/usr/bin/sqlite3", ["-readonly", path,
    "SELECT signal_json FROM public_signals s WHERE revision = (SELECT MAX(revision) FROM public_signals latest WHERE latest.signal_id = s.signal_id) LIMIT 16384"],
    { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 }).trim();
  const entries: PublicTimelineEntry[] = rows ? rows.split("\n").map((row) => ({ signal: JSON.parse(row), disposition: "needs_review" })) : [];
  const now = Date.now();
  const records = resetCalendarRecords([], entries, "zh", now);
  const firstDay = resetCalendarMonths(now)[0].days.find((day) => day)?.key!;
  const inRange = records.filter((record) => record.time != null && localCalendarDate(new Date(record.time)) >= firstDay && record.time <= now);
  const html = renderToStaticMarkup(<ResetCalendar archive={null} timeline={{ sources: [], entries, revisionCount: entries.length,
    evidenceFamilyCount: new Set(entries.map((entry) => entry.signal.evidenceFamilyId)).size }} language="zh" now={now} />);
  expect(html).toContain("重置日期热图");
  expect(html).not.toContain("NaN");
  expect(html).not.toContain("Invalid Date");
  expect(records.every((record) => !record.reviewed)).toBe(true);
  // Re-render the real public cache without modifying account or history data.
  const grantUrl = "https://x.com/thsottiaux/status/2102463847714247142";
  if (entries.some((entry) => entry.signal.source.canonicalUrl === grantUrl)) {
    expect(records.filter((record) => record.url === grantUrl)).toMatchObject([
      { kind: "grant", mode: "events", timeBasis: "grant_announcement", time: Date.parse("2026-09-22T18:23:37Z") },
    ]);
    expect(html).toContain("已宣布开始发放");
  }
  console.info(JSON.stringify({ publicRecords: entries.length, deduplicated: records.length,
    occurrenceRecordsInRange: inRange.filter((record) => record.mode === "events").length,
    announcementRecordsInRange: inRange.filter((record) => record.mode === "announcements").length,
    resetDays: new Set(inRange.filter((record) => record.mode === "events" && record.kind === "reset").map((record) => localCalendarDate(new Date(record.time!)))).size,
    grantDays: new Set(inRange.filter((record) => record.mode === "events" && record.kind === "grant").map((record) => localCalendarDate(new Date(record.time!)))).size,
    missingOrDisputed: records.filter((record) => record.time == null).length }));
});
