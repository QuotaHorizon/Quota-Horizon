import type { QuantForecast, QuantitativeView, QuantReport, ResetTarget } from "./quantitativeTypes";
import type { PublicPost, PublicResetTimeline } from "./types";

export const sourceNames: Record<string, string> = { horizon: "Horizon", codex_reset: "Codex Reset", monitor: "Reset Monitor", nextreset: "NextReset", observatory: "Reset Observatory", lunar: "Lunar", dreaife: "Dreaife" };
export const sourceUrls: Record<string, string> = { codex_reset: "https://codex-reset.com/", monitor: "https://codexreset.org/", nextreset: "https://nextreset.ai/", observatory: "https://codex.gussuriworks.com/", lunar: "https://codex.lunarwerx.com/", dreaife: "https://codexreset.dreaife.tokyo/" };
export const sourceColors: Record<string, string> = { horizon: "var(--radar-accent)", codex_reset: "var(--radar-amber)", monitor: "var(--radar-blue)", nextreset: "var(--radar-violet)", observatory: "var(--quant-cyan)", lunar: "var(--quant-rose)", community: "var(--quant-rose)", survival_baseline: "var(--muted)" };
export const percent = (p: number | null | undefined) => p == null || !Number.isFinite(p) ? "—" : `${(p * 100).toFixed(1)}%`;
export const points = (p: number | null) => p == null ? "—" : `${p > 0 ? "+" : ""}${(p * 100).toFixed(1)}`;
export const forecastFor = (v: QuantitativeView | undefined, target: ResetTarget = "broad_quota_relief") => v?.forecast?.forecasts.find(f => f.target === target);
export const probabilityAt = (f: QuantForecast | undefined, hours: number) => f?.curve.find(p => p.hours === hours)?.probability ?? null;
export function controllingPoint(f: QuantForecast | undefined, automatic: QuantForecast | undefined, hours: number) {
  const point = f?.curve.find(p => p.hours === hours), auto = automatic?.curve.find(p => p.hours === hours);
  if (f?.combination_method !== "baseline_plus_revisions" || !point || !auto
    || f.issued_at !== automatic?.issued_at || (point.coherence_adjustment ?? 0) <= 0) return point;
  if (Math.abs(point.probability-auto.probability) < 1e-8) return auto;
  if (Math.abs(point.probability-(point.community_adjustment ?? 0)-auto.probability) < 1e-8) {
    // The broader outlook inherits the automatic forecast before its own
    // ballot supplement. Keep that source revision visible in the breakdown.
    return { ...auto, probability: point.probability, component_range: point.component_range,
      community_adjustment: (auto.community_adjustment ?? 0) + point.probability-auto.probability };
  }
  return point;
}
export function forecastStale(v: QuantitativeView, f: QuantForecast | undefined, now: number) {
  return !!v.forecastIssue || !f || f.status !== "current" || !f.valid_until || Date.parse(f.valid_until) <= now
    || !Number.isFinite(Date.parse(f.issued_at)) || Date.parse(f.issued_at) > now + 60_000;
}
export function evaluationStale(v: QuantitativeView, now: number) {
  const time = Date.parse(v.evaluation?.generated_at ?? "");
  return !!v.evaluationIssue || !Number.isFinite(time) || now - time > 180_000 || time > now + 60_000;
}
export function sameForecastVersion(version: string, f: QuantForecast) { return f.evaluation_version ? version === f.evaluation_version : version.startsWith(`${f.model_version}:`); }
/** Use the forecast's own ablation before the independently scheduled report.
 * Legacy exports require an exact issue/version match in that report. */
export function communityEffect(report: QuantReport | null, f: QuantForecast | undefined, hours: number) {
  if (!f) return null;
  const point = f.curve.find(p => p.hours === hours), p = point?.probability;
  if (p == null || !Number.isFinite(p) || p < 0 || p > 1) return null;
  const direct = point?.without_community_probability;
  if (direct != null && Number.isFinite(direct) && direct >= 0 && direct <= 1) return p - direct;
  const model = report?.latest.find(r => r.source === "horizon" && r.track === "model" && r.target === f.target
    && r.origin === f.issued_at && sameForecastVersion(r.version, f));
  const other = report?.latest.find(r => r.source === "without_community" && r.track === "shadow" && r.target === f.target
    && r.origin === f.issued_at && r.version === model?.version);
  const without = other?.probabilities_at_publication[String(hours)];
  return without == null || !Number.isFinite(without) || without < 0 || without > 1 ? null : p - without;
}
export function combinedStatements(timeline: PublicResetTimeline): PublicPost[] {
  const posts = new Map<string, PublicPost>();
  for (const s of forecastFor(timeline.quantitative)?.statements ?? []) {
    const previous = posts.get(s.id);
    posts.set(s.id, { id: s.id, url: s.url, text: s.original, translatedText: s.translation_zh ?? previous?.translatedText ?? null, publishedAt: new Date(s.at*1000).toISOString(),
      kind: /banked|credit|grant/.test(s.event_type ?? "") ? "grant" : s.event_type === "usage_reset" ? s.stage === "completed" ? "reset_report" : "notice" : previous?.kind ?? "context",
      stage: s.stage ?? previous?.stage, relevance: s.relevance ?? previous?.relevance,
      isReply: s.is_reply ?? previous?.isReply ?? /^@\w+/u.test(s.original), parent: s.parent ? {...s.parent, checkedAt: null} : previous?.parent ?? null,
      collectorName: s.collector ?? "Codex Reset FM", contextZh: s.context ?? previous?.contextZh });
  }
  // The live post feed carries the newest replies and richer parent topology.
  for (const p of timeline.insights?.posts?.posts ?? []) {
    const previous = posts.get(p.id);
    posts.set(p.id, { ...previous, ...p, translatedText: p.translatedText ?? previous?.translatedText ?? null,
      stage: p.stage ?? previous?.stage, relevance: p.relevance ?? previous?.relevance,
      kind: previous?.stage === "completed" ? previous.kind : p.kind,
      isReply: p.isReply ?? previous?.isReply ?? null, parent: p.parent?.text ? p.parent : previous?.parent?.text ? previous.parent : p.parent ?? previous?.parent ?? null });
  }
  return [...posts.values()].sort((a,b)=>Date.parse(b.publishedAt)-Date.parse(a.publishedAt));
}
export function latestCompletion(timeline: PublicResetTimeline, now: number) {
  return combinedStatements(timeline).find(p => p.stage === "completed" && ["reset_report", "grant"].includes(p.kind)
    && Date.parse(p.publishedAt) <= now + 60_000
    && !timeline.quantitative?.evaluation?.outcomes.some(outcome =>
      outcome.status === "pending" && outcome.reason === "completion_withdrawn"
      && outcome.known_at * 1000 <= now && outcome.known_at * 1000 >= Date.parse(p.publishedAt)
      && outcome.references.some(url => url.match(/\/status\/(\d+)(?:\/|$|\?)/u)?.[1] === p.id)));
}
export function activeForecastAnnouncement(timeline: PublicResetTimeline, now: number) {
  return timeline.quantitative?.forecast?.forecasts.flatMap(f => {
    const r = f.response;
    if (!r || r.kind !== "announcement" || r.deadline == null || r.deadline*1000 <= now
      || r.at*1000 > now || !r.reviewed.length || forecastStale(timeline.quantitative!, f, now)) return [];
    const post = combinedStatements(timeline).find(p => p.id === r.url.match(/\/status\/(\d+)/u)?.[1]);
    return post && !["completed", "cancelled", "retracted"].includes(post.stage ?? "") ? [post] : [];
  }).sort((a, b) => Date.parse(b.publishedAt) - Date.parse(a.publishedAt))[0];
}
export function precedesCompletion(f: QuantForecast | undefined, post: PublicPost | undefined) {
  if (!post || !f || (post.kind === "grant" && f.target === "automatic_reset")) return false;
  const at = Date.parse(post.publishedAt);
  if (f.cycle?.references.some(url => url.match(/\/status\/(\d+)/u)?.[1] === post.id)) return false;
  const boundary = f.cycle ? f.cycle.invalidation_at : f.last_event_at;
  return at > (boundary != null ? boundary * 1000 : Date.parse(f.issued_at));
}
export const recentOutcomes = (report: QuantReport) => [...report.outcomes].sort((a, b) =>
  (b.to ?? b.announced_at ?? 0) - (a.to ?? a.announced_at ?? 0)).slice(0, 8);
export function sourceRows(view: QuantitativeView) {
  const latest = new Map<string, NonNullable<QuantReport["latest"]>[number]>();
  for (const row of (view.evaluation?.latest ?? []).filter(f => f.track === "native" && sourceNames[f.source])) {
    const key = `${row.source}-${row.target}`, old = latest.get(key);
    if (!old || Date.parse(row.origin) > Date.parse(old.origin)
      || (row.origin === old.origin && Date.parse(row.received_at) > Date.parse(old.received_at))) latest.set(key, row);
  }
  // The producer can update before the observer's next import. Its retained
  // native points are the same public source values, not the aligned inputs.
  for (const f of view.forecast?.forecasts ?? []) for (const s of f.sources) {
    if (!sourceNames[s.id] || !s.source_points?.length || s.origin == null) continue;
    const key = `${s.id}-${f.target}`, old = latest.get(key), date = new Date(s.origin*1000);
    if (!Number.isFinite(date.getTime()) || (old && Date.parse(old.origin) > date.getTime())) continue;
    const values = Object.fromEntries([24,48].map(h => [h, s.source_points!.find(p => p[0] === h)?.[1] ?? null]));
    latest.set(key, { source: s.id, target: f.target, track: "native", version: `consumer:${f.model_version}`,
      origin: date.toISOString(), received_at: f.issued_at, valid_until: s.expires_at != null && Number.isFinite(new Date(s.expires_at*1000).getTime()) ? new Date(s.expires_at*1000).toISOString() : old?.valid_until ?? f.valid_until ?? f.issued_at,
      state: f.status, probabilities_at_publication: values });
  }
  return [...latest.values()].map(row => ({
    ...row,
    state: Date.parse(row.origin) < ((forecastFor(view, row.target)?.cycle
      ? forecastFor(view, row.target)?.cycle?.invalidation_at : forecastFor(view, row.target)?.last_event_at) ?? 0)*1000 ? "cycle_closed" : row.state,
    role: forecastFor(view, row.target)?.sources.find(s => s.id === row.source)?.role,
    response_state: forecastFor(view, row.target)?.sources.find(s => s.id === row.source)?.response_state,
    comment: forecastFor(view, row.target)?.sources.find(s => s.id === row.source)?.comment,
    included: !!forecastFor(view, row.target)?.sources.some(s => s.id === row.source && (s.weight ?? 1) > 0),
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
