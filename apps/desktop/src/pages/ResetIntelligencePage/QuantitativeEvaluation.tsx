import { useState } from "react";
import type { Language } from "../../i18n";
import type { QuantitativeView } from "./quantitativeTypes";
import { evaluationStale, percent, recentOutcomes, sourceNames } from "./quantitativeModel";
import { publicEvidenceTime } from "./presentation";
import { evaluationRows, evaluationPage, evaluationVersion, scorecardKey } from "./evaluationModel";
import { EvidenceLink } from "./ArchiveView";
import { ReactionEvaluation } from "./ReactionEvaluation";
import styles from "./quantitative.module.less";

export function QuantitativeEvaluation({ view, language, now, onOpenSource }: {
  view: QuantitativeView; language: Language; now: number; onOpenSource?: (url: string) => void;
}) {
  const zh = language === "zh", report = view.evaluation;
  const [selected, setSelected] = useState("");
  const [history, setHistory] = useState(false);
  const [source, setSource] = useState("");
  const [page, setPage] = useState(0), [comparisonPage, setComparisonPage] = useState(0);
  if (!report) return <article id="forecast-evaluation" className={styles.card}><h2>{zh ? "预测评估" : "Forecast evaluation"}</h2><p className={styles.caption}>{view.evaluationIssue ? (zh ? "评估报告读取失败，后台会自动重试。" : "The evaluation report could not be read. Retrying automatically in the background.") : (zh ? "等待独立评估器输出。" : "Waiting for the independent evaluator.")}</p></article>;
  const stale = evaluationStale(view,now), s = report.summary;
  const name = (id: string) => sourceNames[id] ?? ({ survival_baseline: zh ? "历史基准" : "Historical baseline", without_community: zh ? "移除社区" : "Without community", without_response_cohort: zh ? "移除消息响应分组" : "Without response grouping", without_comment_estimates: zh ? "移除独立承诺判断" : "Without separate commitment estimates", without_information: zh ? "移除来源修正" : "Without source revisions", without_radars: zh ? "移除雷达" : "Without radars", without_baseline: zh ? "移除历史基准" : "Without baseline", equal_components: zh ? "各分量等权" : "Equal component weights" }[id] ?? (zh ? "对照模型" : "Comparison model"));
  const scope = (t: string) => t === "automatic_reset" ? (zh ? "自动重置" : "Automatic reset") : (zh ? "重置或发卡" : "Reset or credit");
  const track = (t: string) => ({ native: zh ? "来源原值" : "Native", adapted: zh ? "转换输入" : "Aligned", model: zh ? "综合模型" : "Combined", shadow: zh ? "对照模型" : "Ablation" }[t]);
  const number = (n: number | null, digits = 3) => n == null ? "—" : n.toFixed(digits);
  const rows = evaluationRows(view, history, source);
  const cardsPage = evaluationPage(rows.cards, page), pairsPage = evaluationPage(rows.comparisons, comparisonPage);
  const displayed = cardsPage.items;
  const active = displayed.find(r => scorecardKey(r) === selected) ?? displayed.find(r => r.track === "model") ?? displayed[0];
  const resetSelection = () => { setSelected(""); setPage(0); setComparisonPage(0); };
  const pages = (current: number, total: number, change: (n: number) => void) => total > 1 && <nav className={styles.evaluationPages} aria-label={zh ? "评估分页" : "Evaluation pages"}>
    <button onClick={() => { change(current - 1); setSelected(""); }} disabled={current === 0}>{zh ? "上一页" : "Previous"}</button>
    <span>{current + 1} / {total}</span>
    <button onClick={() => { change(current + 1); setSelected(""); }} disabled={current + 1 === total}>{zh ? "下一页" : "Next"}</button>
  </nav>;
  const outcomeLabels: Record<string, string> = { confirmed: zh ? "完成有据" : "Completion evidenced", pending: zh ? "等待完成证据" : "Awaiting completion evidence", targeted: zh ? "局部补偿" : "Targeted compensation", disputed: zh ? "时间或范围有分歧" : "Timing or scope disputed" };
  const providers: Record<string,string> = { completion_feed: "Codex Reset FM", completion_page: "Codex Reset FM", public_catalogue: "Codex Resets", quota_events: "QuotaResets", codex_reset_status: "Codex Reset", didcodex_status: "Did Codex Reset" };
  const providerStates: Record<string,string> = {
    healthy: zh ? "可用" : "Available",
    positive_evidence: zh ? "完成证据" : "Completion evidence",
    collector_overdue: zh ? "采集已逾期" : "Collection overdue",
    request_or_decode_failure: zh ? "读取失败" : "Read failed",
    stale_source_content: zh ? "来源内容已过期" : "Source content stale",
    incomplete_source_evidence: zh ? "来源证据不完整" : "Source evidence incomplete",
  };
  const unresolved = (s.settlements.unresolved ?? 0) + (s.settlements.disputed ?? 0);
  const progress = s.scoring_by_horizon;
  const scoredWindows = progress?.reduce((n, p) => n + p.scored_windows, 0) ?? 0;
  const rollingTrials = progress?.reduce((n, p) => n + p.eligible_rolling_trials, 0) ?? 0;
  const settledWindows = progress?.reduce((n, p) => n + p.settled_windows, 0) ?? 0;
  const versionWaiting = !history && (source !== "" || report.scorecards.some(r => r.scores.n > 0));
  const waitingTitle = versionWaiting ? (zh ? "当前版本正在积累独立成绩" : "Current versions are collecting their own results") : rollingTrials > 0
    ? (zh ? "已有滚动窗口结算，等待不重叠评分窗口" : "Outcomes settled; awaiting non-overlapping scoring windows")
    : settledWindows > 0 ? (zh ? "事件窗口已结算，对应时点没有可用预测" : "Outcomes settled; no eligible forecast at those origins")
    : unresolved > 0 ? (zh ? "已有检验窗到期，证据尚不足以记分" : "Some windows have matured; evidence is still insufficient to score")
    : (zh ? "正在积累第一批前瞻成绩" : "Collecting the first forward results");
  const waitingDetail = versionWaiting ? (zh ? "新版本的预测会在对应窗口到期后独立记分；已有版本的结果可在历史成绩中查看。" : "New forecasts receive scores after their own windows mature. Earlier versions remain available in historical results.") : rollingTrials > 0
    ? (zh ? "滚动窗口已经用于逐时核验；主成绩仅采用预先固定的 UTC 不重叠窗口，以免重复放大同一时段。" : "Rolling windows are verified hourly. Headline scores use predeclared non-overlapping UTC windows to avoid counting the same period repeatedly.")
    : settledWindows > 0 ? (zh ? "实际结果已知，但这些窗口起点的预测缺失、过期或不覆盖相应期限，保留缺口。" : "Outcomes are known, but forecasts at these origins were missing, expired or outside their supported horizons. Those gaps remain unscored.")
    : unresolved > 0 ? (zh ? "部分窗口的事件观测存在缺口或分歧，暂不计入成绩。采集和核验仍在继续；证据充分的窗口会自动显示 Brier、对数损失、校准与误报情况。" : "Some windows have gaps or conflicts in outcome observations and remain unscored. Collection and verification continue; windows with sufficient evidence automatically receive Brier, log loss, calibration and false-alert results.")
    : (zh ? "预测已冻结。事件发生后自动核验，待对应窗口到期且观测覆盖充分后，显示 Brier、对数损失、校准与误报情况。" : "Forecasts are frozen. Events are checked automatically; Brier, log loss, calibration and false alerts appear after the corresponding windows mature with sufficient observation coverage.");
  return <article id="forecast-evaluation" className={styles.card}>
    <div className={styles.heading}><h2>{zh ? "预测评估" : "Forecast evaluation"}</h2><span>{stale ? (zh ? "评估输出待更新" : "Evaluation update due") : publicEvidenceTime(report.generated_at,language)}</span></div>
    <div className={styles.evaluationStats}><div><strong>{s.forecast_snapshots}</strong><span>{zh ? "预测快照" : "Forecast snapshots"}</span></div><div><strong>{s.frozen_trials}</strong><span>{zh ? "已冻结检验" : "Frozen trials"}</span></div><div><strong>{s.scored_nonoverlapping_trials}</strong><span>{zh ? "已评分" : "Scored trials"}</span></div><div><strong>{unresolved}</strong><span>{zh ? "待补足证据" : "Evidence pending"}</span></div></div>
    <p className={styles.caption}>{s.next_maturity ? (zh ? "下次可结算 " : "Next eligible settlement ") + publicEvidenceTime(s.next_maturity,language) : (zh ? "等待下一个检验窗。" : "Awaiting the next test window.")}</p>
    {stale && <p className={styles.notice} role="status">{zh ? "评估器最近未更新，以下为保存的报告。" : "The evaluator has not updated recently. Showing the saved report."}</p>}
    <details className={styles.disclosure}><summary>{zh ? "成绩、事件与反应速度" : "Scores, events and response times"}</summary>
    <p>{zh ? `窗口到期 ${report.policy.reporting_grace_seconds/3600}h 后开始结算。记录含不同来源和期限，独立事件数见各项成绩。` : `Settlement starts ${report.policy.reporting_grace_seconds/3600}h after each window ends. Records span multiple sources and horizons; each score lists its distinct events.`}</p>
    {progress && <details className={styles.disclosure}><summary>{zh ? `${s.scored_nonoverlapping_trials} 条评分，覆盖 ${scoredWindows} 个目标时间窗` : `${s.scored_nonoverlapping_trials} scores across ${scoredWindows} target-time windows`}</summary>
      <p>{zh ? "不同来源与对照模型共享同一时间窗；按目标和起止时间去重，窗口数不等于实际重置次数。滚动核验包含主评分窗口。" : "Different sources and variants share the same windows. Windows are deduplicated by target and time, not counted as reset events. Rolling checks include headline scoring windows."}</p>
      <div className={styles.tableScroll}><table><thead><tr>{(zh ? ["期限", "已结算窗口", "主评分窗口", "主评分条数", "滚动核验条数", "已结算但无预测"] : ["Horizon", "Settled windows", "Scored windows", "Scores", "Rolling checks", "Settled without forecast"]).map(h => <th key={h}>{h}</th>)}</tr></thead><tbody>{progress.map(p => <tr key={p.hours}><td>{p.hours}h</td><td>{p.settled_windows}</td><td>{p.scored_windows}</td><td>{p.scored_trials}</td><td>{p.eligible_rolling_trials}</td><td>{p.missing_forecast_trials}</td></tr>)}</tbody></table></div>
    </details>}
    <div className={styles.evaluationControls}>
      <div role="group" aria-label={zh ? "评估版本" : "Evaluation versions"}>
        <button aria-pressed={!history} onClick={() => { setHistory(false); setSource(""); resetSelection(); }}>{zh ? "当前版本" : "Current versions"}</button>
        <button aria-pressed={history} onClick={() => { setHistory(true); setSource(""); resetSelection(); }}>{zh ? "历史成绩" : "Historical results"}</button>
      </div>
      <label>{zh ? "来源" : "Source"} <select value={source} onChange={e => { setSource(e.target.value); resetSelection(); }}>
        <option value="">{zh ? "全部" : "All"}</option>{rows.sources.map(id => <option key={id} value={id}>{name(id)}</option>)}
      </select></label>
      <span>{zh ? `${rows.cards.length} 项成绩` : `${rows.cards.length} scorecards`}</span>
    </div>
    {!history && <p className={styles.caption}>{zh ? "按当前来源与模型版本列出 24h / 48h 成绩；新版等待自己的检验窗口。" : "24h / 48h results for current source and model versions. New versions await their own test windows."}</p>}
    {!rows.cards.some(r => r.scores.n > 0) && <div className={styles.warmup}><strong>{waitingTitle}</strong><p>{waitingDetail}</p></div>}
    {displayed.length ? <>
      <div className={styles.tableScroll}><table><thead><tr>{(zh ? ["来源 / 版本", "范围 / 窗口", "窗口 / 事件", "Brier ↓", "对数损失 ↓", "预测覆盖", "补充事件核验"] : ["Source / version", "Scope / horizon", "Windows / events", "Brier ↓", "Log loss ↓", "Coverage", "Additional event checks"]).map(v => <th key={v}>{v}</th>)}</tr></thead><tbody>{displayed.map(r => <tr key={scorecardKey(r)}><td><button className={styles.textButton} onClick={() => setSelected(scorecardKey(r))} aria-pressed={active === r}>{name(r.source)} · {track(r.track)}</button><small title={r.version}>{evaluationVersion(r.version)}</small></td><td>{scope(r.target)}<small>{r.hours}h</small></td><td>{r.scores.n} / {r.scores.unique_events}</td><td>{number(r.scores.brier)}</td><td>{number(r.scores.log_loss)}</td><td>{percent(r.forecast_coverage)}</td><td>{r.observed_event_scores?.n ? <>{r.observed_event_scores.unique_events} {zh ? "个事件" : "events"}<small>Brier {number(r.observed_event_scores.brier)}</small></> : "—"}</td></tr>)}</tbody></table></div>
      {displayed.some(r => (r.observed_event_scores?.n ?? 0) > 0) && <p className={styles.caption}>{zh ? "补充核验记录观测有缺口、但已有完成证据的事件误差；主评分与校准使用观测完整的窗口。" : "Additional checks measure errors on evidenced events with monitoring gaps. Main scores and calibration use fully monitored windows."}</p>}
      {pages(cardsPage.page, cardsPage.pages, setPage)}
      {active && !active.scores.n && <p className={styles.caption}>{name(active.source)} · {evaluationVersion(active.version)} · {scope(active.target)} · {active.hours}h — {zh ? "等待该版本的完整评分窗口" : "Awaiting complete scoring windows for this version"}</p>}
      {active && active.scores.n > 0 && <div className={styles.diagnostics}>
        <div><h3>{name(active.source)} · {evaluationVersion(active.version)} · {scope(active.target)} · {active.hours}h · {zh ? "概率校准" : "Calibration"}</h3><p className={styles.caption}>{zh ? "横轴为预测均值，纵轴为实际发生比例；越接近对角线，概率与频率越一致。" : "Mean prediction on x; observed event frequency on y. The diagonal indicates agreement."}</p><svg viewBox="0 0 280 185" role="img" aria-label={zh ? "概率校准图" : "Probability calibration"}>
          <line x1="34" x2="252" y1="150" y2="150" /><line x1="34" x2="34" y1="18" y2="150" /><line x1="34" x2="252" y1="150" y2="18" strokeDasharray="4 4" />
          <text x="26" y="155" textAnchor="end">0</text><text x="26" y="23" textAnchor="end">100%</text><text x="34" y="176">0</text><text x="252" y="176" textAnchor="end">100%</text>
          {active.scores.calibration.map((c,i) => <circle key={i} className={styles.calibrationDot} cx={34+c.mean_probability*218} cy={150-c.event_frequency*132} r={Math.min(9,3+Math.sqrt(c.n))}><title>{`${percent(c.mean_probability)} → ${percent(c.event_frequency)} · n=${c.n}`}</title></circle>)}
        </svg></div>
        <div><h3>{zh ? "检验状态与预警质量" : "Evidence and alert quality"}</h3><dl><dt>{zh ? "Brier 的 95% 区间" : "Brier 95% interval"}</dt><dd>{active.scores.brier_interval.ci95?.map(v=>v.toFixed(3)).join(" – ") ?? (zh ? "等待至少 8 个周块" : "Awaiting at least 8 weekly blocks")}</dd><dt>{zh ? "预警次数 / 误报" : "Alerts / false alerts"}</dt><dd>{active.alerts.episodes} / {active.alerts.false_alerts}</dd><dt>{zh ? "预警精确率 / 事件召回率" : "Alert precision / event recall"}</dt><dd>{percent(active.alerts.precision)} / {percent(active.alerts.event_recall)}</dd><dt>{zh ? "提前量中位数" : "Median lead time"}</dt><dd>{number(active.alerts.median_lead_hours,1)} h</dd></dl><p className={styles.caption}>{zh ? `比较参考门槛：${report.policy.minimum_rank_windows} 个不重叠窗口、${report.policy.minimum_rank_events} 个独立事件。` : `Comparison threshold: ${report.policy.minimum_rank_windows} non-overlapping windows and ${report.policy.minimum_rank_events} distinct events.`}</p></div>
      </div>}
    </> : <p className={styles.caption}>{zh ? "此筛选下暂无成绩。" : "No scorecards for this selection."}</p>}
    <details className={styles.disclosure}><summary>{zh ? "来源对照与社区增量价值" : "Source comparisons and community value"}</summary><p>{zh ? "同一目标、同一窗口成对比较。损失差为左项减右项，负值表示左项在这些样本上更好；误差相关性用于检查重复信息。" : "Pairs use identical targets and windows. Loss difference is left minus right; negative values favor the left in these samples. Error correlation helps identify shared information."}</p>{rows.comparisons.length ? <><div className={styles.tableScroll}><table><thead><tr>{(zh ? ["成对比较", "范围 / 窗口", "样本", "Brier 差 / 95% 区间", "误差相关"] : ["Paired comparison", "Scope / horizon", "Samples", "Brier difference / 95% interval", "Error correlation"]).map(v => <th key={v}>{v}</th>)}</tr></thead><tbody>{pairsPage.items.map((c,i) => <tr key={i}><td>{name(c.left.source)} – {name(c.right.source)}<small>{track(c.left.track)} / {track(c.right.track)}</small><small>{evaluationVersion(c.left.version)} / {evaluationVersion(c.right.version)}</small></td><td>{scope(c.target)} · {c.hours}h</td><td>{c.n}</td><td>{number(c.loss_difference.mean)}<small>{c.loss_difference.ci95?.map(v=>v.toFixed(3)).join(" – ") ?? (zh ? "区间待积累" : "Interval pending")}</small></td><td>{number(c.error_correlation)}</td></tr>)}</tbody></table></div>{pages(pairsPage.page, pairsPage.pages, setComparisonPage)}</> : <p>{zh ? "等待同窗成绩；已预先保存“移除社区”、历史基准与等权组合，供后续对照。" : "Awaiting matched results. Community-removed, historical-baseline and equal-weight variants are already saved for comparison."}</p>}</details>
    <ReactionEvaluation view={view} language={language} />
    <div className={styles.heading}><h3>{zh ? "实际事件核验" : "Actual event verification"}</h3></div>
    <div className={styles.outcomes}>{recentOutcomes(report).map(e => <div key={e.id}><i data-state={e.status} /><div><strong>{e.kind === "banked" ? (zh ? "发放重置卡" : "New reset credit") : (zh ? "自动重置" : "Automatic reset")}</strong><span>{e.reason === "conditional_pledge" ? (zh ? "条件承诺跟踪" : "Tracking a conditional commitment") : e.status === "pending" && e.completion_reports > 0 ? e.reason === "insufficient_primary_confirmation" ? (zh ? "已报告完成 · 等待交叉核验" : "Completion reported · awaiting corroboration") : (zh ? "已报告完成 · 覆盖范围待核验" : "Completion reported · scope under review") : outcomeLabels[e.status] ?? (zh ? "等待核验" : "Awaiting verification")}</span></div><time><small>{e.status === "confirmed" ? (zh ? "报告完成" : "Reported complete") : (zh ? "证据时间" : "Evidence time")}</small>{publicEvidenceTime(new Date((e.to ?? e.announced_at ?? e.known_at)*1000).toISOString(),language)}</time><div className={styles.evidenceLinks}>{e.references.slice(0,2).map((url,i) => <EvidenceLink key={url} url={url} label={zh ? `证据 ${i+1}` : `Evidence ${i+1}`} onOpen={onOpenSource} />)}</div></div>)}</div>
    <footer className={styles.footer}><div>{report.outcome_providers.map(p => <span key={p.provider}>{providers[p.provider] ?? (zh ? "事件渠道" : "Outcome feed")} · {p.coverage_role === "positive_only" && p.state === "incomplete_source_evidence"
      ? (zh ? "完成证据" : "Completion evidence")
      : providerStates[p.state ?? ""] ?? (p.healthy ? (zh ? "可用" : "Available") : (zh ? "待更新" : "Update due"))}</span>)}</div><div><span>{zh ? "评估更新 " : "Evaluation updated "}{publicEvidenceTime(report.generated_at,language)}</span><span>{zh ? "前瞻记录开始 " : "Forward record began "}{publicEvidenceTime(report.started_at,language)}</span></div></footer>
    </details>
  </article>;
}
