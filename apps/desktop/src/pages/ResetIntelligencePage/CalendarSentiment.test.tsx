import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { CalendarSentiment } from "./CalendarSentiment";
import type { QuantReport } from "./quantitativeTypes";

const now = Date.parse("2026-10-22T14:00:00Z");
function report(): QuantReport {
  return { ...quantitativeFixture(now).evaluation!, calendar_sentiment: {
    production_weight: 0, frozen_days: 5, frozen_trials: 12, scored_days: 2, scored_trials: 4,
    latest: [{ source: "didcodex_poll", date: "2026-10-22",
      window_start: Date.parse("2026-10-22T07:00:00Z") / 1000,
      window_end: Date.parse("2026-10-23T07:00:00Z") / 1000,
      observed_at: now / 1000 - 30, latest_vote_at: now / 1000 - 90,
      recent_votes: 2529, total_clicks: 12000, weighted_reset_share: .873, reset_share: .841,
      state: "ready_for_evaluation" }],
  } };
}

describe("calendar-day community sentiment", () => {
  it.each(["zh", "en"] as const)("shows an admitted consensus without describing it as only a change in %s", language => {
    const html = renderToStaticMarkup(<CalendarSentiment report={report()} language={language} now={now} input={{
      id: "didcodex_poll", weight: .78, votes: 2529, effective_votes: null, coverage_hours: 17,
      probability_24h: .92, probability_48h: .96, mapping_kind: "sentiment_level",
      mapping_version: "calendar-consensus-2", terminal_probability: .9,
    }} />);
    expect(html).toContain(language === "zh" ? "社区倾向已计入" : "Community sentiment included");
    expect(html).toContain(language === "zh" ? "综合判断权重 78.0%" : "Weight in the combined forecast 78.0%");
    expect(html).toContain("87.3%");
    expect(html).not.toContain(language === "zh" ? "近期倾向变化已计入" : "Recent sentiment change included");
    const visible = html.slice(0, html.indexOf("<details"));
    expect(visible).toContain("87.3%");
    expect(visible).toContain("2,529");
    expect(visible).not.toContain("84.1%");
    expect(visible).not.toContain("78.0%");
    expect(html).not.toMatch(/<details[^>]* open/);
  });
  it("shows the applied innovation weight separately from the raw preference research candidate", () => {
    const html = renderToStaticMarkup(<CalendarSentiment report={report()} language="zh" now={now} input={{
      id: "didcodex_poll", weight: .24, votes: 2529, effective_votes: null, coverage_hours: 17,
      probability_24h: .65, probability_48h: .8, mapping_kind: "sentiment_innovation",
    }} />);
    expect(html).toContain("近期倾向变化已计入");
    expect(html).toContain("综合判断权重 24.0%");
    expect(html).toContain("原始倾向 84.1%");
    expect(html).not.toContain("权重 0.0%");
  });
  it.each(["zh", "en"] as const)("shows click preferences and independent day counts in %s", language => {
    const html = renderToStaticMarkup(<CalendarSentiment report={report()} language={language} now={now} />);
    expect(html).toContain("Did Codex Reset");
    expect(html).toContain("2026-10-22");
    expect(html).toContain("2,529");
    expect(html).toContain("12,000");
    expect(html).toContain("87.3%");
    expect(html).toContain("84.1%");
    expect(html).toContain(language === "zh" ? "太平洋时间" : "Pacific time");
    expect(html).toContain(language === "zh" ? "活动日候选 · 独立评估中" : "Calendar-day candidate · independent evaluation");
    expect(html).toContain(language === "zh" ? "综合判断权重 0.0%" : "Weight in the combined forecast 0.0%");
    expect(html).toContain(language === "zh" ? "已冻结 5 日 / 12 个时点 · 已评估 2 日 / 4 个时点" : "Frozen: 5 days / 12 checkpoints · Scored: 2 days / 4 checkpoints");
    expect(html).not.toContain("24h");
    expect(html).not.toContain("48h");
    expect(html).not.toContain("2529 participants");
  });

  it.each(["expired", "future"])("does not present %s activity-day samples as current", timing => {
    const value = report(), row = value.calendar_sentiment!.latest[0];
    if (timing === "expired") row.window_end = now / 1000;
    else row.window_start = now / 1000 + 1;
    const html = renderToStaticMarkup(<CalendarSentiment report={value} language="zh" now={now} />);
    expect(html).toContain("当前活动日等待新数据");
    expect(html).not.toContain("87.3%");
    expect(html).not.toContain("2,529");
    expect(html).toContain("已评估 2 日 / 4 个时点");
  });

  it.each(["sample", "report", "state"])("labels a stale %s while keeping saved values readable", origin => {
    const value = report();
    if (origin === "sample") value.calendar_sentiment!.latest[0].observed_at -= 600;
    else if (origin === "report") value.generated_at = new Date(now - 600_000).toISOString();
    else value.calendar_sentiment!.latest[0].state = "stale";
    const html = renderToStaticMarkup(<CalendarSentiment report={value} language="en" now={now} />);
    expect(html).toContain("Collection update due · saved observation");
    expect(html).toContain("87.3%");
    expect(html).not.toContain("Calendar-day candidate · independent evaluation");
    expect(html.indexOf("Collection update due")).toBeLessThan(html.indexOf("<details"));
  });

  it("retains a small click count without inventing a probability or unique participants", () => {
    const value = report(), row = value.calendar_sentiment!.latest[0];
    row.recent_votes = 49; row.state = "insufficient_recent_votes";
    row.weighted_reset_share = null; row.reset_share = null;
    const html = renderToStaticMarkup(<CalendarSentiment report={value} language="zh" now={now} />);
    expect(html).toContain("近 6h 新增 49 次点击");
    expect(html).toContain("样本待达标 · 近 6h 需新增至少 50 次点击");
    expect(html).not.toContain("87.3%");
    expect(html).not.toContain("49 人");
    expect(html.indexOf("样本待达标")).toBeLessThan(html.indexOf("<details"));
  });

  it("keeps legacy reports unchanged when the optional calendar section is absent", () => {
    const legacy = report();
    delete legacy.calendar_sentiment;
    for (const value of [null, legacy]) {
      expect(renderToStaticMarkup(<CalendarSentiment report={value} language="en" now={now} />)).toBe("");
    }
  });
});
