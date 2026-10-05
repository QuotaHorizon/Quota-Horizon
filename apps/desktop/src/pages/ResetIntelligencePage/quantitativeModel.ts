import type { QuantForecast, QuantitativeView, QuantReport, ResetTarget } from "./quantitativeTypes";
import type { PublicPost, PublicResetTimeline } from "./types";

export const sourceNames: Record<string, string> = { horizon: "Horizon", codex_reset: "Codex Reset", monitor: "Reset Monitor", nextreset: "NextReset", observatory: "Reset Observatory", lunar: "Lunar", dreaife: "Dreaife" };
export const sourceUrls: Record<string, string> = { codex_reset: "https://codex-reset.com/", monitor: "https://codexreset.org/", nextreset: "https://nextreset.ai/", observatory: "https://codex.gussuriworks.com/", lunar: "https://codex.lunarwerx.com/", dreaife: "https://codexreset.dreaife.tokyo/" };
export const sourceColors: Record<string, string> = { horizon: "var(--radar-accent)", codex_reset: "var(--radar-amber)", monitor: "var(--radar-blue)", nextreset: "var(--radar-violet)", observatory: "var(--quant-cyan)", lunar: "var(--quant-rose)", community: "var(--quant-rose)", survival_baseline: "var(--muted)" };
export const percent = (p: number | null | undefined) => p == null || !Number.isFinite(p) ? "—" : `${(p * 100).toFixed(1)}%`;
export const points = (p: number | null) => p == null ? "—" : `${p > 0 ? "+" : ""}${(p * 100).toFixed(1)}`;
export const forecastFor = (v: QuantitativeView | undefined, target: ResetTarget = "broad_quota_relief") => v?.forecast?.forecasts.find(f => f.target === target);
export const probabilityAt = (f: QuantForecast | undefined, hours: number) => f?.curve.find(p => p.hours === hours)?.probability ?? null;
export function forecastStale(v: QuantitativeView, f: QuantForecast | undefined, now: number) {
  return !!v.forecastIssue || !f || f.status !== "current" || !f.valid_until || Date.parse(f.valid_until) <= now
    || !Number.isFinite(Date.parse(f.issued_at)) || Date.parse(f.issued_at) > now + 60_000;
}
export function evaluationStale(v: QuantitativeView, now: number) {
  const time = Date.parse(v.evaluation?.generated_at ?? "");
  return !!v.evaluationIssue || !Number.isFinite(time) || now - time > 180_000 || time > now + 60_000;
}
export function sameForecastVersion(version: string, f: QuantForecast) { return version.startsWith(`${f.model_version}:`); }
/** Only a contemporaneous ablation can quantify the community contribution.
 * The public consumer deliberately does not expose constituent private inputs. */
export function communityEffect(report: QuantReport | null, f: QuantForecast | undefined, hours: number) {
  if (!f) return null;
  const model = report?.latest.find(r => r.source === "horizon" && r.track === "model" && r.target === f.target
    && r.origin === f.issued_at && sameForecastVersion(r.version, f));
  const other = report?.latest.find(r => r.source === "without_community" && r.track === "shadow" && r.target === f.target
    && r.origin === f.issued_at && r.version === model?.version);
  const p = probabilityAt(f, hours), without = other?.probabilities_at_publication[String(hours)];
  return p == null || without == null ? null : p - without;
}
export function combinedStatements(timeline: PublicResetTimeline): PublicPost[] {
  const posts = new Map<string, PublicPost>();
  for (const s of forecastFor(timeline.quantitative)?.statements ?? []) {
    posts.set(s.id, { id: s.id, url: s.url, text: s.original, translatedText: s.translation_zh, publishedAt: new Date(s.at*1000).toISOString(),
      kind: /banked|credit|grant/.test(s.event_type ?? "") ? "grant" : s.event_type === "usage_reset" ? s.stage === "completed" ? "reset_report" : "notice" : "context",
      isReply: /^@\w+/u.test(s.original), parent: null, collectorName: s.collector ?? "Reset Monitor", contextZh: s.context ?? undefined });
  }
  // The live post feed carries the newest replies and richer parent topology.
  for (const p of timeline.insights?.posts?.posts ?? []) posts.set(p.id, p);
  return [...posts.values()].sort((a,b)=>Date.parse(b.publishedAt)-Date.parse(a.publishedAt));
}
export const recentOutcomes = (report: QuantReport) => [...report.outcomes].sort((a, b) =>
  (b.to ?? b.announced_at ?? 0) - (a.to ?? a.announced_at ?? 0)).slice(0, 8);
export function sourceRows(view: QuantitativeView) {
  return (view.evaluation?.latest ?? []).filter(f => f.track === "native" && sourceNames[f.source]).map(row => ({
    ...row, included: !!forecastFor(view, row.target)?.sources.some(s => s.id === row.source),
  })).sort((a, b) => Number(b.included) - Number(a.included) || a.source.localeCompare(b.source));
}
export function seriesPaths(points: { at: string; p: number | null }[], start: number, end: number, x: (at: number) => number, y: (p: number) => number) {
  const paths: string[] = []; let last: number | null = null;
  for (const p of points) {
    const at = Date.parse(p.at);
    if (at < start || at > end) continue;
    if (p.p == null || !Number.isFinite(p.p)) { last = null; continue; }
    const point = `${x(at).toFixed(2)},${y(p.p).toFixed(2)}`;
    if (last == null || at - last > 90 * 60_000) paths.push(`M${point}`);
    else paths[paths.length - 1] += ` L${point}`;
    last = at;
  }
  return paths;
}
