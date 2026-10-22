import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { evaluationPage, evaluationRows, evaluationVersion } from "./evaluationModel";
import { QuantitativeEvaluation } from "./QuantitativeEvaluation";

const now = Date.parse("2026-10-22T14:00:00Z");
describe("version-specific evaluation", () => {
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
