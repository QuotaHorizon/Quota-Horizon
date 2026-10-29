import type { Language } from "../../i18n";
import type { PublicResetTimeline } from "./types";
import { combinedStatements, forecastFor, percent, points, sourceNames } from "./quantitativeModel";
import { postReading } from "./resetBriefingModel";
import { publicEvidenceTime } from "./presentation";
import { EvidenceLink } from "./ArchiveView";
import styles from "./quantitative.module.less";

export function ForecastChanges({ timeline, language, onOpenSource }: {
  timeline: PublicResetTimeline; language: Language; onOpenSource?: (url: string) => void;
}) {
  const zh = language === "zh", changes = forecastFor(timeline.quantitative)?.changes;
  const names: Record<string,string> = { ...sourceNames, radar_revision: zh ? "来源修正" : "Source revisions", community: zh ? "投票补充" : "Ballot residual", survival_baseline: zh ? "历史基准" : "Historical baseline" };
  const ids = [...new Set(changes?.windows.flatMap(w => w.drivers.map(d => d.id)) ?? [])];
  ids.sort((a, b) => Math.max(...changes!.windows.map(w => Math.abs(w.drivers.find(d => d.id === b)?.effect ?? 0))) - Math.max(...changes!.windows.map(w => Math.abs(w.drivers.find(d => d.id === a)?.effect ?? 0))));
  const fresh = combinedStatements(timeline).filter(p => changes?.new_statement_ids.includes(p.id));
  return <article className={`${styles.card} ${styles.changes}`} aria-label={zh ? "本次判断为何变化" : "Why the outlook changed"}>
    <div className={styles.heading}><h2>{zh ? "预测变化" : "Forecast changes"}</h2>{changes && <span>{publicEvidenceTime(changes.since, language)} → {publicEvidenceTime(changes.until, language)}</span>}</div>
    {changes ? <>
      <div className={styles.changeTotals}>{changes.windows.map(w => <div key={w.hours}><span>{w.hours}h</span><strong>{percent(w.before)} <span>→</span> {percent(w.after)}</strong><b data-direction={w.delta >= 0 ? "up" : "down"}>{points(w.delta)} {zh ? "点" : "pts"}</b></div>)}</div>
      <details className={styles.disclosure}><summary>{zh ? "输入与权重详情" : "Input and weight details"}</summary>
        <div className={styles.driverHeader}><span>{zh ? "贡献（点）" : "Contribution (pts)"}</span><span>24h</span><span>48h</span></div>
        {ids.map(id => <div className={styles.driverRow} key={id}><span>{names[id] ?? id}</span>{[24,48].map(h => { const d = changes.windows.find(w => w.hours === h)?.drivers.find(d => d.id === id); return <b key={h} data-direction={(d?.effect ?? 0) >= 0 ? "up" : "down"}>{points(d?.effect ?? null)}</b>; })}</div>)}
        {changes.windows.some(w => Math.abs(w.coherence_effect) >= .00005) && <div className={styles.driverRow}><span>{zh ? "事件包含关系调整" : "Event-scope adjustment"}</span>{changes.windows.map(w => <b key={w.hours}>{points(w.coherence_effect)}</b>)}</div>}
        <p>{zh ? "比较实际发布的两次预测，最长回看 6 小时。各项贡献之和等于综合概率的变化；输入按各次预测时间转换。" : "Compares two issued forecasts, looking back up to six hours. Contributions sum to the combined change; inputs are aligned to each forecast’s issue time."}</p>
        {changes.windows.map(w => <div key={w.hours}><h3>{w.hours}h</h3>{w.drivers.map(d => <div key={d.id} className={styles.driverDetail}><strong>{names[d.id] ?? d.id}</strong><span>{changes.basis === "increment_terms" && d.id !== "survival_baseline" ? (zh ? "修正点数 " : "Revision points ") : (zh ? "模型输入 " : "Model input ")}{changes.basis === "increment_terms" && d.id !== "survival_baseline" ? points(d.before) : percent(d.before)} → {changes.basis === "increment_terms" && d.id !== "survival_baseline" ? points(d.after) : percent(d.after)}</span>{changes.basis !== "increment_terms" && <span>{zh ? "权重 " : "Weight "}{percent(d.weight_before)} → {percent(d.weight_after)}</span>}<span>{zh ? "输入变化贡献 " : "Input effect "}{points(d.input_effect)} · {zh ? "权重变化贡献 " : "Weight effect "}{points(d.weight_effect)}</span>{(d.raw_before != null || d.raw_after != null) && <span>{zh ? "来源发布原值 " : "Published source value "}{percent(d.raw_before)} → {percent(d.raw_after)}</span>}</div>)}</div>)}
        {!!fresh.length && <div className={styles.newStatements}><strong>{zh ? "同期发言" : "Statements in this period"}</strong>{fresh.map(p => <div key={p.id}><EvidenceLink url={p.url} label={postReading(p,language,timeline).title} onOpen={onOpenSource} /><time>{publicEvidenceTime(p.publishedAt,language)}</time></div>)}</div>}
      </details>
    </> : <p className={styles.caption}>{zh ? "等待同版本的下一次预测。" : "Awaiting the next forecast from this model version."}</p>}
  </article>;
}
