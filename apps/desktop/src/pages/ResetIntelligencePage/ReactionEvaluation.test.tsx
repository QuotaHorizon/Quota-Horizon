import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { ReactionEvaluation } from "./ReactionEvaluation";

const now = Date.parse("2026-10-24T01:00:00Z");
describe("event response evaluation", () => {
  it.each(["zh", "en"] as const)("shows measured response and missing clocks separately in %s", language => {
    const view = quantitativeFixture(now);
    view.presentation = { issuedAt: new Date(now).toISOString(), receivedAt: now + 10, presentedAt: now + 30, visible: false };
    view.evaluation!.reaction = { version: "reset-reaction-1", events: [{
      id: "event", kind: "automatic", status: "confirmed", time_basis: "observed_interval",
      completed_from: now / 1000 - 300, completed_to: now / 1000 - 240, first_received_at: now / 1000 - 180,
      evidence: [], series: [{ source: "horizon", version: "v12", target: "automatic_reset", track: "model",
        state: "updated", published_at: null, source_received_at: null, observer_received_at: now / 1000,
        publication_to_capture_seconds: null, completion_to_update_seconds: 240, known_to_update_seconds: 180,
        old_cycle_residual_seconds: 120, residual_censored: false, before_probability_24h: .93, after_probability_24h: .81,
        ui_received_at: null, ui_delay_seconds: null,
      }] }] };
    const html = renderToStaticMarkup(<ReactionEvaluation view={view} language={language} />);
    expect(html).toContain("20 ms");
    expect(html).toContain(language === "zh" ? "后台页面" : "background page");
    expect(html).toContain(language === "zh" ? "已更新" : "Updated");
    expect(html).toContain(language === "zh" ? "3.0 分钟" : "3.0 min");
    expect(html).toContain("93.0% → 81.0%");
    expect(html).toContain("—");
    expect(html).not.toContain("observed_interval");
    view.evaluation!.reaction.events[0].series[0].state = "awaiting_update";
    view.evaluation!.reaction.events[0].series[0].residual_censored = true;
    const waiting = renderToStaticMarkup(<ReactionEvaluation view={view} language={language} />);
    expect(waiting).toContain(language === "zh" ? "待更新" : "Awaiting update");
    expect(waiting).toContain("≥ ");
  });
  it("keeps legacy reports free of fabricated response measurements", () => {
    expect(renderToStaticMarkup(<ReactionEvaluation view={quantitativeFixture(now)} language="zh" />)).toBe("");
  });
});
