import type { RadarForecast, RadarOpinion, RadarPoint } from "./types";

export const forecastColor = (id: string) => ({ horizon: "var(--radar-accent)", reset_monitor: "var(--radar-blue)", codex_reset: "var(--radar-amber)" }[id] ?? "var(--muted)");
export const acceptedForecasts = (sources: RadarForecast[]) => sources.filter(s => s.id !== "reset_app" && s.method !== "mixed" && !["source_retired", "method_unverified", "missing_timestamp"].includes(s.exclusion ?? ""));

export function historyDelta(points: RadarPoint[], now: number, hours: 24 | 48): number | null {
  const current = points.filter(p => Date.parse(p.at) <= now).at(-1);
  const earlier = points.filter(p => Math.abs(Date.parse(p.at) - (now - 6 * 3_600_000)) <= 30 * 60_000)
    .sort((a, b) => Math.abs(Date.parse(a.at) - (now - 6 * 3_600_000)) - Math.abs(Date.parse(b.at) - (now - 6 * 3_600_000)))[0];
  if (!current || !earlier || current.modelVersion !== earlier.modelVersion || now - Date.parse(current.at) > 30 * 60_000) return null;
  return Math.round((hours === 24 ? current.probability24h - earlier.probability24h : current.probability48h - earlier.probability48h) * 10) / 10;
}

export function evidenceGroups(sources: RadarForecast[]) {
  const groups = new Map<string, string[]>();
  for (const source of sources.filter(s => s.probability24h != null)) {
    const keys = [...new Set(["history", ...source.evidenceUrls, ...(source.method === "mixed" ? [`undisclosed:${source.id}`] : [])])];
    for (const key of keys) groups.set(key, [...(groups.get(key) ?? []), source.name]);
  }
  return [...groups].map(([key, sources]) => ({ key, sources }));
}

export function currentOpinions(opinions: RadarOpinion[], now: number, hours = 6) {
  const authors = new Set<string>(); const texts = new Set<string>();
  return opinions.filter(p => Date.parse(p.publishedAt) <= now && Date.parse(p.publishedAt) > now - hours * 3_600_000)
    .sort((a, b) => Date.parse(b.publishedAt) - Date.parse(a.publishedAt)).filter(p => {
      const author = `${p.channel}:${p.author.toLowerCase()}`;
      const text = p.text.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
      if (authors.has(author)) return false;
      authors.add(author);
      if (texts.has(text)) return false;
      texts.add(text); return true;
    });
}
