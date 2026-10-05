import { useState } from "react";
import type { Language } from "../../i18n";
import type { QuantitativeView } from "./quantitativeTypes";
import { evaluationStale, percent, recentOutcomes, sourceNames } from "./quantitativeModel";
import { publicEvidenceTime } from "./presentation";
import { EvidenceLink } from "./ArchiveView";
import styles from "./quantitative.module.less";

export function QuantitativeEvaluation({ view, language, now, onOpenSource }: {
  view: QuantitativeView; language: Language; now: number; onOpenSource?: (url: string) => void;
}) {
  const zh = language === "zh", report = view.evaluation;
  const [selected, setSelected] = useState("");
  if (!report) return <article id="forecast-evaluation" className={styles.card}><h2>{zh ? "预测评估" : "Forecast evaluation"}</h2><p className={styles.caption}>{zh ? "等待独立评估器输出；预测与实际事件会在这里持续对照。" : "Waiting for the independent evaluator. Forecasts and actual outcomes will be compared here."}</p></article>;
  const stale = evaluationStale(view,now), s = report.summary;
  const name = (id: string) => sourceNames[id] ?? ({ survival_baseline: zh ? "历史基准" : "Historical baseline", without_community: zh ? "移除社区" : "Without community", equal_components: zh ? "各分量等权" : "Equal component weights" }[id] ?? (zh ? "对照模型" : "Comparison model"));
  const scope = (t: string) => t === "automatic_reset" ? (zh ? "自动重置" : "Automatic reset") : (zh ? "重置或发卡" : "Reset or credit");
  const track = (t: string) => ({ native: zh ? "来源原值" : "Native", adapted: zh ? "转换输入" : "Aligned", model: zh ? "综合模型" : "Combined", shadow: zh ? "对照模型" : "Ablation" }[t]);
  const number = (n: number | null, digits = 3) => n == null ? "—" : n.toFixed(digits);
  const eligible = report.scorecards.filter(r => r.track === "model" || r.track === "native" || r.source === "survival_baseline" || r.source === "without_community");
  const cards = eligible.filter(r => [24,48].includes(r.hours) || r.scores.n > 0);
  const key = (r: typeof cards[number]) => `${r.source}-${r.version}-${r.target}-${r.track}-${r.hours}`;
  const scored = cards.filter(r => r.scores.n > 0);
  const active = scored.find(r => key(r) === selected) ?? scored.find(r => r.track === "model") ?? scored[0];
  const outcomeLabels: Record<string, string> = { confirmed: zh ? "完成有据" : "Completion evidenced", pending: zh ? "等待完成证据" : "Awaiting completion evidence", targeted: zh ? "局部补偿" : "Targeted compensation", disputed: zh ? "时间或范围有分歧" : "Timing or scope disputed" };
  const providers: Record<string,string> = { completion_feed: "Reset Monitor", public_catalogue: "Codex Resets", quota_events: "QuotaResets" };
  const unresolved = (s.settlements.unresolved ?? 0) + (s.settlements.disputed ?? 0);
  const progress = s.scoring_by_horizon;
  const scoredWindows = progress?.reduce((n, p) => n + p.scored_windows, 0) ?? 0;
  const rollingTrials = progress?.reduce((n, p) => n + p.eligible_rolling_trials, 0) ?? 0;
  const settledWindows = progress?.reduce((n, p) => n + p.settled_windows, 0) ?? 0;
  const waitingTitle = rollingTrials > 0
    ? (zh ? "已有滚动窗口结算，等待不重叠评分窗口" : "Outcomes settled; awaiting non-overlapping scoring windows")
    : settledWindows > 0 ? (zh ? "事件窗口已结算，对应时点没有可用预测" : "Outcomes settled; no eligible forecast at those origins")
    : unresolved > 0 ? (zh ? "已有检验窗到期，证据尚不足以记分" : "Some windows have matured; evidence is still insufficient to score")
    : (zh ? "正在积累第一批前瞻成绩" : "Collecting the first forward results");
  const waitingDetail = rollingTrials > 0
    ? (zh ? "滚动窗口已经用于逐时核验；主成绩仅采用预先固定的 UTC 不重叠窗口，以免重复放大同一时段。" : "Rolling windows are verified hourly. Headline scores use predeclared non-overlapping UTC windows to avoid counting the same period repeatedly.")
    : settledWindows > 0 ? (zh ? "实际结果已知，但这些窗口起点的预测缺失、过期或不覆盖相应期限，保留缺口。" : "Outcomes are known, but forecasts at these origins were missing, expired or outside their supported horizons. Those gaps remain unscored.")
    : unresolved > 0 ? (zh ? "部分窗口的事件观测存在缺口或分歧，暂不计入成绩。采集和核验仍在继续；证据充分的窗口会自动显示 Brier、对数损失、校准与误报情况。" : "Some windows have gaps or conflicts in outcome observations and remain unscored. Collection and verification continue; windows with sufficient evidence automatically receive Brier, log loss, calibration and false-alert results.")
    : (zh ? "预测已冻结。事件发生后自动核验，待对应窗口到期且观测覆盖充分后，显示 Brier、对数损失、校准与误报情况。" : "Forecasts are frozen. Events are checked automatically; Brier, log loss, calibration and false alerts appear after the corresponding windows mature with sufficient observation coverage.");
  return <article id="forecast-evaluation" className={styles.card}>
    <div className={styles.heading}><h2>{zh ? "预测经得起检验吗" : "Do the predictions hold up?"}</h2><span>{stale ? (zh ? "评估输出待更新" : "Evaluation update due") : (zh ? "独立评估 · 持续核验" : "Independent evaluation · ongoing")}</span></div>
    <div className={styles.evaluationStats}><div><strong>{s.forecast_snapshots}</strong><span>{zh ? "已冻结的预测快照" : "Frozen forecast snapshots"}</span></div><div><strong>{s.frozen_trials}</strong><span>{zh ? "来源 × 目标 × 检验窗" : "Source × target × test windows"}</span></div><div><strong>{s.scored_nonoverlapping_trials}</strong><span>{zh ? "已记分的非重叠检验" : "Scored non-overlapping trials"}</span></div><div><strong>{unresolved}</strong><span>{zh ? "待补足证据" : "Awaiting sufficient evidence"}</span></div></div>
    <div className={styles.evaluationProgress}><span>{zh ? "冻结预测" : "Freeze forecasts"}</span><i /><span>{zh ? "核验重置 / 发卡" : "Verify reset / credit"}</span><i /><span>{zh ? "窗口到期 + " : "Horizon + "}{report.policy.reporting_grace_seconds/3600}h</span><i /><span>{zh ? "记分与对照" : "Score and compare"}</span></div>
    <p className={styles.caption}>{s.next_maturity ? (zh ? "下一批检验窗最早可结算：" : "Next windows eligible for settlement: ") + publicEvidenceTime(s.next_maturity,language) : (zh ? "等待下一个检验窗。" : "Awaiting the next test window.")} {zh ? "记录数含不同来源和期限；独立事件数见各项成绩。" : "Counts include multiple sources and horizons; unique events are shown with each score."}</p>
    {progress && <details className={styles.disclosure}><summary>{zh ? `${s.scored_nonoverlapping_trials} 条评分，覆盖 ${scoredWindows} 个目标时间窗` : `${s.scored_nonoverlapping_trials} scores across ${scoredWindows} target-time windows`}</summary>
      <p>{zh ? "不同来源与对照模型共享同一时间窗；按目标和起止时间去重，窗口数不等于实际重置次数。滚动核验包含主评分窗口。" : "Different sources and variants share the same windows. Windows are deduplicated by target and time, not counted as reset events. Rolling checks include headline scoring windows."}</p>
      <div className={styles.tableScroll}><table><thead><tr>{(zh ? ["期限", "已结算窗口", "主评分窗口", "主评分条数", "滚动核验条数", "已结算但无预测"] : ["Horizon", "Settled windows", "Scored windows", "Scores", "Rolling checks", "Settled without forecast"]).map(h => <th key={h}>{h}</th>)}</tr></thead><tbody>{progress.map(p => <tr key={p.hours}><td>{p.hours}h</td><td>{p.settled_windows}</td><td>{p.scored_windows}</td><td>{p.scored_trials}</td><td>{p.eligible_rolling_trials}</td><td>{p.missing_forecast_trials}</td></tr>)}</tbody></table></div>
    </details>}
    {stale && <p className={styles.notice} role="status">{zh ? "评估器最近未更新，以下为保存的报告。" : "The evaluator has not updated recently. Showing the saved report."}</p>}
    {scored.length ? <>
      <div className={styles.tableScroll}><table><thead><tr>{(zh ? ["来源 / 版本", "范围 / 窗口", "窗口 / 事件", "Brier ↓", "对数损失 ↓", "预测覆盖"] : ["Source / version", "Scope / horizon", "Windows / events", "Brier ↓", "Log loss ↓", "Coverage"]).map(v => <th key={v}>{v}</th>)}</tr></thead><tbody>{scored.map(r => <tr key={key(r)}><td><button className={styles.textButton} onClick={() => setSelected(key(r))} aria-pressed={active === r}>{name(r.source)} · {track(r.track)}</button><small title={r.version}>{r.version.slice(0,28)}{r.version.length>28 ? "…" : ""}</small></td><td>{scope(r.target)}<small>{r.hours}h</small></td><td>{r.scores.n} / {r.scores.unique_events}</td><td>{number(r.scores.brier)}</td><td>{number(r.scores.log_loss)}</td><td>{percent(r.forecast_coverage)}</td></tr>)}</tbody></table></div>
      {active && <div className={styles.diagnostics}>
        <div><h3>{name(active.source)} · {active.hours}h · {zh ? "概率校准" : "Calibration"}</h3><p className={styles.caption}>{zh ? "横轴为预测均值，纵轴为实际发生比例；越接近对角线，概率与频率越一致。" : "Mean prediction on x; observed event frequency on y. The diagonal indicates agreement."}</p><svg viewBox="0 0 280 185" role="img" aria-label={zh ? "概率校准图" : "Probability calibration"}>
          <line x1="34" x2="252" y1="150" y2="150" /><line x1="34" x2="34" y1="18" y2="150" /><line x1="34" x2="252" y1="150" y2="18" strokeDasharray="4 4" />
          <text x="26" y="155" textAnchor="end">0</text><text x="26" y="23" textAnchor="end">100%</text><text x="34" y="176">0</text><text x="252" y="176" textAnchor="end">100%</text>
          {active.scores.calibration.map((c,i) => <circle key={i} className={styles.calibrationDot} cx={34+c.mean_probability*218} cy={150-c.event_frequency*132} r={Math.min(9,3+Math.sqrt(c.n))}><title>{percent(c.mean_probability)} → {percent(c.event_frequency)} · n={c.n}</title></circle>)}
        </svg></div>
        <div><h3>{zh ? "检验状态与预警质量" : "Evidence and alert quality"}</h3><dl><dt>{zh ? "Brier 的 95% 区间" : "Brier 95% interval"}</dt><dd>{active.scores.brier_interval.ci95?.map(v=>v.toFixed(3)).join(" – ") ?? (zh ? "等待至少 8 个周块" : "Awaiting at least 8 weekly blocks")}</dd><dt>{zh ? "预警次数 / 误报" : "Alerts / false alerts"}</dt><dd>{active.alerts.episodes} / {active.alerts.false_alerts}</dd><dt>{zh ? "预警精确率 / 事件召回率" : "Alert precision / event recall"}</dt><dd>{percent(active.alerts.precision)} / {percent(active.alerts.event_recall)}</dd><dt>{zh ? "提前量中位数" : "Median lead time"}</dt><dd>{number(active.alerts.median_lead_hours,1)} h</dd></dl><p className={styles.caption}>{zh ? `比较参考门槛：${report.policy.minimum_rank_windows} 个不重叠窗口、${report.policy.minimum_rank_events} 个独立事件。` : `Comparison threshold: ${report.policy.minimum_rank_windows} non-overlapping windows and ${report.policy.minimum_rank_events} distinct events.`}</p></div>
      </div>}
    </> : <div className={styles.warmup}><strong>{waitingTitle}</strong><p>{waitingDetail}</p></div>}
    <details className={styles.disclosure}><summary>{zh ? "来源对照与社区增量价值" : "Source comparisons and community value"}</summary><p>{zh ? "同一目标、同一窗口成对比较。损失差为左项减右项，负值表示左项在这些样本上更好；误差相关性用于检查重复信息。" : "Pairs use identical targets and windows. Loss difference is left minus right; negative values favor the left in these samples. Error correlation helps identify shared information."}</p>{report.comparisons.length ? <div className={styles.tableScroll}><table><thead><tr>{(zh ? ["成对比较", "范围 / 窗口", "样本", "Brier 差 / 95% 区间", "误差相关"] : ["Paired comparison", "Scope / horizon", "Samples", "Brier difference / 95% interval", "Error correlation"]).map(v => <th key={v}>{v}</th>)}</tr></thead><tbody>{report.comparisons.map((c,i) => <tr key={i}><td>{name(c.left.source)} – {name(c.right.source)}<small>{track(c.left.track)} / {track(c.right.track)}</small></td><td>{scope(c.target)} · {c.hours}h</td><td>{c.n}</td><td>{number(c.loss_difference.mean)}<small>{c.loss_difference.ci95?.map(v=>v.toFixed(3)).join(" – ") ?? (zh ? "区间待积累" : "Interval pending")}</small></td><td>{number(c.error_correlation)}</td></tr>)}</tbody></table></div> : <p>{zh ? "等待同窗成绩；已预先保存“移除社区”、历史基准与等权组合，供后续对照。" : "Awaiting matched results. Community-removed, historical-baseline and equal-weight variants are already saved for comparison."}</p>}</details>
    <div className={styles.heading}><h3>{zh ? "实际事件核验" : "Actual event verification"}</h3><span>{zh ? "完成、局部发放与分歧分别记录" : "Completion, targeted grants and disputes retained"}</span></div>
    <div className={styles.outcomes}>{recentOutcomes(report).map(e => <div key={e.id}><i data-state={e.status} /><div><strong>{e.kind === "banked" ? (zh ? "发放重置卡" : "New reset credit") : (zh ? "自动重置" : "Automatic reset")}</strong><span>{e.reason === "conditional_pledge" ? (zh ? "条件承诺跟踪" : "Tracking a conditional commitment") : outcomeLabels[e.status] ?? (zh ? "等待核验" : "Awaiting verification")}</span></div><time><small>{e.status === "confirmed" ? (zh ? "报告完成" : "Reported complete") : (zh ? "证据时间" : "Evidence time")}</small>{publicEvidenceTime(new Date((e.to ?? e.announced_at ?? e.known_at)*1000).toISOString(),language)}</time><div className={styles.evidenceLinks}>{e.references.slice(0,2).map((url,i) => <EvidenceLink key={url} url={url} label={zh ? `证据 ${i+1}` : `Evidence ${i+1}`} onOpen={onOpenSource} />)}</div></div>)}</div>
    <footer className={styles.footer}><div>{report.outcome_providers.map(p => <span key={p.provider}>{providers[p.provider] ?? (zh ? "事件渠道" : "Outcome feed")} · {p.healthy ? (zh ? "可用" : "Available") : (zh ? "待更新" : "Update due")}</span>)}</div><div><span>{zh ? "评估更新 " : "Evaluation updated "}{publicEvidenceTime(report.generated_at,language)}</span><span>{zh ? "前瞻记录开始 " : "Forward record began "}{publicEvidenceTime(report.started_at,language)}</span></div></footer>
  </article>;
}
