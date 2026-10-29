import type { ForecastIdentity, QuantitativeView, QuantScorecard, ResetTarget } from "./quantitativeTypes";
import { sameForecastVersion } from "./quantitativeModel";

const controls = new Set(["survival_baseline", "without_community", "without_response_cohort", "without_comment_estimates", "without_information", "without_radars", "without_baseline", "equal_components"]);
export const evaluationIdentity = (r: ForecastIdentity, target: ResetTarget) => JSON.stringify([r.source, r.version, r.track, target]);
export const scorecardKey = (r: QuantScorecard) => `${evaluationIdentity(r, r.target)}:${r.hours}`;
export const evaluationVersion = (version: string) => version.replace(/^reset-intelligence-source-signals-(\d+):(.+)$/, (_, n, hash) => `v${n} · ${hash.slice(0, 8)}`);

/** A new model must wait for its own results, even while old versions have scores. */
export function evaluationRows(view: QuantitativeView, history: boolean, source = "") {
  const report = view.evaluation;
  if (!report) return { cards: [], comparisons: [], sources: [] };
  const latest = new Map<string, typeof report.latest[number]>();
  for (const row of report.latest) {
    if (row.track === "model" || row.track === "shadow") {
      const forecast = view.forecast?.forecasts.find(f => f.target === row.target);
      if (forecast && !sameForecastVersion(row.version, forecast)) continue;
    }
    const group = JSON.stringify([row.source, row.track, row.target]);
    const previous = latest.get(group);
    if (!previous || Date.parse(row.origin) > Date.parse(previous.origin)
      || (row.origin === previous.origin && Date.parse(row.received_at) > Date.parse(previous.received_at))) latest.set(group, row);
  }
  const current = new Set([...latest.values()].map(r => evaluationIdentity(r, r.target)));
  const eligible = (r: ForecastIdentity) => r.track === "model" || r.track === "native" || controls.has(r.source);
  const admitted = (r: ForecastIdentity, target: ResetTarget) => eligible(r) && (history || current.has(evaluationIdentity(r, target)));
  const all = report.scorecards.filter(r => admitted(r, r.target) && (history || [24, 48].includes(r.hours))
    && (!history || r.scores.n > 0 || (r.observed_event_scores?.n ?? 0) > 0));
  const order = (r: ForecastIdentity) => r.track === "model" ? 0 : r.track === "native" ? 1 : 2;
  const cards = all.filter(r => !source || r.source === source).sort((a, b) => order(a) - order(b)
    || a.source.localeCompare(b.source) || b.version.localeCompare(a.version, undefined, { numeric: true })
    || b.target.localeCompare(a.target) || a.hours - b.hours);
  const comparisons = report.comparisons.filter(c => c.n > 0 && (history || [24, 48].includes(c.hours))
    && admitted(c.left, c.target) && admitted(c.right, c.target)
    && (!source || c.left.source === source || c.right.source === source));
  return { cards, comparisons, sources: [...new Set(all.map(r => r.source))].sort() };
}

export function evaluationPage<T>(items: T[], requested: number, size = 20) {
  const pages = Math.max(1, Math.ceil(items.length / size));
  const page = Math.min(Math.max(0, requested), pages - 1);
  return { items: items.slice(page * size, (page + 1) * size), page, pages };
}

/** Keep targets and horizons separate: their windows and events overlap. */
export function currentHorizonProgress(view: QuantitativeView) {
  const cards = evaluationRows(view, false, "horizon").cards.filter(card => card.track === "model");
  return (["broad_quota_relief", "automatic_reset"] as ResetTarget[]).flatMap(target =>
    [24, 48].map(hours => ({ target, hours, card: cards.find(card => card.target === target && card.hours === hours) ?? null })),
  );
}
