import type { QuantCurve, ResetTarget } from "./quantitativeTypes";
import { sourceNames } from "./quantitativeModel";

export const CANDLE_MS = 30 * 60_000;
const MAX_GAP = 90 * 60_000;
export interface TrendPoint { at: number; p: number }
export interface TrendSegment { key: string; curve: QuantCurve; points: TrendPoint[]; readUntil: number }
export interface ProbabilityCandle {
  key: string; segment: TrendSegment; bucket: number; from: number; until: number;
  open: number; high: number; low: number; close: number;
}
export const trendId = (curve: QuantCurve) => `${curve.source}-${curve.target}`;

export function trendSegments(curves: QuantCurve[], now: number) {
  // Include the whole first bucket: clipping already thinned receipts mid-bucket
  // would change that candle's open or hide an extremum.
  const start = Math.floor((now - 86400_000) / CANDLE_MS) * CANDLE_MS;
  const segments: TrendSegment[] = [];
  for (const curve of curves) {
    if (curve.hours !== 24 || !(curve.track === "model" && curve.source === "horizon"
      || curve.track === "native" && sourceNames[curve.source])) continue;
    let current: TrendSegment | null = null;
    const sorted = curve.points.map(p => ({ at: Date.parse(p.at), p: p.p }))
      .filter(p => Number.isFinite(p.at) && p.at >= start && p.at <= now).sort((a,b) => a.at-b.at);
    for (const point of sorted) {
      if (point.p == null || !Number.isFinite(point.p) || point.p < 0 || point.p > 1) {
        if (current) current.readUntil = point.at - 1;
        current = null; continue;
      }
      const last = current?.points.at(-1);
      if (!current || last && point.at-last.at > MAX_GAP) {
        current = { key: `${trendId(curve)}-${curve.version}-${segments.length}`, curve, points: [], readUntil: point.at };
        segments.push(current);
      }
      // Keep distinct values received together; they can contain a real extremum.
      if (current.points.at(-1)?.at === point.at && current.points.at(-1)?.p === point.p) continue;
      current.points.push({ at: point.at, p: point.p });
      current.readUntil = point.at;
    }
    // The current tail can be inspected as a last received value, with its own
    // timestamp. Never extend the drawn line or carry it through a closed gap.
    if (current) current.readUntil = Math.min(now, current.points.at(-1)!.at + MAX_GAP);
  }
  return segments.sort((a,b) => a.points[0].at-b.points[0].at);
}

export function candleTarget(segments: TrendSegment[]): ResetTarget {
  return segments.some(s => s.curve.source === "horizon" && s.curve.target === "broad_quota_relief")
    ? "broad_quota_relief" : "automatic_reset";
}
export function isCandleSeries(segment: TrendSegment, target: ResetTarget) {
  return segment.curve.source === "horizon" && segment.curve.track === "model"
    && segment.curve.target === target && segment.curve.candle_interval_minutes === 30;
}
export function probabilityCandles(segments: TrendSegment[], target: ResetTarget) {
  const candles: ProbabilityCandle[] = [];
  for (const segment of segments.filter(s => isCandleSeries(s,target))) {
    let current: ProbabilityCandle | undefined;
    for (const point of segment.points) {
      const bucket = Math.floor(point.at/CANDLE_MS)*CANDLE_MS;
      if (!current || bucket !== current.bucket) {
        current = { key: `${segment.key}-${bucket}`, segment, bucket, from: point.at, until: point.at,
          open: point.p, high: point.p, low: point.p, close: point.p };
        candles.push(current);
      } else {
        current.high = Math.max(current.high,point.p); current.low = Math.min(current.low,point.p);
        current.close = point.p; current.until = point.at;
      }
    }
  }
  return candles.sort((a,b) => a.from-b.from);
}
/** A crosshair reads actual receipts; it never bridges missing data or versions. */
export function trendPointAt(segment: TrendSegment, at: number): TrendPoint | null {
  const points = segment.points;
  if (at < points[0].at || at > segment.readUntil) return null;
  let lo=0, hi=points.length-1;
  while (lo < hi) { const mid=Math.ceil((lo+hi)/2); if (points[mid].at <= at) lo=mid; else hi=mid-1; }
  return points[lo];
}
export function latestTrendSegments(segments: TrendSegment[]) {
  const latest = new Map<string,TrendSegment>();
  for (const s of segments) {
    const previous=latest.get(trendId(s.curve));
    if (!previous || s.points.at(-1)!.at >= previous.points.at(-1)!.at) latest.set(trendId(s.curve),s);
  }
  return [...latest.values()].sort((a,b) => Number(b.curve.source==="horizon")-Number(a.curve.source==="horizon")
    || Number(b.curve.target==="broad_quota_relief")-Number(a.curve.target==="broad_quota_relief"));
}
