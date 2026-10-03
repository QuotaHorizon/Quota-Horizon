import type { CSSProperties } from "react";
import type { Language } from "../../i18n";
import { publicEvidenceTime } from "./presentation";
import type { RadarPoint, RadarView } from "./types";
import { acceptedForecasts, forecastColor } from "./radarOverviewModel";
import styles from "./resetBriefing.module.less";

export function ForecastTrend({ radar, now, language }: { radar: RadarView; now: number; language: Language }) {
  const zh = language === "zh";
  const series = [{ id: "horizon", name: "Horizon", history: radar.history }, ...acceptedForecasts(radar.forecasts)]
    .map(s => ({ ...s, points: s.history.filter(p => Date.parse(p.at) <= now && Date.parse(p.at) >= now - 86400_000).sort((a, b) => Date.parse(a.at) - Date.parse(b.at)) }));
  const x = (p: RadarPoint) => 44 + (Date.parse(p.at) - now + 86400_000) / 86400_000 * 448;
  const y = (p: RadarPoint, hours: 24 | 48) => 169 - (hours === 24 ? p.probability24h : p.probability48h) * 1.45;
  return <div className={styles.trend}>
    <div className={styles.cardHeading}><h3>{zh ? "预测变化 · 来源同图对照" : "Forecast trends · sources compared"}</h3><span>{zh ? "过去 24 小时" : "Past 24 hours"}</span></div>
    <div className={styles.lineKey}><span><i />24h</span><span><i data-dashed />48h</span></div>
    <svg viewBox="0 0 520 205" role="img" aria-label={zh ? "各来源 24h 和 48h 预测曲线，实线为 24h，虚线为 48h" : "All source forecasts: solid lines for 24h, dashed lines for 48h"}>
      {[0, 50, 100].map(n => <g key={n}><line x1="44" x2="492" y1={169 - n * 1.45} y2={169 - n * 1.45} className={styles.gridLine} /><text x="36" y={173 - n * 1.45} textAnchor="end">{n}%</text></g>)}
      {series.map(s => {
        const paths = s.points.reduce<RadarPoint[][]>((all, p, i) => {
          const prior = s.points[i - 1];
          if (!prior || Date.parse(p.at) - Date.parse(prior.at) > (s.id === "reset_monitor" ? 8 * 60 : 45) * 60_000 || prior.modelVersion !== p.modelVersion) all.push([]);
          all[all.length - 1].push(p); return all;
        }, []);
        return <g key={s.id} style={{ "--series-color": forecastColor(s.id) } as CSSProperties}>
          {([24, 48] as const).map(hours => <g key={hours} data-series={`${s.id}-${hours}`}>
            {paths.map((path, i) => <path key={i} d={path.map((p, index) => `${index ? "L" : "M"}${x(p).toFixed(1)},${y(p, hours).toFixed(1)}`).join(" ")} className={styles.overlayLine} strokeDasharray={hours === 48 ? "6 5" : undefined} strokeWidth={s.id === "horizon" ? 3 : 1.8} />)}
            {s.points.map(p => <circle key={p.at} cx={x(p)} cy={y(p, hours)} r={p === s.points.at(-1) ? 3.5 : 1.5} className={styles.overlayDot}><title>{s.name} · {hours}h · {hours === 24 ? p.probability24h : p.probability48h}% · {publicEvidenceTime(p.at, language)}</title></circle>)}
          </g>)}
        </g>;
      })}
      <text x="44" y="199">−24h</text><text x="268" y="199" textAnchor="middle">−12h</text><text x="492" y="199" textAnchor="end">{zh ? "现在" : "Now"}</text>
    </svg>
    <div className={styles.seriesLegend}>{series.map(s => <span key={s.id}><i style={{ background: forecastColor(s.id) }} />{s.name}<small>{s.points.length} {zh ? "点" : "points"}</small></span>)}</div>
    <p className={styles.trendCaption}>{zh ? "各来源原值与 Horizon 同时展示；空白时段尚无记录，模型版本变化处断开连线。" : "Source values and Horizon appear together. Unrecorded periods and model-version changes remain disconnected."}</p>
  </div>;
}
