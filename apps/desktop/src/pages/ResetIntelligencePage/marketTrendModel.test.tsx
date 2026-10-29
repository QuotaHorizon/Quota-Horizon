import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { QuantitativeTrend } from "./MarketTrend";
import { candleTarget, latestTrendSegments, probabilityCandles, trendPointAt, trendSegments } from "./marketTrendModel";
import type { QuantCurve } from "./quantitativeTypes";

const t = Date.parse("2026-10-29T00:00:00Z");
const curve = (values: [number, number | null][], overrides: Partial<QuantCurve> = {}): QuantCurve => ({
  source:"horizon", track:"model", version:"v1", target:"broad_quota_relief", hours:24,
  candle_interval_minutes:30,
  points:values.map(([minute,p])=>({at:new Date(t+minute*60_000).toISOString(),p})), ...overrides,
});

describe("received probability candlesticks", () => {
  it("retains exact OHLC, including tiny extrema, zero, duplicate receipt times and partial latest buckets", () => {
    const segments=trendSegments([curve([[0,.5],[8,.501],[8,.499],[20,.5001],[30,0],[40,.2],[50,.1]])],t+41*60_000);
    const candles=probabilityCandles(segments,candleTarget(segments));
    expect(candles.map(c=>[c.open,c.high,c.low,c.close])).toEqual([[.5,.501,.499,.5001],[0,.2,0,.2]]);
    expect(candles[1].until).toBe(t+40*60_000);
  });
  it("keeps a whole first bucket so a rolling display cutoff cannot change its open", () => {
    const segments=trendSegments([curve([[0,.6],[5,.4],[20,.5],[30,.7]])],t+24*3600_000+10*60_000);
    expect(probabilityCandles(segments,"broad_quota_relief")[0].open).toBe(.6);
  });
  it("splits explicit gaps, long gaps and versions without manufacturing intermediate candles", () => {
    const segments=trendSegments([
      curve([[0,.8],[4,.9],[6,null],[10,.3],[15,.4],[120,.5]]),
      curve([[20,.7],[25,.6]],{version:"v2"}),
    ],t+121*60_000);
    const candles=probabilityCandles(segments,"broad_quota_relief");
    expect(candles.map(c=>[c.open,c.close])).toEqual([[.8,.9],[.3,.4],[.7,.6],[.5,.5]]);
    expect(new Set(candles.map(c=>c.key)).size).toBe(4);
    expect(trendPointAt(segments[0],t+8*60_000)).toBeNull();
    expect(trendPointAt(segments[1],t+90*60_000)).toBeNull();
    expect(trendPointAt(segments[0],t+2*60_000)?.p).toBe(.8);
  });
  it("excludes other horizons, hidden sources, future receipts and invalid probabilities", () => {
    const good=curve([[0,.2],[5,NaN],[10,.4],[15,1.1],[20,.5],[70,.8]]);
    const segments=trendSegments([good,curve([[0,.9]],{hours:48}),
      curve([[0,.9]],{source:"private_signal",track:"native"}),
      curve([[0,.9]],{source:"without_community",track:"shadow"})],t+60*60_000);
    expect(segments.flatMap(s=>s.points.map(p=>p.p))).toEqual([.2,.4,.5]);
    expect(segments).toHaveLength(3);
  });
  it("uses the latest version for the quote and keeps automatic reset as a separate line", () => {
    const segments=trendSegments([curve([[0,.1],[20,.2]]),curve([[30,.5],[40,.6]],{version:"v2"}),
      curve([[30,.4],[40,.3]],{target:"automatic_reset"})],t+50*60_000);
    expect(candleTarget(segments)).toBe("broad_quota_relief");
    const latest=latestTrendSegments(segments);
    expect(latest[0].curve.version).toBe("v2");
    expect(latest[0].points[0].p).toBe(.5);
    expect(probabilityCandles(segments,candleTarget(segments))).toHaveLength(2);
  });
  it("shows the latest received value with its timestamp until an explicit expiry, without extending its line", () => {
    const active=trendSegments([curve([[0,.3],[10,.4]])],t+20*60_000)[0];
    expect(trendPointAt(active,t+15*60_000)).toEqual({at:t+10*60_000,p:.4});
    expect(active.points).toHaveLength(2);
    const closed=trendSegments([curve([[0,.3],[10,.4],[12,null]])],t+20*60_000)[0];
    expect(trendPointAt(closed,t+11*60_000)?.p).toBe(.4);
    expect(trendPointAt(closed,t+12*60_000)).toBeNull();
  });
  it("renders faithful candles only from certified OHLC projections, preserving legacy lines", () => {
    const view=quantitativeFixture(t+60*60_000,true);
    view.evaluation!.curves=[curve([[0,.8],[4,.9],[20,.6],[28,.7],[30,.7]])];
    const html=renderToStaticMarkup(<QuantitativeTrend view={view} now={t+60*60_000} language="en"/>);
    expect(html).toContain('data-open="0.8" data-high="0.9" data-low="0.6" data-close="0.7"');
    expect(html).toContain('data-direction="down"');
    expect(html).toContain('data-direction="flat"');
    expect(html).not.toContain("48h");
    delete view.evaluation!.curves[0].candle_interval_minutes;
    const legacy=renderToStaticMarkup(<QuantitativeTrend view={view} now={t+60*60_000} language="en"/>);
    expect(legacy).not.toContain("data-candle=");
    expect(legacy).toContain("showing the saved line");
    expect(legacy).toMatch(/<path[^>]*d="M[^\"]+L/);
  });
});
