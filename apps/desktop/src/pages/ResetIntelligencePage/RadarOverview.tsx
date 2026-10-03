import { useState } from "react";
import { ArrowUpRight, ChevronDown, Clock3, Layers3, MessageCircle, Radio, Users } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { publicEvidenceTime } from "./presentation";
import { forecastPresentation, resetRelated } from "./resetBriefingModel";
import { currentOpinions, evidenceGroups, historyDelta } from "./radarOverviewModel";
import { PostConversation, PostUpdate } from "./ResetPosts";
import type { PublicReadReceipt, PublicResetTimeline, RadarForecast, RadarPoint, RadarView } from "./types";
import styles from "./resetBriefing.module.less";

type Props = { timeline: PublicResetTimeline; radar: RadarView; language: Language; now: number; failed?: boolean;
  onOpenSource?: (url: string) => void; onRead?: (receipts: PublicReadReceipt[]) => void };
const percent = (n: number | null | undefined) => n == null ? "—" : `${Number(n.toFixed(1))}%`;
const signed = (n: number) => `${n > 0 ? "+" : ""}${Number(n.toFixed(1))}`;

function Trend({ radar, hours, now, zh, language }: { radar: RadarView; hours: 24 | 48; now: number; zh: boolean; language: Language }) {
  const [source, setSource] = useState("horizon");
  const monitor = radar.forecasts.find(f => f.id === "reset_monitor");
  const points = (source === "horizon" ? radar.history : monitor?.history ?? [])
    .filter(p => Date.parse(p.at) <= now && Date.parse(p.at) >= now - 86400_000).sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  const value = (p: RadarPoint) => hours === 24 ? p.probability24h : p.probability48h;
  const x = (p: RadarPoint) => 44 + (Date.parse(p.at) - now + 86400_000) / 86400_000 * 438;
  const y = (p: RadarPoint) => 145 - value(p) / 100 * 125;
  // Disconnected paths preserve outages; unobserved hours stay blank.
  const paths = points.reduce<RadarPoint[][]>((all, p, i) => {
    if (!i || Date.parse(p.at) - Date.parse(points[i - 1].at) > (source === "horizon" ? 45 : 8 * 60) * 60_000) all.push([]);
    all[all.length - 1].push(p); return all;
  }, []);
  const latest = points.at(-1);
  return <div className={styles.trend}>
    <div className={styles.cardHeading}><h3>{zh ? "预测变化" : "Forecast trend"}</h3><span>{zh ? "过去 24 小时" : "Past 24 hours"}</span></div>
    <div className={styles.trendTabs} role="group" aria-label={zh ? "走势来源" : "Trend source"}>
      <button aria-pressed={source === "horizon"} onClick={() => setSource("horizon")}>Horizon</button>
      {!!monitor?.history.length && <button aria-pressed={source === "monitor"} onClick={() => setSource("monitor")}>Codex Reset Monitor</button>}
    </div>
    <svg viewBox="0 0 510 182" role="img" aria-label={`${source === "horizon" ? "Horizon" : "Codex Reset Monitor"} · ${hours}h · ${points.length} ${zh ? "个真实记录" : "recorded forecasts"}`}>
      {[0, 50, 100].map(n => <g key={n}><line x1="44" x2="482" y1={145 - n * 1.25} y2={145 - n * 1.25} className={styles.gridLine} /><text x="36" y={149 - n * 1.25} textAnchor="end">{n}%</text></g>)}
      {paths.map((path, i) => <path key={i} d={path.map((p, index) => `${index ? "L" : "M"}${x(p).toFixed(1)},${y(p).toFixed(1)}`).join(" ")} className={styles.trendLine} />)}
      {points.map(p => <circle key={p.at} cx={x(p)} cy={y(p)} r={p === latest ? 4 : 2} className={styles.trendDot}><title>{publicEvidenceTime(p.at, language)} · {percent(value(p))}</title></circle>)}
      <text x="44" y="176">−24h</text><text x="263" y="176" textAnchor="middle">−12h</text><text x="482" y="176" textAnchor="end">{zh ? "现在" : "Now"}</text>
    </svg>
    <p className={styles.trendCaption}>{points.length < 2 ? (zh ? "本机刚开始记录，后续每次采集会延长曲线。" : "Local recording has started. Each collection extends the series.")
      : source === "horizon" ? (zh ? `${points.length} 次本机预测 · 公告判定单独展示` : `${points.length} local forecasts · announcement rule shown separately`)
        : (zh ? "第三方保存的历史预测 · 可对照查看" : "Historical forecasts recorded by the source")}</p>
  </div>;
}

function SourceComparison({ radar, hours, zh, language, onOpenSource }: { radar: RadarView; hours: 24 | 48; zh: boolean; language: Language; onOpenSource?: (url: string) => void }) {
  const groups = evidenceGroups(radar.forecasts);
  const reason = (f: RadarForecast) => f.exclusion ? ({
    unavailable: zh ? "暂未取得预测" : "Forecast unavailable", fetch_failed: zh ? "采集失败 · 保留上次值" : "Fetch failed · saved value",
    reset_mismatch: zh ? "遗漏近期重置 · 未计入" : "Recent reset missing · excluded", stale: zh ? "数据待更新 · 未计入" : "Update due · excluded",
    community_overlap: zh ? "社区依据可能重叠 · 未再计入" : "Possible community overlap · excluded",
  }[f.exclusion]) : f.method === "cadence" ? (zh ? "历史节奏" : "Historical cadence") : f.method === "statements" ? (zh ? "历史与公开发言" : "History and statements") : (zh ? "综合判断 · 方法未完全披露" : "Mixed outlook · partial methodology");
  return <article className={styles.card}>
    <div className={styles.cardHeading}><h2>{zh ? "各方怎样判断" : "How sources compare"}</h2><span>{zh ? "同一事件" : "Same event"} · {hours}h</span></div>
    <div className={styles.scale}><span>0%</span><span>100%</span></div>
    <div className={styles.comparison}>{radar.forecasts.map(f => <div key={f.id} className={styles.sourceEstimate}>
      <div className={styles.barRow} data-muted={!!f.exclusion || undefined}>
        <EvidenceLink url={f.url} label={f.name} onOpen={onOpenSource} />
        <div className={styles.track} aria-hidden="true"><i style={{ width: `${(hours === 24 ? f.probability24h : f.probability48h) ?? 0}%` }} /></div>
        <strong>{percent(hours === 24 ? f.probability24h : f.probability48h)}</strong>
      </div><p data-excluded={!!f.exclusion || undefined}>{reason(f)}{!f.exclusion && ` · ${zh ? "权重" : "Weight"} ${Math.round(f.weight * 100)}%`}</p>
    </div>)}</div>
    <details className={styles.overlap}>
      <summary><Layers3 size={14} aria-hidden="true" /><span>{radar.forecasts.filter(f => f.probability24h != null).length} {zh ? "份预测" : "forecasts"} · {radar.estimate?.sourceCount ?? 0} {zh ? "份参与综合" : "pooled"}</span><span>{zh ? "查看依据与重叠" : "Evidence & overlap"}</span></summary>
      <p>{zh ? "共享历史节奏归为一组；同一原帖只列一次。同类方法共享权重，重复网站不会增加这类依据的总权重。" : "Historical cadence is one group and each original post appears once. Sources using the same method share its weight."}</p>
      {groups.map(g => <div key={g.key} className={styles.evidenceGroup}><strong>{g.key === "history" ? (zh ? "共同历史节奏" : "Shared reset history") : g.key.startsWith("undisclosed:") ? (zh ? "未披露的额外依据" : "Undisclosed additional inputs") : <EvidenceLink url={g.key} label={zh ? `原帖 ${g.key.split("/").at(-1)}` : `Post ${g.key.split("/").at(-1)}`} onOpen={onOpenSource} />}</strong><span>{g.sources.join(" · ")}</span></div>)}
      {radar.forecasts.map(f => <div className={styles.sourceTimes} key={f.id}><strong>{f.name}</strong><span>{zh ? "来源生成" : "Generated"} · {f.updatedAt ? publicEvidenceTime(f.updatedAt, language) : zh ? "网站未提供" : "Not published"}</span><span>{zh ? "本机读取" : "Collected"} · {publicEvidenceTime(f.collectedAt, language)}</span>{f.lastResetAt && <span>{zh ? "记录的最近重置" : "Last known reset"} · {publicEvidenceTime(f.lastResetAt, language)}</span>}</div>)}
    </details>
  </article>;
}

function CommunityPanel({ radar, zh, language, now, onOpenSource }: { radar: RadarView; zh: boolean; language: Language; now: number; onOpenSource?: (url: string) => void }) {
  const c = radar.community; const denominator = c.optimistic + c.uncertain + c.pessimistic;
  const [open, setOpen] = useState(false);
  const opinions = currentOpinions(c.opinions, now);
  const sampleStale = now - Date.parse(radar.updatedAt ?? "") >= 30 * 60_000 || c.channels.some(channel => !!channel.issue);
  const names = zh ? ["看好", "观望", "不看好", "愿望", "情况反馈"] : ["Optimistic", "Uncertain", "Pessimistic", "Wish", "Reported experience"];
  const stances = ["optimistic", "uncertain", "pessimistic", "wish", "observation"];
  return <article className={styles.card}>
    <div className={styles.cardHeading}><h2>{zh ? "社区预期" : "Community outlook"}</h2><span>{zh ? "过去 6 小时" : "Past 6 hours"}</span></div>
    {sampleStale && <p className={styles.correction}>{zh ? "采样待更新，显示上次统计。" : "Sampling update due; showing the previous summary."}</p>}
    <div className={styles.communityValue}><strong>{percent(c.optimisticShare)}</strong><span>{denominator ? (zh ? "预测样本内倾向会重置" : "of sampled predictions expect a reset") : (zh ? "暂未采到明确的近期预测" : "No explicit near-term predictions sampled")}</span></div>
    {denominator > 0 ? <>
      <div className={styles.sentimentBar} aria-label={zh ? "社区预期分布" : "Sampled sentiment distribution"}>{[c.optimistic, c.uncertain, c.pessimistic].map((n, i) => <i key={i} data-stance={stances[i]} style={{ width: `${n / denominator * 100}%` }} />)}</div>
      <div className={styles.legend}>{[c.optimistic, c.uncertain, c.pessimistic].map((n, i) => <span key={i}><i data-stance={stances[i]} />{names[i]} {Math.round(n / denominator * 100)}%</span>)}</div>
    </> : <div className={styles.discussionCounts}><div><strong>{c.wishes}</strong><span>{zh ? "期待与愿望" : "Wishes"}</span></div><div><strong>{c.observations}</strong><span>{zh ? "额度与重置反馈" : "Quota / reset reports"}</span></div></div>}
    <p className={styles.communityMeta}>{c.authors} {zh ? "位发言者" : "authors"} · {c.communities} {zh ? "个社区有相关内容" : "communities with matching posts"}
      {c.previousShare != null && c.optimisticShare != null && <><br />{zh ? "较上一时段 " : "vs. previous window "}{signed(c.optimisticShare - c.previousShare)} {zh ? "点" : "pts"}</>}</p>
    <p className={styles.meta}>{zh ? `${c.independentAuthors} 位发言者给出有期限的独立判断；愿望和到账反馈单独统计。` : `${c.independentAuthors} authors give independent, timed predictions. Wishes and delivery reports are counted separately.`}</p>
    <button className={styles.moreButton} onClick={() => setOpen(!open)} aria-expanded={open}>{zh ? "查看观点、依据与采样范围" : "Views, evidence and sampling"}<ArrowUpRight size={15} aria-hidden="true" /></button>
    {open && <div className={styles.communityDetails}>
      <p>{zh ? `每位作者在一个时段内取最新发言，合并 ${c.duplicates} 条重复内容。百分比的分母为 ${denominator} 位表达预测或不确定判断的作者。` : `Latest statement per author per window; ${c.duplicates} duplicates merged. Percentages use ${denominator} authors expressing predictions or uncertainty.`}</p>
      {c.channels.map(channel => <div className={styles.sourceTimes} key={channel.id}><EvidenceLink url={channel.url} label={channel.name} onOpen={onOpenSource} /><span>{channel.issue ? (zh ? "本轮采集失败，保留上次资料" : "Collection failed; saved data retained") : (zh ? `读取 ${channel.scanned} 条记录` : `${channel.scanned} records scanned`)}{channel.truncated && (zh ? " · 达到采样上限" : " · sampling limit reached")}</span><span>{channel.query}</span><span>{publicEvidenceTime(channel.successAt, language)}</span></div>)}
      <p>{zh ? "当前采样来自 GitHub 与 Hacker News 的公开接口，按最新记录固定采样。微信、Reddit 与 Linux.do 尚未接入。" : "Fixed latest-record sampling from public GitHub and Hacker News APIs. WeChat, Reddit and Linux.do are not connected."}</p>
      {opinions.map(p => <article key={p.id} className={styles.opinion}><div><EvidenceLink url={p.url} label={`@${p.author}`} onOpen={onOpenSource} /><span>{names[stances.indexOf(p.stance)]}{p.horizonHours != null && ` · ${p.horizonHours}h`}</span></div><p>{p.text}</p><time>{publicEvidenceTime(p.publishedAt, language)}</time></article>)}
      {!opinions.length && <p>{zh ? "本时段没有符合条件的发言。" : "No matching statements in this window."}</p>}
    </div>}
  </article>;
}

export function RadarOverview({ timeline, radar, language, now, failed, onOpenSource, onRead }: Props) {
  const zh = language === "zh"; const [hours, setHours] = useState<24 | 48>(24); const [more, setMore] = useState(false);
  const [latestOpen, setLatestOpen] = useState(false);
  const outlook = forecastPresentation(timeline.insights, now, failed, timeline);
  const e = radar.estimate;
  const p = hours === 24 ? outlook.probability24h : outlook.probability48h;
  const adjustment = e ? hours === 24 ? e.communityAdjustment24h : e.communityAdjustment48h : 0;
  const stale = !!failed || now - Date.parse(radar.updatedAt ?? "") >= 30 * 60_000;
  const delta = historyDelta(radar.history, now, hours);
  const committed = !!outlook.announcement?.timing && outlook.announcement.timing.end <= now + hours * 3_600_000;
  const spread = e ? hours === 24 ? e.spread24h : e.spread48h : null;
  const posts = [...(timeline.insights?.posts?.posts ?? [])].filter(resetRelated).sort((a, b) => Date.parse(b.publishedAt) - Date.parse(a.publishedAt));
  const latest = posts[0];
  const postSource = timeline.sources.find(source => source.sourceId === "codex_reset_posts");
  const postsStale = !!failed || !!postSource?.issue || now - Date.parse(timeline.insights?.posts?.fetchedAt ?? "") >= 30 * 60_000;
  const posture = p == null ? (zh ? "等待数据" : "Awaiting data") : committed ? (zh ? "已明确预告" : "Explicitly announced") : p >= 65 ? (zh ? "偏乐观" : "Optimistic") : p >= 40 ? (zh ? "保持关注" : "Watch closely") : (zh ? "近期预期偏低" : "Lower near-term outlook");
  const notice = outlook.notice;
  const noticeStatus = outlook.noticeWithdrawn ? (zh ? "重置预告已更正" : "Reset announcement corrected")
    : notice?.state === "reported" ? (zh ? "上一轮重置已报告完成" : "Previous reset reported complete")
    : notice?.state === "elapsed" ? (zh ? "预告时间已到 · 等待确认" : "Scheduled time reached · awaiting confirmation")
    : notice?.conflictingTiming ? (zh ? "预告时间有分歧" : "Timing interpretations differ")
    : notice?.stale || failed ? (zh ? "上次预告 · 待更新" : "Saved announcement · update due")
    : (zh ? "已明确预告 · 追踪进展" : "Explicitly announced · tracking progress");
  const latestReset = radar.forecasts.find(f => f.id === "codex_reset")?.lastResetAt;
  return <section className={styles.briefing} aria-label={zh ? "重置雷达综合概览" : "Reset radar overview"}>
    <div className={styles.overviewHeading}><span>{zh ? "公共额度重置 · 自动采集" : "Public quota resets · automatic collection"}</span><div className={styles.periods} aria-label={zh ? "预测时间窗口" : "Forecast window"}>{([24, 48] as const).map(h => <button key={h} aria-pressed={hours === h} onClick={() => setHours(h)}>{h} {zh ? "小时" : "hours"}</button>)}</div></div>
    <article className={styles.forecast} data-stale={stale || undefined}>
      <div className={styles.heroGrid}><div className={styles.outlook}>
        <div className={styles.byline}><Radio size={16} aria-hidden="true" /><h2>{zh ? "Horizon 综合估计" : "Horizon combined estimate"}</h2><span className={styles.badge}>{committed ? (zh ? "公告判定" : "Announcement rule") : (zh ? "试验" : "Experimental")}</span></div>
        <div className={styles.heroProbability}><strong>{p == null ? "—" : Math.round(p)}{p != null && <small>%</small>}</strong><div><b>{posture}</b><span>{stale ? (zh ? "上次结果 · 待更新" : "Saved result · update due") : committed ? (zh ? `未来 ${hours} 小时已有明确安排` : `Explicitly scheduled within ${hours} hours`) : delta != null ? (zh ? `较 6 小时前 ${signed(delta)} 点` : `${signed(delta)} pts vs. 6 hours ago`) : (zh ? `未来 ${hours} 小时 · 本机开始持续记录` : `Next ${hours} hours · local history recording`)}</span></div></div>
        <p className={styles.heroReason}>{committed ? (zh ? "明确预告优先，追踪实际重置进展" : "The announcement leads; delivery is tracked below") : !e ? (zh ? "预测来源正在更新" : "Forecast sources are updating") : latestReset && now - Date.parse(latestReset) < 24 * 3_600_000 ? (zh ? "刚发生一次重置，关注下一次新信号" : "A reset recently completed; watching for the next signal") : adjustment > 0 ? (zh ? "独立社区判断抬升了近期预期" : "Independent community predictions lift the outlook") : adjustment < 0 ? (zh ? "社区判断使近期预期有所回落" : "Community predictions reduce the near-term outlook") : (zh ? "综合历史节奏与公开发言，跟踪来源分歧" : "Combining cadence and public statements, tracking disagreement")}</p>
        <div className={styles.heroMeta}><span>{zh ? "可用预测" : "Usable forecasts"} <b>{e?.sourceCount ?? 0}</b></span><span>{zh ? "来源分歧" : "Source spread"} <b>{spread == null ? "—" : `${Math.round(spread)} ${zh ? "点" : "pts"}`}</b></span><span>{zh ? "社区影响" : "Community effect"} <b>{e ? `${signed(adjustment)} ${zh ? "点" : "pts"}` : "—"}</b></span></div>
      </div><Trend radar={radar} hours={hours} now={now} zh={zh} language={language} /></div>
    </article>
    <div className={styles.detailGrid}><SourceComparison radar={radar} hours={hours} zh={zh} language={language} onOpenSource={onOpenSource} /><CommunityPanel radar={radar} zh={zh} language={language} now={now} onOpenSource={onOpenSource} /></div>
    <article className={styles.card}>
      <div className={styles.cardHeading}><h2>{zh ? "影响当前判断的变化" : "What is shaping the outlook"}</h2><span>{zh ? "按新增信息排序" : "Newest information first"}</span></div>
      {latest && postsStale && <p className={styles.correction}>{zh ? "发言来源待更新，保留上次正文与发布时间。" : "Post source update due; saved text and publication times are retained."}</p>}
      {latest && <div className={styles.leadUpdate}><button className={styles.updateButton} aria-expanded={latestOpen} onClick={() => setLatestOpen(!latestOpen)}><MessageCircle size={19} aria-hidden="true" /><span className={styles.updateText}><strong>{zh ? "Tibo · 最新相关发言" : "Tibo · latest relevant statement"}</strong><span>{zh ? latest.translatedText || latest.text : latest.text}</span></span><time>{publicEvidenceTime(latest.publishedAt, language)}</time><ChevronDown size={14} aria-hidden="true" /></button>{latestOpen && <PostConversation post={latest} timeline={timeline} language={language} onRead={onRead} onOpenSource={onOpenSource} />}</div>}
      {notice && <div className={styles.changeRow}><Clock3 size={19} aria-hidden="true" /><div><strong>{noticeStatus}</strong><div className={styles.alignedTimes}>{notice.reportedAt && <div><span>{zh ? "报告完成" : "Reported complete"}</span><time>{publicEvidenceTime(new Date(notice.reportedAt).toISOString(), language)}</time></div>}{notice.timing && <div><span>{zh ? "预告时间" : "Scheduled"}</span><time>{publicEvidenceTime(new Date(notice.timing.end).toISOString(), language)}</time></div>}</div>{notice.cohort && <p>{zh && notice.cohort === "all paid ChatGPT accounts" ? "所有付费 ChatGPT 账号" : notice.cohort}</p>}</div><EvidenceLink url={notice.url} label={zh ? "查看原帖" : "Source"} onOpen={onOpenSource} /></div>}
      <div className={styles.changeRow}><Users size={19} aria-hidden="true" /><div><strong>{zh ? "社区采样与综合判断" : "Community sample and combined estimate"}</strong><p>{zh ? `${radar.community.authors} 位近期发言者 · ${radar.community.independentAuthors} 位独立、有期限的预测作者 · 社区影响 ${signed(adjustment)} 点` : `${radar.community.authors} recent authors · ${radar.community.independentAuthors} independent, timed predictions · community effect ${signed(adjustment)} pts`}</p></div><time>{publicEvidenceTime(radar.updatedAt, language)}</time></div>
      {posts.length > 1 && <><button className={styles.moreButton} aria-expanded={more} onClick={() => setMore(!more)}>{zh ? "更多相关发言" : "More related statements"}<ChevronDown size={14} aria-hidden="true" /></button>{more && posts.slice(1, 8).map(post => <PostUpdate key={post.id} post={post} timeline={timeline} language={language} onRead={onRead} onOpenSource={onOpenSource} />)}</>}
    </article>
    <footer className={styles.health}>
      <div><span>{zh ? "每 15 分钟自动采集" : "Collects every 15 minutes"}</span><span>{zh ? "预测记录 " : "Estimate recorded "}{publicEvidenceTime(radar.updatedAt, language)}</span><span>{timeline.sources.filter(s => !s.issue && s.lastSuccessAt && now - Date.parse(s.lastSuccessAt) < 30 * 60_000).length}/{timeline.sources.length} {zh ? "事件渠道可读" : "event feeds responding"} · {radar.forecasts.filter(f => !f.issue).length}/{radar.forecasts.length} {zh ? "预测渠道可读" : "forecast feeds responding"} · {radar.community.channels.filter(c => !c.issue).length}/{radar.community.channels.length} {zh ? "社区渠道可读" : "community feeds responding"}</span></div>
      <details className={styles.signal}><summary>{zh ? "综合估计如何计算" : "How the estimate is calculated"}</summary><p>{zh ? "按方法分组的线性加权：历史节奏 50%、公开发言 35%、混合方法 15%，可用组之间重新归一化。组内网站共享权重；资料过期、采集失败或遗漏近期重置的预测退出计算。" : "Linear pool grouped by method: cadence 50%, statements 35%, mixed methods 15%, normalized across available groups. Sites divide their group’s weight. Stale, failed or reset-inconsistent inputs are excluded."}</p><p>{zh ? "社区按作者去重，至少 3 位独立且明确期限的预测作者才调整估计；引用已有发言不重复加入。调整按样本量收缩，并受渠道覆盖限制。此时可能已使用社区因素的混合来源退出组合。参数是首版试验设定，后续用保存的预测检验。" : "Community adjustment needs at least three independent authors with explicit horizons. Authors are deduplicated and existing statement evidence is excluded. The adjustment shrinks with sample size and limited coverage; potentially overlapping mixed sources are then excluded. These are v1 experimental parameters to evaluate against saved forecasts."}</p><p>{e?.modelVersion ?? "horizon-pool-v1"} · {zh ? "本机保存输入与输出，最多 30 天 / 24 MiB。Horizon 曲线从实际记录时刻开始。" : "Inputs and outputs are stored locally, up to 30 days / 24 MiB. The Horizon curve starts at actual recording time."}</p></details>
    </footer>
  </section>;
}
