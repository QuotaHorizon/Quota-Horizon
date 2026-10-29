import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { CommunityInputStatus } from "./CommunityInputStatus";
import type { CommunityDiagnostic } from "./quantitativeTypes";

const now = Date.parse("2026-10-30T01:00:00Z");
const diagnosis = (): CommunityDiagnostic => ({ id: "date_poll", target: "broad_quota_relief", state: "excluded",
  reason: "insufficient_recent_votes", observed_at: now/1000, generated_at: now/1000-60, read_failed: false,
  recent_votes: 18, total_votes: 1018, minimum_recent_votes: 50, observation_window_hours: 6,
  latest_vote_at: now/1000-300, window_start: now/1000-3600, window_end: now/1000+86400,
  question_kind: "interval_votes", unit: "votes" });
describe("ballot admission details", () => {
  it.each(["zh", "en"] as const)("shows recent counts and original scope without turning totals into sample size in %s", language => {
    const forecast = quantitativeFixture(now).forecast!.forecasts[0];
    forecast.community_diagnostics = [diagnosis()];
    const html = renderToStaticMarkup(<CommunityInputStatus forecast={forecast} language={language} />);
    expect(html).toContain("<b>18</b> / 50");
    expect(html).toContain("1018");
    expect(html).toContain(language === "zh" ? "近期样本不足" : "Recent sample too small");
    expect(html).toContain(language === "zh" ? "重置时间预期" : "Expected reset timing");
    expect(html).toContain(language === "zh" ? "原期限" : "Original window");
    expect(html).not.toContain("insufficient_recent_votes");
  });
  it("retains failed reads and unknown counts instead of displaying zero", () => {
    const forecast = quantitativeFixture(now).forecast!.forecasts[0];
    forecast.community_diagnostics = [{...diagnosis(), recent_votes:null, reason:"observation_stale", read_failed:true}];
    const html = renderToStaticMarkup(<CommunityInputStatus forecast={forecast} language="zh" />);
    expect(html).toContain("<b>—</b> / 50");
    expect(html).toContain("采样已过期");
    expect(html).toContain("最近读取失败");
  });
  it("keeps admitted or different-scope polls out of the exclusion list and handles old exports", () => {
    const forecast = quantitativeFixture(now).forecast!.forecasts[0];
    const render = () => renderToStaticMarkup(<CommunityInputStatus forecast={forecast} language="en" />);
    expect(render()).toBe("");
    forecast.community_diagnostics = [{...diagnosis(), state:"included", reason:null}, {...diagnosis(), target:"automatic_reset"}];
    expect(render()).toBe("");
  });
});
