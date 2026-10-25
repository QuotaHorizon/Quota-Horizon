import { Activity, ArrowDownRight, Radio } from "lucide-react";
import type { Language } from "../../i18n";
import type { PublicReadReceipt, PublicResetTimeline } from "./types";
import type { CommentEstimate, QuantForecast, QuantitativeView } from "./quantitativeTypes";
import { communityEffect, controllingPoint, forecastFor, forecastStale, percent, points, precedesCompletion, probabilityAt, sourceColors, sourceNames, sourceRows, sourceUrls } from "./quantitativeModel";
import { forecastPresentation } from "./resetBriefingModel";
import { publicEvidenceTime } from "./presentation";
import { EvidenceLink } from "./ArchiveView";
import { TiboBriefing } from "./TiboBriefing";
import { CommunityForecasts } from "./CommunityForecasts";
import { CalendarSentiment } from "./CalendarSentiment";
import { ProbabilityCurve, QuantitativeTrend } from "./QuantitativeTrend";
import { ModuleRuntimeStatus } from "./ModuleRuntimeStatus";
import { QuantitativeEvaluation } from "./QuantitativeEvaluation";
import { ForecastChanges } from "./ForecastChanges";
import shared from "./resetBriefing.module.less";
import styles from "./quantitative.module.less";

export function QuantitativeOverview({ timeline, language, now, failed, onOpenSource, onRead }: {
  timeline: PublicResetTimeline; language: Language; now: number; failed?: boolean;
  onOpenSource?: (url: string) => void; onRead?: (receipts: PublicReadReceipt[]) => void;
}) {
  const zh = language === "zh", view = timeline.quantitative!;
  const f = forecastFor(view), auto = forecastFor(view, "automatic_reset"), stale = forecastStale(view,f,now);
  const outlook = forecastPresentation(timeline.insights,now,failed,timeline);
  const sources = sourceRows(view), sourceCount = new Set(view.forecast?.forecasts.flatMap(f => f.sources.filter(s => s.id !== "survival_baseline" && (s.weight ?? 1) > 0).map(s => s.id))).size;
  return <section className={`${shared.briefing} ${styles.overview}`} aria-label={zh ? "重置雷达综合概览" : "Reset radar overview"}>
    <div className={styles.heading}><span>{zh ? "重置或发卡 · 未来 24h / 48h" : "Reset or new credit · next 24h / 48h"}</span><a href="#forecast-evaluation"><Activity size={13} />{zh ? "查看预测评估" : "View evaluation"}<ArrowDownRight size={13} /></a></div>
    {view.forecastIssue && <p className={styles.notice} role="status">{zh ? "预测模块输出暂不可读，保留上次可用结果；后台会继续重读。" : "The model output is temporarily unavailable. Saved results are retained and the app will keep checking."}</p>}
    <div className={styles.topGrid}>
    <article className={styles.hero}>
      <div className={styles.heroCopy}>
        <div className={styles.byline}><Radio size={16} /><h2>{zh ? "Horizon 综合判断" : "Horizon outlook"}</h2></div>
        <div className={styles.probabilities}>{([24,48] as const).map(h => {
          const committed = !!outlook.announcement?.timing && outlook.announcement.timing.end <= now + h*3600_000;
          const p = outlook.awaitingResetForecast ? null : probabilityAt(f,h), effect = communityEffect(view.evaluation,f,h);
          return <div key={h}><span>{zh ? `未来 ${h} 小时` : `Next ${h} hours`}</span><strong>{percent(committed ? 1 : p)}</strong>
            {(outlook.awaitingResetForecast || committed || stale) && <b>{outlook.awaitingResetForecast ? (zh ? "等待重置后预测" : "Awaiting a post-reset forecast") : committed ? (zh ? "明确预告覆盖" : "Explicitly announced") : f?.curve.length ? (zh ? "上次预测 · 待更新" : "Saved forecast · update due") : (zh ? "等待预测输出" : "Awaiting forecast")}</b>}
            {committed && <small>{zh ? "模型估计 " : "Model estimate "}{percent(p)}</small>}
            <small>{zh ? "自动重置 " : "Automatic reset "}{percent(precedesCompletion(auto,outlook.completion) ? null : probabilityAt(auto,h))}{forecastStale(view,auto,now) && (zh ? " · 待更新" : " · update due")}</small>
            {!outlook.awaitingResetForecast && <small className={styles.accent}>{zh ? "新增投票补充 " : "Recent ballot effect "}{points(effect)} {zh ? "点" : "pts"}</small>}
          </div>;
        })}</div>
        <div className={styles.meta}><span>{sourceCount} {zh ? "家雷达参与" : "contributing radars"}</span><time dateTime={f?.issued_at}>{publicEvidenceTime(f?.issued_at,language)}</time></div>
        {!outlook.awaitingResetForecast && <details className={styles.breakdown}><summary>{zh ? "概率组成" : "Probability breakdown"}</summary><PoolWeights forecast={f} automatic={auto} language={language} /><span className={styles.meta}>{f?.event_count ?? 0} {zh ? "次历史事件" : "historical events"}</span></details>}
      </div>
      {outlook.awaitingResetForecast ? <p className={styles.notice}>{zh ? "预测更新中…" : "Updating forecast…"}</p> : <ProbabilityCurve view={view} language={language} />}
    </article>

    <TiboBriefing className={styles.intelligence} bodyClassName={styles.intelligenceBody} timeline={timeline} language={language} notice={outlook.notice} now={now} failed={failed} onOpenSource={onOpenSource} onRead={onRead}>
    {outlook.notice && <section className={styles.noticeProgress}><div className={styles.heading}><h3>{outlook.noticeWithdrawn ? (zh ? "预告已更正" : "Announcement corrected") : outlook.notice.state === "reported" ? (zh ? "上一轮重置已报告完成" : "Previous reset reported complete") : (zh ? "重置安排与进展" : "Reset timing and progress")}</h3><EvidenceLink url={outlook.notice.url} label={zh ? "查看原帖" : "Original post"} onOpen={onOpenSource} /></div><div className={shared.alignedTimes}>{outlook.notice.reportedAt && <div><span>{zh ? "报告完成" : "Reported complete"}</span><time>{publicEvidenceTime(new Date(outlook.notice.reportedAt).toISOString(),language)}</time></div>}{outlook.notice.timing && <div><span>{zh ? "预告时间" : "Scheduled"}</span><time>{publicEvidenceTime(new Date(outlook.notice.timing.end).toISOString(),language)}</time></div>}</div></section>}
    </TiboBriefing>
    <QuantitativeTrend view={view} language={language} now={now} />
    </div>
    <div className={styles.twoColumns}>
      <article className={styles.card}>
        <div className={styles.heading}><h2>{zh ? "各方怎样判断" : "How sources compare"}</h2><span>{zh ? "发布时原值" : "As published"}</span></div>
        <div className={styles.sourceHeader}><span>{zh ? "来源 / 事件范围" : "Source / scope"}</span><span>24h</span><span>48h</span></div>
        {sources.filter(s => s.state !== "excluded").map(s => <div className={styles.sourceRow} key={`${s.source}-${s.target}`}>
          <div><b style={{ color: sourceColors[s.source] }}><EvidenceLink url={sourceUrls[s.source]} label={sourceNames[s.source]} onOpen={onOpenSource} /></b><small>{s.target === "automatic_reset" ? (zh ? "自动重置" : "Automatic reset") : (zh ? "重置或发卡" : "Reset or new credit")} · {s.state === "cycle_closed" || (outlook.completion && Date.parse(s.origin) < Date.parse(outlook.completion.publishedAt)) ? (zh ? "等待重置后更新" : "Awaiting post-reset update") : s.response_state === "awaiting" ? (zh ? "新消息响应待确认" : "Response to new information unconfirmed") : Date.parse(s.valid_until) <= now ? (zh ? "待更新" : "Update due") : s.role === "background" ? (zh ? "历史背景" : "Historical background") : s.role === "comparison" ? (zh ? "来源对照 · 未提供基准拆分" : "Comparison · baseline split unavailable") : s.included ? (zh ? "信息修正" : "Information revision") : (zh ? "当前未纳入" : "Not currently admitted")}</small>{s.comment && <small>{zh ? "承诺判断 " : "Commitment estimate "}{percent(s.comment.probability)} · {s.comment.included ? (zh ? "已计入" : "Included") : s.comment.state === "closed_or_superseded" ? (zh ? "已结束" : "Ended") : (zh ? "待明确时间窗" : "Awaiting a timed window")}</small>}<time>{publicEvidenceTime(s.origin,language)}</time></div>
          {[24,48].map(h => { const p = s.probabilities_at_publication[h]; return <div key={h}><strong>{percent(p)}</strong><i className={styles.bar}>{p != null && <i style={{ width: `${p*100}%`, background: sourceColors[s.source] }} />}</i></div>; })}
        </div>)}
        {!sources.length && <p className={styles.caption}>{zh ? "评估器完成下一次导入后显示各来源原值。" : "Source values appear after the evaluator’s next import."}</p>}
        <details className={styles.disclosure}><summary>{zh ? "查看转换后的模型输入" : "Inspect aligned model inputs"}</summary>
          <p>{zh ? "上表保留各自发布时的窗口。下表为生成本次模型时转换的输入；来源支持范围之外由历史基准补足，并单独标记。" : "The comparison preserves each source’s publication windows. Inputs below are aligned to this model’s issue time; unsupported tails use the historical baseline and are marked."}</p>
          {view.forecast?.forecasts.map(f => <div key={f.target}><h3>{f.target === "automatic_reset" ? (zh ? "自动重置" : "Automatic reset") : (zh ? "重置或发卡" : "Reset or new credit")}</h3>{f.sources.map(s => <div key={s.id} className={styles.inputRow}><span>{sourceNames[s.id] ?? (zh ? "历史基准" : "Historical baseline")}</span><span>{percent(s.probabilities[23])} / {percent(s.probabilities[47])} · {zh ? "修正权重 " : "Revision weight "}{percent(s.weight)}</span>{s.baseline_weight != null && <small>{zh ? "历史起点权重 " : "Historical-anchor weight "}{percent(s.baseline_weight)}</small>}{s.baseline_points && <small>{zh ? "来源完整预测已包含信号调整；其历史基准为 " : "The complete forecast includes source adjustments; its historical baseline is "}{s.baseline_points.map(p => percent(p[1])).join(" / ")}</small>}<CommentEvidence comment={s.comment} language={language} onOpenSource={onOpenSource} />{s.source_coverage_hours != null && <small>{zh ? `原始曲线还支持 ${s.source_coverage_hours.toFixed(1)}h` : `Native support remaining: ${s.source_coverage_hours.toFixed(1)}h`}</small>}</div>)}</div>)}
          {sources.filter(s => s.state === "excluded").map(s => <p key={s.source}>{sourceNames[s.source]} · {zh ? "质量或口径审查未通过，保留观测但不纳入当前模型。" : "Quality or scope review pending; observations retained and excluded from the current model."}</p>)}
        </details>
      </article>
      <Community view={view} language={language} now={now} />
    </div>
    <ForecastChanges timeline={timeline} language={language} onOpenSource={onOpenSource} />
    {timeline.radar && <details className={styles.rawPolls}><summary>{zh ? "全部投票与日期分布" : "All ballots and date distributions"}</summary><CommunityForecasts radar={timeline.radar} language={language} now={now} onOpenSource={onOpenSource} independent /></details>}
    <QuantitativeEvaluation view={view} language={language} now={now} onOpenSource={onOpenSource} />
    <footer className={styles.footer}><ModuleRuntimeStatus view={view} language={language} /><details><summary>{zh ? "模型与评估方法" : "Model and evaluation methods"}</summary><p>{zh ? "历史起点只计入一次。来源提供的基准与完整预测成对转换，其正负修正按共有依据合并；只有历史值的来源保留为背景，未提供拆分的来源用于对照。来源单列的承诺判断按同一事件时间窗补足未覆盖差额。新增投票以当前判断为参照提供补充，按有效样本与先验的相对信息量确定权重，未知重叠按 50% 折减，按票数实际变化的时间每 2 小时减半，并计入剩余期限。广义恢复包含自动重置的概率下限。参数与范围传递规则等待前瞻检验。" : "The historical anchor enters once. Paired source baselines and complete forecasts supply signed revisions, pooled by shared evidence. History-only sources provide background; sources without a split remain comparisons. Separate source commitment estimates fill uncovered gaps within the matching event window. Recent ballots supplement the informed outlook: weight follows effective sample information relative to the prior, discounted by 50% for unknown overlap decayed with a two-hour half-life from observed ballot arrivals, and adjusted for remaining coverage. Broader relief inherits the automatic-reset probability floor. These assumptions await forward validation."}</p><p>{zh ? "评估器冻结当时可见的预测，独立核验实际事件，等待窗口到期及报告宽限后记分。比较采用相同事件范围与时间窗；缺失证据保留未决。当前成绩用于评估模型改进，评估器不会自行修改权重。" : "The evaluator freezes forecasts available at the time, independently checks outcomes, and scores after the horizon and reporting grace period. Comparisons match event scope and time windows; missing evidence remains unresolved. Evaluation informs model changes and does not itself modify weights."}</p><p>{f?.model_version} · {zh ? "权重训练窗口 " : "Weight-training windows "}{f?.evaluation_windows ?? 0}</p></details></footer>
  </section>;
}

function PoolWeights({ forecast, automatic, language }: { forecast: QuantForecast | undefined; automatic?: QuantForecast; language: Language }) {
  const zh = language === "zh", weights = forecast?.family_weights;
  if (forecast?.combination_method === "baseline_plus_revisions") return <div className={styles.incrementBreakdown} aria-label={zh ? "概率组成" : "Probability breakdown"}>
    <div><span>{zh ? "概率组成" : "Probability breakdown"}</span><b>24h</b><b>48h</b></div>
    {[
      {name: zh ? "历史起点" : "Historical anchor", key: "baseline", probability: true},
      {name: zh ? "来源修正" : "Source revisions", key: "radar_adjustment", probability: false},
      ...(forecast.curve.some(p => (p.comment_adjustment ?? 0) > 0) || automatic?.curve.some(p => (p.comment_adjustment ?? 0) > 0)
        ? [{name: zh ? "承诺判断补充" : "Commitment supplement", key: "comment_adjustment", probability: false}] : []),
      {name: zh ? "新增投票补充" : "Recent ballots", key: "community_adjustment", probability: false},
      {name: zh ? "范围与曲线一致性" : "Scope and curve adjustment", key: "coherence_adjustment", probability: false},
    ].map(row => <div key={row.key}><span>{row.name}</span>{[24,48].map(h => {
      const point = controllingPoint(forecast, automatic, h);
      const raw = point?.[row.key as "baseline" | "radar_adjustment" | "comment_adjustment" | "community_adjustment" | "coherence_adjustment"];
      const value = row.key === "radar_adjustment" && raw != null ? raw - (point?.comment_adjustment ?? 0) : raw;
      return <b key={h}>{row.probability ? percent(value) : `${points(value ?? null)} ${zh ? "点" : "pts"}`}</b>;
    })}</div>)}
    {forecast.curve.some(p => (p.coherence_adjustment ?? 0) > .00001) && <small>{zh ? "当前包含自动重置预期；上表展示主导判断的计算过程。" : "The outlook includes the automatic-reset forecast; the breakdown shows the controlling estimate."}</small>}
  </div>;
  if (!weights || !Object.keys(weights).length) return null;
  const families = [{ id: "baseline", name: zh ? "历史基准" : "Historical baseline", color: "var(--muted)" },
    { id: "radars", name: zh ? "雷达完整预测" : "Complete radar forecasts", color: "var(--radar-accent)" },
    { id: "community", name: zh ? "社区量化" : "Quantified community", color: "var(--quant-rose)" }];
  return <div className={styles.poolWeights} aria-label={zh ? "本次实际权重" : "Applied weights"}>
    <div className={styles.poolBar}>{families.map(f => <i key={f.id} style={{ width: `${(weights[f.id] ?? 0)*100}%`, background: f.color }} />)}</div>
    <div>{families.map(f => <span key={f.id}><i style={{ background: f.color }} />{f.name} <b>{percent(weights[f.id] ?? 0)}</b></span>)}</div>
  </div>;
}

function CommentEvidence({ comment, language, onOpenSource }: {
  comment?: CommentEstimate | null; language: Language; onOpenSource?: (url: string) => void;
}) {
  if (!comment?.timing_evidence?.length) return null;
  const zh = language === "zh";
  const evidence = [...new Map(comment.timing_evidence.map(e => [e.timing_url, e])).values()];
  return <small><EvidenceLink url={comment.url} label={zh ? "承诺原帖" : "Commitment post"} onOpen={onOpenSource} />
    {evidence.map(e => <span key={e.timing_url}> · <EvidenceLink url={e.timing_url}
      label={e.relationship === "direct_reply" ? (zh ? "关联回复与时间" : "Linked reply and timing") : (zh ? "事件时间" : "Event timing")}
      onOpen={onOpenSource} /> · {zh ? "截止 " : "By "}{publicEvidenceTime(new Date(e.deadline * 1000).toISOString(), language)}</span>)}
  </small>;
}

function Community({ view, language, now }: { view: QuantitativeView; language: Language; now: number }) {
  const zh = language === "zh", f = forecastFor(view), c = f?.community;
  return <article className={`${styles.card} ${styles.community}`}>
    <div className={styles.heading}><h2>{zh ? "社区预期" : "Community outlook"}</h2><span>{zh ? "重置或发卡" : "Reset or new credit"}</span></div>
    <div className={styles.communityNumbers}>{[24,48].map(h => <div key={h}><span>{h}h · {zh ? "社区预估" : "Community estimate"}</span><strong>{percent(c?.probabilities[h-1])}</strong><small>{zh ? "贡献 " : "Contribution "}<b>{points(communityEffect(view.evaluation,f,h))} {zh ? "点" : "pts"}</b></small></div>)}</div>
    <div className={styles.weight}><span>{zh ? "补充权重" : "Supplement weight"}</span><strong>{percent(c?.weight)}</strong><i><i style={{ width: `${(c?.weight ?? 0)*100}%` }} /></i></div>
    <CalendarSentiment report={view.evaluation} language={language} now={now} input={f?.community_inputs?.find(c => c.id === "didcodex_poll")} />
    <details className={styles.disclosure}><summary>{zh ? "投票输入与权重" : "Ballot inputs and weights"}</summary>
    <div className={styles.communityInputs}>{view.forecast?.forecasts.map(f => <section key={f.target}>
      <h3>{f.target === "automatic_reset" ? (zh ? "自动重置" : "Automatic reset") : (zh ? "重置或发卡" : "Reset or new credit")}</h3>
      {(f.community_inputs ?? []).map(c => <div className={styles.communityInput} key={c.id}>
        <div><b>{c.id === "didcodex_poll" ? "Did Codex Reset" : c.id === "date_poll" ? (zh ? "Codex Reset 日期投票" : "Codex Reset timing poll") : (zh ? "NextReset 概率投票" : "NextReset probability poll")}</b>
          <span>{c.recent_votes != null ? (zh ? `近 ${c.observation_window_hours?.toFixed(1) ?? "6"}h 新增 ${c.recent_votes} / 累计 ${c.total_votes ?? c.votes}` : `${c.recent_votes} new in ${c.observation_window_hours?.toFixed(1) ?? "6"}h / ${c.total_votes ?? c.votes} total`) : c.votes ?? "—"} {c.id === "didcodex_poll" ? (zh ? "次点击" : "clicks") : (zh ? "票" : "votes")}
            {c.mapping_kind ? (zh ? " · 已折减重复点击影响" : " · repeated-click sensitivity applied")
              : `${zh ? " · 有效样本 " : " · Effective sample "}${c.effective_votes?.toFixed(1) ?? "—"}`}</span>
          {c.latest_vote_at != null && <small>{zh ? "最近检测到新增票：" : "Latest new ballots observed: "}{publicEvidenceTime(new Date(c.latest_vote_at * 1000).toISOString(), language)}</small>}
          <small>{zh ? "原期限还剩 " : "Original window remaining: "}{c.coverage_hours?.toFixed(1) ?? "—"}h{c.coverage_hours != null && c.coverage_hours < 48 ? c.mapping_kind ? (zh ? " · 仅修正该时段" : " · adjustment scoped to this window") : (zh ? " · 超出期限接历史基准" : " · baseline after deadline") : ""}</small>
          {c.terminal_probability != null && c.window_end != null && <small>{zh ? "原期限内预估 " : "Estimate by original deadline "}{percent(c.terminal_probability)} · <time dateTime={new Date(c.window_end * 1000).toISOString()}>{publicEvidenceTime(new Date(c.window_end * 1000).toISOString(), language)}</time></small>}</div>
        <div><strong>{percent(c.weight)}</strong><small>{zh ? "实际权重" : "Applied weight"}</small><span>24h {percent(c.probability_24h)}<br />48h {percent(c.probability_48h)}</span></div>
      </div>)}
      {f.community_coverage && <p className={styles.coverage}>{zh ? `${f.community_coverage.included} 项有效 · ${f.community_coverage.unavailable} 项待新数据或质量达标` : `${f.community_coverage.included} eligible · ${f.community_coverage.unavailable} awaiting fresh or qualifying data`}
        {f.community_coverage.supplementary_checked > 0 && <span>{zh ? `补充社区指标：${f.community_coverage.supplementary_included}/${f.community_coverage.supplementary_checked} 项纳入，权重 ` : `Supplementary community indicators: ${f.community_coverage.supplementary_included}/${f.community_coverage.supplementary_checked} admitted, weight `}{percent(f.community_coverage.supplementary_weight)}</span>}</p>}
    </section>)}</div></details>
    {(!c || forecastStale(view,f,now)) && <div className={styles.communityStatus}>{!c ? (zh ? "本次暂无可直接计入概率的近期投票。" : "No recent ballots currently qualify as a probability input.") : forecastStale(view,f,now) ? (zh ? "上次社区结果 · 等待模块更新" : "Saved community result · awaiting an update") : (zh ? "已参与本次综合判断" : "Included in this combined forecast")}</div>}
  </article>;
}
