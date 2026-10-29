import type { CSSProperties } from "react";
import type { Language } from "../../i18n";
import type { QuantitativeView } from "./quantitativeTypes";
import { controllingPoint, forecastFor, percent, sourceColors } from "./quantitativeModel";
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

export { QuantitativeTrend } from "./MarketTrend";
