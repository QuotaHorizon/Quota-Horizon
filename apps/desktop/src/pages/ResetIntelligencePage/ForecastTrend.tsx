import { useState, type CSSProperties } from "react";
import type { Language } from "../../i18n";
import { publicEvidenceTime } from "./presentation";
import type { RadarPoint, RadarView } from "./types";
import { forecastColor, forecastSeries } from "./radarOverviewModel";
import styles from "./resetBriefing.module.less";

export function ForecastTrend({ radar, now, language }: { radar: RadarView; now: number; language: Language }) {
  const zh = language === "zh";
  const [fullDay, setFullDay] = useState(false);
  const series = forecastSeries(radar, now);
  const localStart = series.find(s => s.id === "horizon")?.points[0]?.at;
  const start = fullDay || !localStart ? now - 86400_000 : Math.min(Date.parse(localStart), now - 1800_000);
  const x = (time: number) => 44 + (time - start) / (now - start) * 448;
  const y = (p: RadarPoint, hours: 24 | 48) => 169 - (hours === 24 ? p.probability24h : p.probability48h) * 1.45;
  const tick = (t: number) => new Date(t).toLocaleTimeString(zh ? "zh-CN" : "en-GB", { hour: "2-digit", minute: "2-digit", hour12: false });
  return <div className={styles.trend}>
    <div className={styles.cardHeading}><h3>{zh ? "历史预测变化 · 来源同图对照" : "Recorded forecasts · sources compared"}</h3></div>
    <div className={styles.trendControls}><div className={styles.lineKey}><span><i />24h</span><span><i data-dashed />48h</span></div><button type="button" onClick={() => setFullDay(!fullDay)}>{fullDay ? (zh ? "缩放到本机记录时段" : "Zoom to local recording period") : (zh ? "查看完整 24 小时" : "Show full 24 hours")}</button></div>
    <svg viewBox="0 0 520 205" role="img" aria-label={zh ? "各来源 24h 和 48h 预测曲线，实线为 24h，虚线为 48h" : "All source forecasts: solid lines for 24h, dashed lines for 48h"}>
      {[0, 50, 100].map(n => <g key={n}><line x1="44" x2="492" y1={169 - n * 1.45} y2={169 - n * 1.45} className={styles.gridLine} /><text x="36" y={173 - n * 1.45} textAnchor="end">{n}%</text></g>)}
      {series.map(s => {
        // Clip line segments at the view boundary. Boundary intersections are
        // not observations and receive no dot or fabricated timestamp.
        const paths = s.points.reduce<RadarPoint[][]>((all, p, i) => {
          const prior = s.points[i - 1];
          if (!prior || Date.parse(p.at) - Date.parse(prior.at) > (s.id === "reset_monitor" ? 8 * 60 : 45) * 60_000 || prior.modelVersion !== p.modelVersion) all.push([]);
          all[all.length - 1].push(p); return all;
        }, []);
        return <g key={s.id} style={{ "--series-color": forecastColor(s.id) } as CSSProperties}>
          {([24, 48] as const).map(hours => <g key={hours} data-series={`${s.id}-${hours}`}>
            {paths.flatMap((path, index) => {
              const visible = path.filter(p => Date.parse(p.at) >= start);
              if (!visible.length) return [];
              const before = path.filter(p => Date.parse(p.at) < start).at(-1);
              let d = visible.map((p, i) => `${i ? "L" : "M"}${x(Date.parse(p.at)).toFixed(1)},${y(p, hours).toFixed(1)}`).join(" ");
              if (before) { const next = visible[0]; const f = (start - Date.parse(before.at)) / (Date.parse(next.at) - Date.parse(before.at)); d = `M44,${(y(before,hours) + f * (y(next,hours) - y(before,hours))).toFixed(1)} L${d.slice(1)}`; }
              return <path key={index} d={d} className={styles.overlayLine} strokeDasharray={hours === 48 ? "6 5" : undefined} strokeWidth={s.id === "horizon" ? 3 : 1.8} />;
            })}
            {s.points.filter(p => Date.parse(p.at) >= start).map(p => <circle key={p.at} cx={x(Date.parse(p.at))} cy={y(p,hours)} r={p === s.points.at(-1) ? 3.5 : 1.5} className={styles.overlayDot}><title>{s.name} · {hours}h · {Number((hours === 24 ? p.probability24h : p.probability48h).toFixed(1))}% · {zh ? "生成 " : "Generated "}{publicEvidenceTime(p.at,language)}{p.observedAt ? ` · ${zh ? "本机读取 " : "Observed "}${publicEvidenceTime(p.observedAt,language)}` : ""}</title></circle>)}
          </g>)}
        </g>;
      })}
      <text x="44" y="199">{tick(start)}</text><text x="268" y="199" textAnchor="middle">{tick((now + start) / 2)}</text><text x="492" y="199" textAnchor="end">{tick(now)} · {zh ? "现在" : "Now"}</text>
    </svg>
    <div className={styles.seriesLegend}>{series.map(s => <span key={s.id}><span><i style={{ background: forecastColor(s.id) }} />{s.name} <small>{s.points.length} {zh ? "次预测" : s.points.length === 1 ? "forecast" : "forecasts"}</small></span><small>{s.points[0] ? `${zh ? "记录起点 " : "First record "}${publicEvidenceTime(s.points[0].at, language)}` : zh ? "等待首条记录" : "Awaiting first record"}</small></span>)}</div>
    <p className={styles.trendCaption}>{zh ? "横轴统一为预测生成时间。Monitor 含网站提供的较早历史，其他来源从本机首次记录积累；空白表示尚无记录，重复读取不增加预测点。" : "The x-axis uses forecast generation time. Monitor includes earlier provider history; other series begin with local collection. Blank periods have no records; repeated reads add no forecast points."}</p>
  </div>;
}
