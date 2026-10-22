import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { CalendarTrend } from "./CalendarTrend";
import type { CalendarTrend as Trend } from "./quantitativeTypes";

const now = 1792677600;
const fixture = (): Trend => ({ window_hours: 6, bin_hours: 1, minimum_bin_votes: 50, minimum_coverage: .9,
  bins: Array.from({ length: 6 }, (_, i) => ({ start: now - (6 - i) * 3600, end: now - (5 - i) * 3600,
    new_clicks: 100, reset_clicks: i === 5 ? 66 : 95, feature_clicks: i === 5 ? 34 : 5,
    coverage_fraction: .98, reset_share: i === 5 ? .66 : .95, clicks_per_hour: 100 / .98, state: "available" })),
  change: { state: "available", reset_share_delta: -.29, click_rate_ratio: 1 },
});
describe("observed hourly sentiment", () => {
  it.each(["zh", "en"] as const)("shows change and click volume without turning preference into a forecast in %s", language => {
    const html = renderToStaticMarkup(<CalendarTrend trend={fixture()} language={language} />);
    expect(html).toContain("66.0%"); expect(html).toContain("-29.0");
    expect(html.match(/<path /g)).toHaveLength(5);
    expect(html.match(/<rect /g)).toHaveLength(6);
    expect(html).toContain(language === "zh" ? "点击速度 1.00 倍" : "Click rate 1.00×");
    expect(html).not.toContain("24h"); expect(html).not.toContain("participants");
  });
  it.each(["sample", "coverage"])("keeps an ineligible %s bin as a gap and hides the comparison", issue => {
    const trend = fixture();
    if (issue === "sample") trend.bins[4].new_clicks = 49;
    else trend.bins[4].coverage_fraction = .89;
    const html = renderToStaticMarkup(<CalendarTrend trend={trend} language="en" />);
    expect(html.match(/<path /g)).toHaveLength(3);
    expect(html).not.toContain("-29.0");
    expect(html).toContain("Each hour needs at least 50 new clicks");
    expect(html.match(/<rect /g)).toHaveLength(6);
  });
  it("accepts old reports without hourly data", () => {
    expect(renderToStaticMarkup(<CalendarTrend language="en" />)).toBe("");
  });
});
