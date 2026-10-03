import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { RadarOverview } from "./RadarOverview";
import { historyDelta, evidenceGroups, currentOpinions } from "./radarOverviewModel";
import { forecastPresentation } from "./resetBriefingModel";
import { ResetNoticeChip } from "../../components/CapacityPopover/ResetNoticeChip";
import type { PublicResetTimeline, RadarView, RadarOpinion } from "./types";
const now = Date.parse("2026-10-03T08:00:00Z");
const stamp = new Date(now).toISOString();
const source = { id: "codex_reset", name: "Codex Reset", url: "https://codex-reset.com", method: "cadence" as const,
  probability24h: 20, probability48h: 40, baseline24h: null, baseline48h: null, updatedAt: stamp, collectedAt: stamp,
  lastResetAt: null, evidenceUrls: ["https://x.com/thsottiaux/status/123"], usesCommunity: false, issue: null, exclusion: null, weight: 1, history: [] };
function radar(): RadarView {
  return { updatedAt: stamp, forecasts: [source], history: [{ at: stamp, probability24h: 32, probability48h: 52 }],
    estimate: { probability24h: 32, probability48h: 52, pooled24h: 30, pooled48h: 50, communityAdjustment24h: 2, communityAdjustment48h: 2,
      sourceCount: 1, spread24h: 0, spread48h: 0, evidenceGroups: 2, modelVersion: "test-v1" },
    community: { channels: [], opinions: [], optimistic: 3, uncertain: 1, pessimistic: 0, wishes: 0, observations: 0,
      authors: 4, communities: 1, independentAuthors: 3, duplicates: 2, optimisticShare: 75, previousShare: 50 },
  };
}
function timeline(r = radar()): PublicResetTimeline {
  return { radar: r, sources: [], entries: [], revisionCount: 0, evidenceFamilyCount: 0 };
}
describe("combined radar overview", () => {
  it("uses the same Horizon estimate in the overview and menu while preserving source values", () => {
    const r = radar(); const t = timeline(r);
    const html = renderToStaticMarkup(<RadarOverview timeline={t} radar={r} now={now} language="en" />);
    expect(html).toContain("Horizon combined estimate"); expect(html).toContain("How sources compare");
    expect(html).toContain("Community outlook"); expect(html).toContain("20%"); expect(html).toContain("75%");
    expect(forecastPresentation(undefined, now, false, t).probability24h).toBe(32);
    const menu = renderToStaticMarkup(<ResetNoticeChip timeline={t} now={now} language="en" onOpen={() => {}} />);
    expect(menu).toContain("Horizon 24h 32%"); expect(menu).not.toContain("Codex Reset · Horizon");
  });
  it("keeps insufficient community sampling separate from a zero optimistic share", () => {
    const r = radar(); Object.assign(r.community, { optimistic: 0, uncertain: 0, pessimistic: 0, optimisticShare: null, authors: 0, independentAuthors: 0 });
    const html = renderToStaticMarkup(<RadarOverview timeline={timeline(r)} radar={r} now={now} language="en" />);
    expect(html).toContain("No explicit near-term predictions sampled"); expect(html).not.toContain("Optimistic 0%");
  });
  it("only reports a six-hour change when that observation actually exists", () => {
    const points = radar().history; expect(historyDelta(points, now, 24)).toBeNull();
    points.unshift({ at: new Date(now - 6 * 3600_000).toISOString(), probability24h: 20, probability48h: 30 });
    expect(historyDelta(points, now, 24)).toBe(12); expect(historyDelta(points, now, 48)).toBe(22);
    expect(historyDelta(points, now + 3600_000, 24)).toBeNull();
  });
  it("groups shared original statements once and keeps undisclosed methods explicit", () => {
    const groups = evidenceGroups([source, { ...source, id: "copy", name: "Copy", method: "mixed" }]);
    expect(groups.find(g => g.key.includes("/123"))?.sources).toEqual(["Codex Reset", "Copy"]);
    expect(groups.filter(g => g.key === "history")).toHaveLength(1);
    expect(groups.some(g => g.key === "undisclosed:copy")).toBe(true);
  });
  it("sample details show latest author statements and collapse exact copies", () => {
    const p: RadarOpinion = { id: "a", channel: "github", author: "one", url: "https://github.com/openai/codex/issues/1", text: "Codex will reset today", publishedAt: stamp,
      stance: "optimistic", reason: "explicit_prediction", evidenceUrls: [], horizonHours: 24 };
    const samples = currentOpinions([p, { ...p, id: "b", author: "two" }, { ...p, id: "c", publishedAt: new Date(now - 7*3600_000).toISOString() }], now);
    expect(samples).toHaveLength(1);
  });
  it("does not fall back to a third-party probability when a composite calculation is unavailable", () => {
    const r = radar(); r.estimate = null;
    expect(forecastPresentation(undefined, now, false, timeline(r)).probability24h).toBeNull();
  });
});
