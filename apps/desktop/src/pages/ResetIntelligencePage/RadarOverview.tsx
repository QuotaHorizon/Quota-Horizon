import { useState, type CSSProperties } from "react";
import { ArrowUpRight, Clock3, Layers3, Radio } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { publicEvidenceTime } from "./presentation";
import { forecastPresentation } from "./resetBriefingModel";
import { acceptedForecasts, currentOpinions, evidenceGroups, forecastColor, historyDelta } from "./radarOverviewModel";
import { ForecastTrend } from "./ForecastTrend";
import { TiboBriefing } from "./TiboBriefing";
import type { PublicReadReceipt, PublicResetTimeline, RadarForecast, RadarView } from "./types";
import styles from "./resetBriefing.module.less";

type Props = { timeline: PublicResetTimeline; radar: RadarView; language: Language; now: number; failed?: boolean;
  onOpenSource?: (url: string) => void; onRead?: (receipts: PublicReadReceipt[]) => void };
const percent = (n: number | null | undefined) => n == null ? "—" : `${Number(n.toFixed(1))}%`;
const signed = (n: number) => `${n > 0 ? "+" : ""}${Number(n.toFixed(1))}`;

function SourceComparison({ radar, zh, language, onOpenSource }: { radar: RadarView; zh: boolean; language: Language; onOpenSource?: (url: string) => void }) {
  const sources = acceptedForecasts(radar.forecasts);
  const groups = evidenceGroups(sources);
  const reasons: Record<string, string> = { unavailable: zh ? "预测待取得" : "Forecast unavailable", fetch_failed: zh ? "采集失败 · 上次值" : "Fetch failed · saved value", reset_mismatch: zh ? "遗漏近期重置" : "Recent reset missing", stale: zh ? "来源待更新" : "Update due" };
  const reason = (f: RadarForecast) => reasons[f.exclusion ?? ""] ?? (f.method === "cadence" ? (zh ? "历史节奏" : "Historical cadence") : (zh ? "历史与公开发言" : "History and statements"));
  return <article className={styles.card}>
    <div className={styles.cardHeading}><h2>{zh ? "各方怎样判断" : "How sources compare"}</h2><span>{zh ? "同一事件 · 两个窗口并列" : "Same event · both horizons"}</span></div>
    <div className={styles.sourceMatrix}><div className={styles.sourceMatrixHeading}><span>{zh ? "通过来源审查" : "Reviewed sources"}</span><span>24h</span><span>48h</span></div>
      {sources.map(f => <div key={f.id} className={styles.sourceMatrixRow} data-muted={!!f.exclusion || undefined} style={{ "--series-color": forecastColor(f.id) } as CSSProperties}>
        <div><EvidenceLink url={f.url} label={f.name} onOpen={onOpenSource} /><p>{reason(f)}{f.weight > 0 && ` · ${zh ? "权重" : "weight"} ${Math.round(f.weight * 100)}%`}</p></div>
        {[f.probability24h, f.probability48h].map((p, i) => <div className={styles.sourceCell} key={i}><strong>{percent(p)}</strong><div className={styles.track}><i style={{ width: `${p ?? 0}%` }} /></div></div>)}
      </div>)}
    </div>
    <details className={styles.overlap}><summary><Layers3 size={14} aria-hidden="true" /><span>{radar.estimate?.sourceCount ?? 0} {zh ? "份有效预测参与综合" : "usable forecasts pooled"}</span><span>{zh ? "依据与来源审查" : "Evidence & source review"}</span></summary>
      <p>{zh ? "同类方法共享权重；共同历史和原帖分别归组。只有预测对象、生成时间和近期事件记录通过检查的来源参与计算。" : "Methods share weights; common history and original posts are grouped. Inputs must pass event-scope, generation-time and recent-reset checks."}</p>
      {groups.map(g => <div key={g.key} className={styles.evidenceGroup}><strong>{g.key === "history" ? (zh ? "共同历史节奏" : "Shared reset history") : <EvidenceLink url={g.key} label={zh ? `原帖 ${g.key.split("/").at(-1)}` : `Post ${g.key.split("/").at(-1)}`} onOpen={onOpenSource} />}</strong><span>{g.sources.join(" · ")}</span></div>)}
      {sources.map(f => <div className={styles.sourceTimes} key={f.id}><strong>{f.name}</strong><span>{zh ? "来源生成" : "Generated"} · {publicEvidenceTime(f.updatedAt, language)}</span><span>{zh ? "本机读取" : "Collected"} · {publicEvidenceTime(f.collectedAt, language)}</span><span>{zh ? "记录的最近重置" : "Last known reset"} · {publicEvidenceTime(f.lastResetAt, language)}</span></div>)}
      <div className={styles.sourceTimes}><strong>{zh ? "未纳入的来源" : "Sources not admitted"}</strong><span>CodexReset.app · {zh ? "停止常规采集：遗漏近期重置，混入无关服务事故。" : "Collection retired: missed recent resets and included unrelated service incidents."}</span><span>QuotaCue · {zh ? "仅保留待审查资料：未提供生成时间，方法依据不足。" : "Review only: no generation timestamp and insufficient methodology detail."}</span></div>
    </details>
  </article>;
}

function CommunityPanel({ radar, zh, language, now, onOpenSource }: { radar: RadarView; zh: boolean; language: Language; now: number; onOpenSource?: (url: string) => void }) {
  const c = radar.community; const denominator = c.optimistic + c.uncertain + c.pessimistic;
  const [open, setOpen] = useState(false);
  const windowHours = c.windowHours || 6;
  const opinions = currentOpinions(c.opinions, now, windowHours);
  const sampleStale = now - Date.parse(radar.updatedAt ?? "") >= 30 * 60_000 || c.channels.some(channel => !!channel.issue);
  const names = zh ? ["看好", "观望", "不看好", "愿望", "情况反馈"] : ["Optimistic", "Uncertain", "Pessimistic", "Wish", "Reported experience"];
  const stances = ["optimistic", "uncertain", "pessimistic", "wish", "observation"];
  return <article className={styles.card}>
    <div className={styles.cardHeading}><h2>{zh ? "社区预期" : "Community outlook"}</h2><span>{zh ? `过去 ${windowHours} 小时` : `Past ${windowHours} hours`}</span></div>
    {sampleStale && <p className={styles.correction}>{zh ? "采样待更新，显示上次统计。" : "Sampling update due; showing the previous summary."}</p>}
    <div className={styles.communityValue}><strong>{c.optimistic + c.pessimistic ? percent(c.optimisticShare) : "—"}</strong><span>{c.optimistic + c.pessimistic ? (zh ? "预测样本内倾向会重置" : "of sampled predictions expect a reset") : denominator ? (zh ? "只有观望判断，暂无明确方向" : "Only uncertainty sampled; no directional outlook") : (zh ? "暂未采到明确的近期预测" : "No explicit near-term predictions sampled")}</span></div>
    {denominator > 0 ? <>
      <div className={styles.sentimentBar} aria-label={zh ? "社区预期分布" : "Sampled sentiment distribution"}>{[c.optimistic, c.uncertain, c.pessimistic].map((n, i) => <i key={i} data-stance={stances[i]} style={{ width: `${n / denominator * 100}%` }} />)}</div>
      <div className={styles.legend}>{[c.optimistic, c.uncertain, c.pessimistic].map((n, i) => <span key={i}><i data-stance={stances[i]} />{names[i]} {Math.round(n / denominator * 100)}%</span>)}</div>
    </> : <div className={styles.discussionCounts}><div><strong>{c.wishes}</strong><span>{zh ? "期待与愿望" : "Wishes"}</span></div><div><strong>{c.observations}</strong><span>{zh ? "额度与重置反馈" : "Quota / reset reports"}</span></div></div>}
    <p className={styles.communityMeta}>{c.authors} {zh ? "位发言者" : "authors"} · {c.communities} {zh ? "个社区有相关内容" : "communities with matching posts"}
      {denominator > 0 && <><br />{denominator} {zh ? "位表达预测或观望" : "with predictions or uncertainty"} · {c.wishes} {zh ? "份愿望" : "wishes"} · {c.observations} {zh ? "份情况反馈" : "reports"}</>}
      {c.previousShare != null && c.optimisticShare != null && <><br />{zh ? "较上一时段 " : "vs. previous window "}{signed(c.optimisticShare - c.previousShare)} {zh ? "点" : "pts"}</>}</p>
    <div className={styles.communityContribution}>{([24, 48] as const).map(hours => {
      const effect = c.effects?.find(e => e.hours === hours);
      const change = radar.estimate ? hours === 24 ? radar.estimate.communityAdjustment24h : radar.estimate.communityAdjustment48h : null;
      return <div key={hours}><span>{hours}h · {zh ? "对综合估计的贡献" : "contribution to estimate"}</span><strong>{change == null ? "—" : `${signed(change)} ${zh ? "点" : "pts"}`}</strong><span>{effect ? (zh ? `${effect.eligibleAuthors} 位有效作者 · 折合 ${effect.effectiveAuthors} 份去重、时效加权判断` : `${effect.eligibleAuthors} eligible authors · ${effect.effectiveAuthors} effective, age-weighted opinions`) : (zh ? "等待新采样记录" : "Awaiting new sampling record")}</span></div>;
    })}</div>
    <p className={styles.meta}>{zh ? "愿望、到账反馈和不明确期限的观点保留展示；有效期内的预测才影响对应窗口。" : "Wishes, delivery reports and untimed views remain visible. Only unexpired predictions affect the matching horizon."}</p>
    <button className={styles.moreButton} onClick={() => setOpen(!open)} aria-expanded={open}>{zh ? "查看观点、依据与采样范围" : "Views, evidence and sampling"}<ArrowUpRight size={15} aria-hidden="true" /></button>
    {open && <div className={styles.communityDetails}>
      <p>{zh ? `每位作者在一个时段内取最新发言，合并 ${c.duplicates} 条重复内容。百分比的分母为 ${denominator} 位表达预测或不确定判断的作者。` : `Latest statement per author per window; ${c.duplicates} duplicates merged. Percentages use ${denominator} authors expressing predictions or uncertainty.`}</p>
      {c.channels.map(channel => <div className={styles.sourceTimes} key={channel.id}><EvidenceLink url={channel.url} label={channel.name} onOpen={onOpenSource} /><span>{channel.issue ? (zh ? "本轮采集失败，保留上次资料" : "Collection failed; saved data retained") : (zh ? `读取 ${channel.scanned} 条记录` : `${channel.scanned} records scanned`)}{channel.truncated && (zh ? " · 达到采样上限" : " · sampling limit reached")}</span><span>{channel.query}</span><span>{publicEvidenceTime(channel.successAt, language)}</span></div>)}
      <p>{zh ? "当前采样来自 GitHub 与 Hacker News 的公开接口，按最新记录固定采样。微信、Reddit 与 Linux.do 尚未接入。" : "Fixed latest-record sampling from public GitHub and Hacker News APIs. WeChat, Reddit and Linux.do are not connected."}</p>
      <p>{zh ? "补采近期活跃重置话题的回复。引用同一事实但带有个人推断的观点保留，共用依据合计最多计一份；按 12 小时半衰期降低旧观点影响。" : "Recent active reset threads are sampled for replies. Personal inferences citing a shared fact are retained and collectively capped at one effective opinion; older views decay with a 12-hour half-life."}</p>
      {opinions.map(p => <article key={p.id} className={styles.opinion}><div><EvidenceLink url={p.url} label={`@${p.author}`} onOpen={onOpenSource} /><span>{names[stances.indexOf(p.stance)]}{p.horizonHours != null && ` · ${p.horizonHours}h`}</span></div><p>{p.text}</p><time>{publicEvidenceTime(p.publishedAt, language)}</time></article>)}
      {!opinions.length && <p>{zh ? "本时段没有符合条件的发言。" : "No matching statements in this window."}</p>}
    </div>}
  </article>;
}
export function RadarOverview({ timeline, radar, language, now, failed, onOpenSource, onRead }: Props) {
  const zh = language === "zh";
  const outlook = forecastPresentation(timeline.insights, now, failed, timeline);
  const e = radar.estimate;
  const stale = !!failed || now - Date.parse(radar.updatedAt ?? "") >= 30 * 60_000;
  const notice = outlook.notice;
  const noticeStatus = outlook.noticeWithdrawn ? (zh ? "重置预告已更正" : "Reset announcement corrected")
    : notice?.state === "reported" ? (zh ? "上一轮重置已报告完成" : "Previous reset reported complete")
    : notice?.state === "elapsed" ? (zh ? "预告时间已到 · 等待确认" : "Scheduled time reached · awaiting confirmation")
    : notice?.conflictingTiming ? (zh ? "预告时间有分歧" : "Timing interpretations differ")
    : notice?.stale || failed ? (zh ? "上次预告 · 待更新" : "Saved announcement · update due")
    : (zh ? "已明确预告 · 追踪进展" : "Explicitly announced · tracking progress");
  const latestReset = radar.forecasts.find(f => f.id === "codex_reset")?.lastResetAt;
  return <section className={styles.briefing} aria-label={zh ? "重置雷达综合概览" : "Reset radar overview"}>
    <div className={styles.overviewHeading}><span>{zh ? "公共额度重置 · 自动采集" : "Public quota resets · automatic collection"}</span><span>{zh ? "24h / 48h 同时对照" : "24h / 48h at a glance"}</span></div>
    <article className={styles.forecast} data-stale={stale || undefined}>
      <div className={styles.heroGrid}><div className={styles.outlook}>
        <div className={styles.byline}><Radio size={16} aria-hidden="true" /><h2>{zh ? "Horizon 综合估计" : "Horizon combined estimate"}</h2><span className={styles.badge}>{zh ? "试验" : "Experimental"}</span></div>
        <div className={styles.horizonPair}>{([24, 48] as const).map(hours => {
          const p = hours === 24 ? outlook.probability24h : outlook.probability48h;
          const committed = !!outlook.announcement?.timing && outlook.announcement.timing.end <= now + hours * 3_600_000;
          const delta = historyDelta(radar.history, now, hours);
          const adjustment = e ? hours === 24 ? e.communityAdjustment24h : e.communityAdjustment48h : null;
          return <div key={hours} className={styles.horizonValue}><span>{zh ? `未来 ${hours} 小时` : `Next ${hours} hours`}</span><strong>{p == null ? "—" : Math.round(p)}{p != null && <small>%</small>}</strong>
            <b>{committed ? (zh ? "明确预告覆盖" : "Explicitly announced") : p == null ? (zh ? "等待可用预测" : "Awaiting forecasts") : p >= 65 ? (zh ? "偏乐观" : "Optimistic") : p >= 40 ? (zh ? "保持关注" : "Watch closely") : (zh ? "近期预期偏低" : "Lower near-term outlook")}</b>
            <span>{stale ? (zh ? "上次结果 · 待更新" : "Saved result · update due") : delta == null ? (zh ? "正在积累真实历史" : "Recording forecast history") : (zh ? `较 6 小时前 ${signed(delta)} 点` : `${signed(delta)} pts vs. 6h ago`)}</span>
            <small>{zh ? "社区贡献 " : "Community contribution "}{adjustment == null ? "—" : `${signed(adjustment)} ${zh ? "点" : "pts"}`}</small>
          </div>;
        })}</div>
        <p className={styles.heroReason}>{outlook.announcement ? (zh ? "明确预告优先，追踪实际重置进展" : "The announcement leads; tracking actual progress") : latestReset && now - Date.parse(latestReset) < 24 * 3_600_000 ? (zh ? "刚发生一次重置，关注新的明确安排与社区判断" : "A reset recently completed; watching new commitments and community forecasts") : (zh ? "结合有效来源与去重的社区判断" : "Combining reviewed sources and deduplicated community forecasts")}</p>
        <div className={styles.heroMeta}><span>{zh ? "有效来源" : "Usable sources"} <b>{e?.sourceCount ?? 0}</b></span><span>{zh ? "来源分歧 24h / 48h" : "Spread 24h / 48h"} <b>{e ? `${Math.round(e.spread24h)} / ${Math.round(e.spread48h)} ${zh ? "点" : "pts"}` : "—"}</b></span></div>
      </div><ForecastTrend radar={radar} now={now} language={language} /></div>
    </article>
    <TiboBriefing timeline={timeline} language={language} notice={notice} now={now} failed={failed} onOpenSource={onOpenSource} onRead={onRead} />
    <div className={styles.detailGrid}><SourceComparison radar={radar} zh={zh} language={language} onOpenSource={onOpenSource} /><CommunityPanel radar={radar} zh={zh} language={language} now={now} onOpenSource={onOpenSource} /></div>
    {notice && <article className={styles.card}><div className={styles.changeRow}><Clock3 size={19} aria-hidden="true" /><div><strong>{noticeStatus}</strong><div className={styles.alignedTimes}>{notice.reportedAt && <div><span>{zh ? "报告完成" : "Reported complete"}</span><time>{publicEvidenceTime(new Date(notice.reportedAt).toISOString(), language)}</time></div>}{notice.timing && <div><span>{zh ? "预告时间" : "Scheduled"}</span><time>{publicEvidenceTime(new Date(notice.timing.end).toISOString(), language)}</time></div>}</div>{notice.cohort && <p>{zh && notice.cohort === "all paid ChatGPT accounts" ? "所有付费 ChatGPT 账号" : notice.cohort}</p>}</div><EvidenceLink url={notice.url} label={zh ? "查看原帖" : "Source"} onOpen={onOpenSource} /></div></article>}
    <footer className={styles.health}>
      <div><span>{zh ? "每 15 分钟自动采集" : "Collects every 15 minutes"}</span><span>{zh ? "预测记录 " : "Estimate recorded "}{publicEvidenceTime(radar.updatedAt, language)}</span><span>{acceptedForecasts(radar.forecasts).filter(f => !f.issue).length}/{acceptedForecasts(radar.forecasts).length} {zh ? "已审查预测渠道可读" : "reviewed forecast feeds responding"} · {radar.community.channels.filter(c => !c.issue).length}/{radar.community.channels.length} {zh ? "社区渠道可读" : "community feeds responding"}</span></div>
      <details className={styles.signal}><summary>{zh ? "综合估计如何计算" : "How the estimate is calculated"}</summary><p>{zh ? "先检查预测对象、生成时间和最近重置记录，再按方法分组加权；历史节奏与公开发言的相对权重为 50:35，组内相同判断共享权重。未通过来源审查的数值不进入组合。" : "Review target event, generation time and latest reset first, then pool by method: cadence and statements use relative weights of 50:35, with identical opinions sharing weight. Unadmitted sources do not enter the pool."}</p><p>{zh ? "社区按作者和文本去重，区分原始事实与个人预测；同一依据下的推断共享贡献额度。近期、有期限的判断按样本量与时效收缩，缺少样本保留为空。社区贡献与各窗口有效样本在面板中并列显示。当前设定：12 小时半衰期、12 份有效判断的收缩强度、最多 1.5 的对数赔率调整；单一社区按半权重计算，参数待验证。" : "Community inputs deduplicate authors and text, separating facts from personal predictions. Inferences sharing evidence share a contribution cap. Timed predictions shrink with sample size and age; missing samples remain absent. Each horizon shows its effective sample and contribution. Current priors: a 12-hour half-life, shrinkage equivalent to 12 effective opinions and at most 1.5 log-odds adjustment; one community receives half coverage weight. These parameters remain experimental."}</p><p>{e?.modelVersion ?? "horizon-pool-v2"} · {zh ? "本机保留输入与输出，最多 30 天 / 24 MiB。" : "Inputs and outputs retained locally, up to 30 days / 24 MiB."}</p></details>
    </footer>
  </section>;
}
