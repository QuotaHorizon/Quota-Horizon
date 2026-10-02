import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { UpcomingResetNotice } from "../../pages/ResetIntelligencePage/upcomingResetModel";
import { ResetNoticeChip } from "./ResetNoticeChip";

const now = new Date(2026, 8, 12, 14, 30).getTime();
const notice: UpcomingResetNotice = { key: "hidden_family", kind: "reset", excerpt: "Synthetic announcement", url: "https://x.com/thsottiaux/status/123",
  publishedAt: now - 3_600_000, collectedVia: [], trackerCount: 2, state: "upcoming", stale: false, conflictingTiming: false,
  timing: { start: now - 3_600_000, end: new Date(2026, 8, 12, 15).getTime(), timeZone: "America/Los_Angeles", sourceUrl: "https://quotaresets.com/api/v1/events.json", deadline: true, explicit: false },
  explicit: false, cohort: null };
const render = (value = notice, failed = false) => renderToStaticMarkup(<ResetNoticeChip notice={value} now={now} language="zh" failed={failed} onOpen={() => undefined} />);

describe("menu reset notice", () => {
  it("shows a local-time reference deadline without claiming exact receipt", () => {
    const html = render();
    expect(html).toContain("重置预告"); expect(html).toContain("预计 15:00 前");
    expect(html).toContain("追踪站预计期限"); expect(html).toContain("查看详情");
    expect(html).not.toContain("hidden_family"); expect(html).not.toContain("100%");
  });
  it("keeps elapsed, corrected and follow-up states distinct and removes the deadline", () => {
    for (const [state, label] of [["elapsed", "预告待确认"], ["withdrawn", "预告有更正"], ["reported", "重置新动态"], ["announced_delivery", "发卡新动态"]] as const) {
      const html = render({ ...notice, state });
      expect(html).toContain(label); expect(html).not.toContain("预计 15:00");
    }
  });
  it("marks stale/unknown time and preserves a retry action on navigation failure", () => {
    const html = render({ ...notice, stale: true, timing: null });
    expect(html).toContain("含缓存"); expect(html).not.toContain("15:00");
    expect(render(notice, true)).toContain("未能打开 · 重试");
    const range = render({ ...notice, timing: { ...notice.timing!, deadline: false } });
    expect(range).not.toContain("预计 15:00");
  });
  it("supports grants and a dated next-day time without changing quota", () => {
    const html = renderToStaticMarkup(<ResetNoticeChip notice={{ ...notice, kind: "grant", timing: { ...notice.timing!, end: new Date(2026, 8, 13, 15).getTime() } }} now={now} language="en" onOpen={() => undefined} />);
    expect(html).toContain("Grant announced"); expect(html).toContain("09/13");
    expect(html).not.toContain("hidden_family");
  });
  it("puts an explicit timed reset commitment ahead of the third-party baseline", () => {
    const html = render({ ...notice, explicit: true, cohort: "all paid ChatGPT accounts",
      timing: { ...notice.timing!, explicit: true } });
    expect(html).toContain("Horizon 当前判断 24h 100%");
    expect(html).toContain("预告时间");
    expect(html).toContain("all paid ChatGPT accounts");
    expect(html).not.toContain("第三方 24h");
  });
  it("keeps the menu's window and expiry rules consistent with the outlook", () => {
    const explicit = { ...notice, explicit: true, timing: { ...notice.timing!, explicit: true } };
    expect(render({ ...explicit, timing: { ...explicit.timing, end: now + 30 * 3_600_000 } })).toContain("48h 100%");
    expect(render({ ...explicit, timing: { ...explicit.timing, end: now + 49 * 3_600_000 } })).not.toContain("100%");
    expect(render({ ...explicit, timing: { ...explicit.timing, end: now } })).not.toContain("100%");
    expect(render({ ...explicit, stale: true })).not.toContain("100%");
    expect(render({ ...explicit, state: "withdrawn" })).not.toContain("100%");
  });
});
