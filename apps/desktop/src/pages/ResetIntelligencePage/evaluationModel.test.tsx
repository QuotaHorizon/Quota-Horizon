import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { currentHorizonProgress, evaluationPage, evaluationRows, evaluationVersion } from "./evaluationModel";
import { QuantitativeEvaluation } from "./QuantitativeEvaluation";

const now = Date.parse("2026-10-22T14:00:00Z");
describe("version-specific evaluation", () => {
  it("keeps evaluation results reachable while leaving update failures visible outside the disclosure", () => {
    const view = quantitativeFixture(now, true);
    view.evaluationIssue = "read_failed";
    const html = renderToStaticMarkup(<QuantitativeEvaluation view={view} now={now} language="en" />);
    const detailStart = html.indexOf("<details");
    expect(detailStart).toBeGreaterThan(0);
    expect(html.slice(0, detailStart)).toContain("saved report");
    expect(html.slice(0, detailStart)).not.toContain("Probability calibration");
    expect(html.slice(detailStart)).toContain("Probability calibration");
    expect(html.slice(detailStart)).toContain("0.184");
    expect(html).not.toMatch(/<details[^>]* open/);
  });
  it("keeps old model scores in history while a new model waits for its own windows", () => {
    const view = quantitativeFixture(now, true), report = view.evaluation!;
    report.scorecards = report.scorecards.filter(r => r.track === "model");
    const previous = report.scorecards[0];
    const version = "reset-intelligence-source-signals-10:config-v2";
    view.forecast!.forecasts.forEach(f => { f.model_version = "reset-intelligence-source-signals-10"; });
    report.latest = report.latest.filter(r => r.track === "model").map(r => ({ ...r, version }));
    const pending = { ...previous, version, scores: { ...previous.scores, n: 0, brier: null, log_loss: null, calibration: [] } };
    report.scorecards.push(pending);
    expect(evaluationRows(view, false).cards).toEqual([pending]);
    expect(evaluationRows(view, true).cards).toContain(previous);
    const html = renderToStaticMarkup(<QuantitativeEvaluation view={view} now={now} language="en" />);
    expect(html).toContain("Awaiting complete scoring windows for this version");
    expect(html).toContain("v10 · config-v");
    expect(html).not.toContain("Probability calibration");
    expect(html).not.toContain("0.184");
  });
  it("does not borrow results when the producer version is ahead of the report", () => {
    const view = quantitativeFixture(now, true);
    view.forecast!.forecasts.forEach(f => { f.model_version = "new-version"; });
    expect(evaluationRows(view, false).cards.every(r => r.track === "native")).toBe(true);
    expect(evaluationRows(view, true).cards.some(r => r.track === "model")).toBe(true);
  });
  it("waits for the exact configuration even when the model version is unchanged", () => {
    const view = quantitativeFixture(now, true);
    view.forecast!.forecasts.forEach(f => { f.evaluation_version = `${f.model_version}:new-config`; });
    expect(evaluationRows(view, false).cards.every(r => r.track === "native")).toBe(true);
    expect(evaluationRows(view, true).cards.some(r => r.track === "model")).toBe(true);
  });
  it("selects the newest exact source/config identity without hiding older scored versions", () => {
    const view = quantitativeFixture(now, true), report = view.evaluation!;
    const old = report.latest[0], version = "synthetic-model:new-config";
    report.latest.push({ ...old, version, origin: new Date(now).toISOString() });
    const fresh = { ...report.scorecards[0], version };
    report.scorecards.push(fresh);
    const rows = evaluationRows(view, false, old.source).cards;
    expect(rows).toContain(fresh);
    expect(rows.some(r => r.target === old.target && r.version === old.version)).toBe(false);
    expect(evaluationRows(view, true).cards).toContain(report.scorecards[0]);
  });
  it("keeps only matched nonzero comparisons and applies the same version/source selection", () => {
    const view = quantitativeFixture(now, true), report = view.evaluation!, pair = report.comparisons[0];
    report.comparisons.push({ ...pair, n: 0 }, { ...pair, left: { ...pair.left, version: "obsolete" } });
    expect(evaluationRows(view, false).comparisons).toEqual([pair]);
    expect(evaluationRows(view, true).comparisons).toHaveLength(2);
    expect(evaluationRows(view, false, "monitor").comparisons).toEqual([]);
  });
  it("bounds the rendered table and clamps pages after reports or filters change", () => {
    const rows = Array.from({ length: 821 }, (_, i) => i);
    expect(evaluationPage(rows, 0).items).toHaveLength(20);
    expect(evaluationPage(rows, 999)).toEqual({ items: [820], page: 41, pages: 42 });
    expect(evaluationPage([], 5)).toEqual({ items: [], page: 0, pages: 1 });
    expect(evaluationVersion("reset-intelligence-source-signals-10:0123456789abc")).toBe("v10 · 01234567");
    expect(evaluationVersion("native-source-v3")).toBe("native-source-v3");
  });
});

describe("current Horizon evidence", () => {
  it("keeps the four target/horizon counts separate from all-history score totals", () => {
    const view = quantitativeFixture(now, true);
    view.evaluation!.summary.scored_nonoverlapping_trials = 99999;
    const cards = view.evaluation!.scorecards.filter(card => card.source === "horizon" && card.track === "model");
    cards.forEach((card, index) => { card.scores = { ...card.scores, n: index + 1, unique_events: 1 }; });
    const progress = currentHorizonProgress(view);
    expect(progress).toHaveLength(4);
    for (const item of progress) expect(item.card).toBe(cards.find(card => card.target === item.target && card.hours === item.hours));
    const html = renderToStaticMarkup(<QuantitativeEvaluation view={view} language="en" now={now} />);
    expect(html.indexOf("Current Horizon scored windows")).toBeLessThan(html.indexOf("<details"));
    expect(html.indexOf("99999")).toBeGreaterThan(html.indexOf("<details"));
    expect(html.match(/1 distinct events/g)).toHaveLength(4);
  });

  it("does not attribute the retired model's results to a forecast awaiting its own report", () => {
    const view = quantitativeFixture(now, true);
    view.forecast!.forecasts.forEach(forecast => { forecast.evaluation_version = "new-version:identity"; });
    expect(currentHorizonProgress(view).every(item => item.card === null)).toBe(true);
    const html = renderToStaticMarkup(<QuantitativeEvaluation view={view} language="zh" now={now} />);
    expect(html).toContain("等待该版本的评估记录");
  });

  it("distinguishes no scored windows from an absent report and keeps evidence gaps visible", () => {
    const view = quantitativeFixture(now);
    const first = view.evaluation!.scorecards.find(card => card.source === "horizon" && card.target === "broad_quota_relief" && card.hours === 24)!;
    first.settlement_states = { pending: 5, unresolved: 2 };
    expect(currentHorizonProgress(view)[0].card?.scores.n).toBe(0);
    const html = renderToStaticMarkup(<QuantitativeEvaluation view={view} language="en" now={now} />);
    expect(html).toContain("Some windows await outcome evidence");
    expect(html).toContain("Awaiting window maturity");
    expect(html).not.toContain("Awaiting this version’s evaluation");
  });
});
