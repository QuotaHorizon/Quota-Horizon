import { Activity, ArrowDownRight, Radio } from "lucide-react";
import type { Language } from "../../i18n";
import type { PublicReadReceipt, PublicResetTimeline } from "./types";
import type { QuantitativeView } from "./quantitativeTypes";
import { communityEffect, forecastFor, forecastStale, percent, points, probabilityAt, sourceColors, sourceNames, sourceRows, sourceUrls } from "./quantitativeModel";
import { forecastPresentation } from "./resetBriefingModel";
import { publicEvidenceTime } from "./presentation";
import { EvidenceLink } from "./ArchiveView";
import { TiboBriefing } from "./TiboBriefing";
import { CommunityForecasts } from "./CommunityForecasts";
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
  const sources = sourceRows(view), sourceCount = new Set(view.forecast?.forecasts.flatMap(f => f.sources.filter(s => s.id !== "survival_baseline").map(s => s.id))).size;
  return <section className={`${shared.briefing} ${styles.overview}`} aria-label={zh ? "重置雷达综合概览" : "Reset radar overview"}>
    <div className={styles.heading}><span>{zh ? "重置或发卡 · 未来 24h / 48h" : "Reset or new credit · next 24h / 48h"}</span><a href="#forecast-evaluation"><Activity size={13} />{zh ? "查看预测评估" : "View evaluation"}<ArrowDownRight size={13} /></a></div>
    {view.forecastIssue && <p className={styles.notice} role="status">{zh ? "预测模块输出暂不可读，保留上次可用结果；后台会继续重读。" : "The model output is temporarily unavailable. Saved results are retained and the app will keep checking."}</p>}
    <div className={styles.topGrid}>
    <article className={styles.hero}>
      <div className={styles.heroCopy}>
        <div className={styles.byline}><Radio size={16} /><h2>{zh ? "Horizon 综合判断" : "Horizon outlook"}</h2><span>{zh ? "独立预测模块" : "Independent model"}</span></div>
        <div className={styles.probabilities}>{([24,48] as const).map(h => {
          const committed = !!outlook.announcement?.timing && outlook.announcement.timing.end <= now + h*3600_000;
          const p = probabilityAt(f,h), effect = communityEffect(view.evaluation,f,h);
          return <div key={h}><span>{zh ? `未来 ${h} 小时` : `Next ${h} hours`}</span><strong>{percent(committed ? 1 : p)}</strong>
            <b>{committed ? (zh ? "明确预告覆盖" : "Explicitly announced") : stale ? (f?.curve.length ? (zh ? "上次预测 · 待更新" : "Saved forecast · update due") : (zh ? "等待预测输出" : "Awaiting forecast")) : p != null && p >= .65 ? (zh ? "近期预期偏乐观" : "Optimistic near-term outlook") : p != null && p >= .4 ? (zh ? "保持关注" : "Watch closely") : (zh ? "近期预期偏低" : "Lower near-term outlook")}</b>
            {committed && <small>{zh ? "模型估计 " : "Model estimate "}{percent(p)}</small>}
            <small>{zh ? "自动重置 " : "Automatic reset "}{percent(probabilityAt(auto,h))}{forecastStale(view,auto,now) && (zh ? " · 待更新" : " · update due")}</small>
            <small className={styles.accent}>{zh ? "社区贡献 " : "Community effect "}{points(effect)} {zh ? "点" : "pts"}</small>
          </div>;
        })}</div>
        <p className={styles.reason}>{outlook.announcement ? (zh ? "明确预告优先，模型结果继续保留供评估" : "The announcement leads; model estimates remain available for evaluation") : (zh ? "把多方预测与社区预期放在同一概率基准上" : "Combining source forecasts and community expectations on a common probability basis")}</p>
        <div className={styles.meta}><span>{sourceCount} {zh ? "家雷达纳入模型" : "radars admitted"}</span><span>{f?.event_count ?? 0} {zh ? "次历史事件" : "historical events"}</span><span>{f?.training_state === "learned" ? (zh ? "使用学习权重" : "Fitted weights") : (zh ? "初始权重 · 积累前瞻成绩" : "Initial weights · collecting forward results")}</span></div>
        <p className={styles.caption}>{zh ? "预测生成 " : "Forecast issued "}{publicEvidenceTime(f?.issued_at,language)}</p>
      </div>
      <ProbabilityCurve view={view} language={language} />
    </article>

    <div className={styles.intelligence}>
    <TiboBriefing timeline={timeline} language={language} notice={outlook.notice} now={now} failed={failed} onOpenSource={onOpenSource} onRead={onRead} />
    {outlook.notice && <article className={styles.card}><div className={styles.heading}><h3>{outlook.noticeWithdrawn ? (zh ? "预告已更正" : "Announcement corrected") : outlook.notice.state === "reported" ? (zh ? "上一轮重置已报告完成" : "Previous reset reported complete") : (zh ? "重置安排与进展" : "Reset timing and progress")}</h3><EvidenceLink url={outlook.notice.url} label={zh ? "查看原帖" : "Original post"} onOpen={onOpenSource} /></div><div className={shared.alignedTimes}>{outlook.notice.reportedAt && <div><span>{zh ? "报告完成" : "Reported complete"}</span><time>{publicEvidenceTime(new Date(outlook.notice.reportedAt).toISOString(),language)}</time></div>}{outlook.notice.timing && <div><span>{zh ? "预告时间" : "Scheduled"}</span><time>{publicEvidenceTime(new Date(outlook.notice.timing.end).toISOString(),language)}</time></div>}</div></article>}
    </div>
    <QuantitativeTrend view={view} language={language} now={now} />
    </div>
    <ForecastChanges timeline={timeline} language={language} onOpenSource={onOpenSource} />
    <div className={styles.twoColumns}>
      <article className={styles.card}>
        <div className={styles.heading}><h2>{zh ? "各方怎样判断" : "How sources compare"}</h2><span>{zh ? "发布时原值" : "As published"}</span></div>
        <div className={styles.sourceHeader}><span>{zh ? "来源 / 事件范围" : "Source / scope"}</span><span>24h</span><span>48h</span></div>
        {sources.filter(s => s.state !== "excluded").map(s => <div className={styles.sourceRow} key={`${s.source}-${s.target}`}>
          <div><b style={{ color: sourceColors[s.source] }}><EvidenceLink url={sourceUrls[s.source]} label={sourceNames[s.source]} onOpen={onOpenSource} /></b><small>{s.target === "automatic_reset" ? (zh ? "自动重置" : "Automatic reset") : (zh ? "重置或发卡" : "Reset or new credit")} · {Date.parse(s.valid_until) <= now ? (zh ? "待更新" : "Update due") : s.included ? (zh ? "已纳入" : "Admitted") : (zh ? "当前未纳入" : "Not currently admitted")}</small><time>{publicEvidenceTime(s.origin,language)}</time></div>
          {[24,48].map(h => { const p = s.probabilities_at_publication[h]; return <div key={h}><strong>{percent(p)}</strong><i className={styles.bar}>{p != null && <i style={{ width: `${p*100}%`, background: sourceColors[s.source] }} />}</i></div>; })}
        </div>)}
        {!sources.length && <p className={styles.caption}>{zh ? "评估器完成下一次导入后显示各来源原值。" : "Source values appear after the evaluator’s next import."}</p>}
        <details className={styles.disclosure}><summary>{zh ? "查看转换后的模型输入" : "Inspect aligned model inputs"}</summary>
          <p>{zh ? "上表保留各自发布时的窗口。下表为生成本次模型时转换的输入；来源支持范围之外由历史基准补足，并单独标记。" : "The comparison preserves each source’s publication windows. Inputs below are aligned to this model’s issue time; unsupported tails use the historical baseline and are marked."}</p>
          {view.forecast?.forecasts.map(f => <div key={f.target}><h3>{f.target === "automatic_reset" ? (zh ? "自动重置" : "Automatic reset") : (zh ? "重置或发卡" : "Reset or new credit")}</h3>{f.sources.map(s => <div key={s.id} className={styles.inputRow}><span>{sourceNames[s.id] ?? (zh ? "历史基准" : "Historical baseline")}</span><span>{percent(s.probabilities[23])} / {percent(s.probabilities[47])}</span>{s.source_coverage_hours != null && <small>{zh ? `原始曲线还支持 ${s.source_coverage_hours.toFixed(1)}h` : `Native support remaining: ${s.source_coverage_hours.toFixed(1)}h`}</small>}</div>)}</div>)}
          {sources.filter(s => s.state === "excluded").map(s => <p key={s.source}>{sourceNames[s.source]} · {zh ? "质量或口径审查未通过，保留观测但不纳入当前模型。" : "Quality or scope review pending; observations retained and excluded from the current model."}</p>)}
        </details>
      </article>
      <Community view={view} language={language} now={now} />
    </div>
    {timeline.radar && <details className={styles.rawPolls}><summary>{zh ? "查看社区原始投票 · 日期分布、票数与每日走势" : "Original community ballots · dates, counts and daily trends"}</summary><CommunityForecasts radar={timeline.radar} language={language} now={now} onOpenSource={onOpenSource} independent /></details>}
    <QuantitativeEvaluation view={view} language={language} now={now} onOpenSource={onOpenSource} />
    <footer className={styles.footer}><ModuleRuntimeStatus view={view} language={language} /><span>{zh ? "Horizon 约每分钟读取模块输出。" : "Horizon checks module outputs about once a minute."}</span><details><summary>{zh ? "模型与评估方法" : "Model and evaluation methods"}</summary><p>{zh ? "历史事件提供分段风险率基准。雷达先检查事件范围与时效，再按共有历史归组；社区按样本和有效期限转换，作为独立信息组分配权重。两类事件的概率保持包含关系。" : "A piecewise hazard baseline uses historical events. Radars are checked for scope and freshness, then grouped by shared history. Community inputs are converted using sample size and deadlines, with weight assigned at group level. Automatic resets remain a subset of broader relief."}</p><p>{zh ? "评估器冻结当时可见的预测，独立核验实际事件，等待窗口到期及报告宽限后记分。比较采用相同事件范围与时间窗；缺失证据保留未决。当前成绩用于评估模型改进，评估器不会自行修改权重。" : "The evaluator freezes forecasts available at the time, independently checks outcomes, and scores after the horizon and reporting grace period. Comparisons match event scope and time windows; missing evidence remains unresolved. Evaluation informs model changes and does not itself modify weights."}</p><p>{f?.model_version} · {zh ? "权重训练窗口 " : "Weight-training windows "}{f?.evaluation_windows ?? 0}</p></details></footer>
  </section>;
}

function Community({ view, language, now }: { view: QuantitativeView; language: Language; now: number }) {
  const zh = language === "zh", f = forecastFor(view), c = f?.community;
  return <article className={`${styles.card} ${styles.community}`}>
    <div className={styles.heading}><h2>{zh ? "社区预期的实际贡献" : "What community expectations contribute"}</h2><span>{zh ? "重置或发卡" : "Reset or new credit"}</span></div>
    <div className={styles.communityNumbers}>{[24,48].map(h => <div key={h}><span>{h}h · {zh ? "社区综合概率" : "Community probability"}</span><strong>{percent(c?.probabilities[h-1])}</strong><small>{zh ? "对综合判断 " : "Effect on combined outlook "}<b>{points(communityEffect(view.evaluation,f,h))} {zh ? "点" : "pts"}</b></small></div>)}</div>
    <div className={styles.weight}><span>{zh ? "本次社区组权重" : "Community group weight"}</span><strong>{percent(c?.weight)}</strong><i><i style={{ width: `${(c?.weight ?? 0)*100}%` }} /></i></div>
    <p>{zh ? "综合有效的日期投票、概率判断和其他社区量化信号，按各自期限转换。相同信息群体合并后分配权重。" : "Combines eligible timing ballots, probability opinions and other quantified community signals using their own deadlines. Shared populations are grouped before weighting."}</p>
    <p>{zh ? "贡献为“完整模型 − 移除社区后的模型”，两者使用相同的时间、权重及事件包含约束。" : "The effect is the full model minus its community-removed counterpart, at the same issue time with matching weights and event constraints."}</p>
    <div className={styles.communityStatus}>{!c ? (zh ? "本次没有可用的社区概率，等待新数据。" : "No eligible community probability in this forecast; awaiting data.") : forecastStale(view,f,now) ? (zh ? "上次社区结果 · 等待模块更新" : "Saved community result · awaiting an update") : (zh ? "已参与本次综合判断" : "Included in this combined forecast")}</div>
  </article>;
}
