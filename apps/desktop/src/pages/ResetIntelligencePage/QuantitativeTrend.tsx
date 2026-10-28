import type { CSSProperties } from "react";
import type { Language } from "../../i18n";
import type { QuantitativeView } from "./quantitativeTypes";
import { controllingPoint, forecastFor, percent, seriesPaths, sourceColors, sourceNames } from "./quantitativeModel";
import { publicEvidenceTime } from "./presentation";
import styles from "./quantitative.module.less";
import { usePlotWidth } from "./usePlotWidth";

export function ProbabilityCurve({ view, language }: { view: QuantitativeView; language: Language }) {
  const zh = language === "zh", f = forecastFor(view), auto = forecastFor(view, "automatic_reset");
  const { ref, width } = usePlotWidth(490, 280);
  const right = width - 26;
  const series = [
    { name: zh ? "综合 · 重置或发卡" : "Combined · reset or credit", color: sourceColors.horizon, values: f?.curve.map(p => p.probability) ?? [], dashed: false },
    { name: zh ? "综合 · 自动重置" : "Combined · automatic reset", color: sourceColors.monitor, values: auto?.curve.map(p => p.probability) ?? [], dashed: true },
    { name: zh ? "社区综合" : "Community aggregate", color: sourceColors.community, values: f?.community?.probabilities ?? [], dashed: false },
    { name: zh ? "历史基准" : "Historical baseline", color: sourceColors.survival_baseline, values: f?.curve.map(p => controllingPoint(f,auto,p.hours)?.baseline ?? p.baseline) ?? [], dashed: true },
  ];
  const x = (h: number) => 40 + h / 72 * (right - 40), y = (p: number) => 167 - p * 141;
  return <div className={styles.chart} ref={ref}>
    <div className={styles.heading}><h3>{zh ? "未来 72 小时 · 累计概率" : "Next 72 hours · cumulative probability"}</h3></div>
    <svg viewBox={`0 0 ${width} 205`} role="img" aria-label={zh ? "综合、社区与基准的未来概率曲线，标出 24h 和 48h" : "Combined, community and baseline probability curves, with 24h and 48h marked"}>
      {[0, .5, 1].map(p => <g key={p}><line x1="40" x2={right} y1={y(p)} y2={y(p)} /><text x="32" y={y(p)+4} textAnchor="end">{p*100}%</text></g>)}
      {[24,48,72].map(h => <g key={h}><line x1={x(h)} x2={x(h)} y1="26" y2="167" strokeDasharray="3 5" /><text x={x(h)} y="191" textAnchor="middle">{h}h</text></g>)}
      {series.map(s => <g key={s.name} style={{ "--series-color": s.color } as CSSProperties}>
        <path className={styles.line} strokeDasharray={s.dashed ? "5 4" : undefined} d={s.values.map((p, i) => `${i ? "L" : "M"}${x(i+1)},${y(p)}`).join(" ")} />
        {[24,48].map(h => s.values[h-1] == null ? null : <circle key={h} className={styles.dot} cx={x(h)} cy={y(s.values[h-1])} r="3"><title>{`${s.name} · ${h}h · ${percent(s.values[h-1])}`}</title></circle>)}
      </g>)}
    </svg>
    <div className={styles.legend}>{series.filter(s => s.values.length).map(s => <span key={s.name}><i style={{ background: s.color }} />{s.name}</span>)}</div>
    {!f?.curve.length && <p>{zh ? "等待预测模块输出有效曲线。" : "Waiting for an available model curve."}</p>}
  </div>;
}

export function QuantitativeTrend({ view, language, now }: { view: QuantitativeView; language: Language; now: number }) {
  const zh = language === "zh";
  const { ref, width } = usePlotWidth(948, 480);
  const right = width - 28;
  const series = (view.evaluation?.curves ?? []).filter(c => c.hours === 24 && (c.track === "model" || (c.track === "native" && sourceNames[c.source])));
  // Native curves preserve publication values at their actual receipt time.
  const times = series.flatMap(s => s.points.filter(p => p.p != null && Date.parse(p.at) <= now && Date.parse(p.at) >= now - 86400_000).map(p => Date.parse(p.at)));
  const start = times.length ? Math.min(...times, now - 3600_000) : now - 3600_000;
  const x = (t: number) => 40 + (t-start) / (now-start) * (right-40), y = (p: number) => 172 - p*144;
  const tick = (t: number) => new Date(t).toLocaleTimeString(zh ? "zh-CN" : "en-GB", { hour: "2-digit", minute: "2-digit", hour12: false });
  const name = (source: string, target: string) => `${sourceNames[source] ?? source}${source === "horizon" && target === "automatic_reset" ? (zh ? " · 自动" : " · auto") : ""}`;
  const color = (source: string, target: string) => source === "horizon" && target === "automatic_reset" ? "var(--quant-orange)" : sourceColors[source] ?? "var(--muted)";
  const legends = [...new Map(series.filter(s => s.points.some(p => p.p != null && Date.parse(p.at) >= start && Date.parse(p.at) <= now)).map(s => [`${s.source}-${s.target}`, s])).values()];
  return <article className={styles.card}>
    <div className={styles.heading}><h2>{zh ? "预测怎样变化" : "How predictions change"}</h2><span>{zh ? "全部来源 · 24h 概率" : "All sources · 24h probability"}</span></div>
    <div className={styles.historyChart} ref={ref}><svg viewBox={`0 0 ${width} 218`} role="img" aria-label={zh ? "全部来源在统一观测时间的 24h 概率曲线" : "All sources’ 24h probabilities on the same observation clock"}>
      {[0,.5,1].map(p => <g key={p}><line x1="40" x2={right} y1={y(p)} y2={y(p)} /><text x="32" y={y(p)+4} textAnchor="end">{p*100}%</text></g>)}
      {series.map(s => <g key={`${s.source}-${s.target}-${s.hours}-${s.version}`} data-series={`${s.source}-${s.target}-${s.hours}`} style={{ "--series-color": color(s.source,s.target), "--series-width": s.track === "model" ? 3 : 1.6 } as CSSProperties}>
        {seriesPaths(s.points,start,now,x,y).map((d, i) => <path key={i} d={d} className={styles.line} />)}
        {s.points.filter(p => p.p != null && Date.parse(p.at) >= start && Date.parse(p.at) <= now).map(p => <circle key={p.at} className={styles.dot} cx={x(Date.parse(p.at))} cy={y(p.p!)} r="2.8"><title>{`${name(s.source,s.target)} · ${s.hours}h · ${percent(p.p)} · ${publicEvidenceTime(p.at,language)}`}</title></circle>)}
      </g>)}
      <text x="40" y="202">{tick(start)}</text><text x={(40+right)/2} y="202" textAnchor="middle">{tick((start+now)/2)}</text><text x={right} y="202" textAnchor="end">{tick(now)}</text>
    </svg></div>
    <div className={styles.legend}>{legends.map(s => <span key={`${s.source}-${s.target}`}><i style={{ background: color(s.source,s.target) }} />{name(s.source,s.target)}<small>{s.target === "automatic_reset" ? (zh ? "自动重置" : "automatic") : (zh ? "重置或发卡" : "reset or credit")}</small></span>)}</div>
    {(!view.evaluation && view.evaluationIssue || !times.length) && <p className={styles.caption}>{!view.evaluation && view.evaluationIssue ? (zh ? "评估报告读取失败，后台会自动重试。" : "The evaluation report could not be read. Retrying automatically in the background.") : (zh ? "等待首条预测记录。" : "Awaiting the first forecast record.")}</p>}
    {times.length > 0 && <details className={styles.chartNotes}><summary>{zh ? "曲线口径" : "About these curves"}</summary><p>{zh ? "展示来源发布的 24h 概率，按本机收到的时间记录并保留转折。缺失、过期及版本变化分段展示。" : "Published 24h probabilities at their receipt times, preserving turning points. Missing, expired and changed versions remain separate."}</p></details>}
  </article>;
}
