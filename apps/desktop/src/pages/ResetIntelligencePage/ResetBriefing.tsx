import { useState } from "react";
import { Check, ChevronDown, Clock3, Radio, TrendingUp } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { publicEvidenceTime, publicSourceLabel, SOURCE_NAMES } from "./presentation";
import { forecastPresentation, resetRelated, selectBriefingPost } from "./resetBriefingModel";
import type { PublicReadReceipt, PublicResetTimeline } from "./types";
import { PostConversation, PostUpdate } from "./ResetPosts";
import shared from "./index.module.less";
import styles from "./resetBriefing.module.less";
import { RadarOverview } from "./RadarOverview";

function ProbabilityBar({ label, value, muted = false }: { label: string; value: number | null; muted?: boolean }) {
  return <div className={styles.barRow} data-muted={muted || undefined}>
    <span>{label}</span><div className={styles.track} aria-hidden="true">{value != null && <i style={{ width: `${value}%` }} />}</div>
    <strong>{value == null ? "—" : `${value}%`}</strong>
  </div>;
}

type BriefingProps = {
  timeline: PublicResetTimeline | null; language: Language; now: number; failed?: boolean; onOpenSource?: (url: string) => void;
  onRead?: (receipts: PublicReadReceipt[]) => void;
};
export function ResetBriefing(props: BriefingProps) {
  return props.timeline?.radar?.updatedAt
    ? <RadarOverview {...props} timeline={props.timeline} radar={props.timeline.radar} />
    : <LegacyBriefing {...props} />;
}

function LegacyBriefing({ timeline, language, now, failed = false, onOpenSource, onRead }: BriefingProps) {
  const zh = language === "zh";
  const { value, stale, signalActive, signalWithdrawn, notice, noticeWithdrawn, announcement, probability24h, probability48h } = forecastPresentation(timeline?.insights, now, failed, timeline);
  const feed = timeline?.insights?.posts;
  const posts = feed?.posts ?? [];
  const [hours, setHours] = useState<24 | 48>(24);
  const [all, setAll] = useState(false);
  const [limit, setLimit] = useState(3);
  const latest = selectBriefingPost(posts, notice);
  const otherPosts = (all ? posts : posts.filter(resetRelated)).filter((post) => post.id !== latest?.id)
    .sort((a, b) => Date.parse(b.publishedAt) - Date.parse(a.publishedAt));
  const feedSource = timeline?.sources.find((source) => source.sourceId === "codex_reset_posts");
  const feedAge = feed ? now - Date.parse(feed.fetchedAt) : NaN;
  const postsStale = !!feed && (failed || !!feedSource?.issue || !Number.isFinite(feedAge) || feedAge < -60_000 || feedAge >= 30 * 60_000);
  const confidence = value ? ({ low: ["低", "Low"], medium: ["中", "Medium"], high: ["高", "High"] }[value.confidence] ?? ["未说明", "Unspecified"])[zh ? 0 : 1] : "—";
  const statusLabel = noticeWithdrawn ? (zh ? "预告已更正" : "Announcement corrected")
    : notice?.state === "reported" ? (zh ? "已报告重置" : "Reset reported")
    : notice?.state === "elapsed" ? (zh ? "预告时间已到 · 等待确认" : "Scheduled time reached · awaiting confirmation")
    : notice?.conflictingTiming ? (zh ? "预告时间有分歧" : "Timing interpretations differ")
    : notice && (notice.stale || failed) ? (zh ? "上次预告 · 待更新" : "Saved announcement · update due")
    : announcement ? (zh ? "已明确预告" : "Explicitly announced") : (zh ? "重置预告" : "Reset announcement");
  const scope = notice?.cohort === "all paid ChatGPT accounts" && zh ? "所有付费 ChatGPT 账号" : notice?.cohort;
  const pendingNotice = !!notice && !announcement && (noticeWithdrawn || notice.explicit || notice.state !== "upcoming" || notice.conflictingTiming);
  const committed = !!announcement?.timing && announcement.timing.end <= now + hours * 3_600_000;
  const probability = hours === 24 ? probability24h : probability48h;
  const sourceProbability = value ? (hours === 24 ? value.probability24h : value.probability48h) : null;
  const windowLabel = zh ? `未来 ${hours} 小时` : `Next ${hours} hours`;
  const sourceStatus = stale ? (zh ? "上次结果 · 待更新" : "Last result · update due") : (zh ? "来源预测" : "Source forecast");
  const progressStage = noticeWithdrawn || notice?.state === "reported" ? 2 : notice?.state === "elapsed" ? 1 : 0;
  const checkedTimes = (timeline?.sources ?? []).flatMap((source) => source.lastAttemptAt ? [Date.parse(source.lastAttemptAt)] : []).filter(Number.isFinite);
  const checkedAt = checkedTimes.length ? new Date(Math.max(...checkedTimes)).toISOString() : value?.checkedAt;

  return <section className={styles.briefing} aria-label={zh ? "重置情报速览" : "Reset briefing"}>
    <article className={styles.forecast} data-stale={!announcement && stale || undefined} data-announced={announcement ? true : undefined}>
      <div className={styles.forecastHeading}>
        <div className={styles.byline}><Radio size={15} aria-hidden="true" /><h2>{announcement || pendingNotice
          ? (zh ? "Horizon · 当前判断" : "Horizon · current outlook") : (zh ? "来源预测" : "Source forecast")}</h2>
          <span className={styles.badge}>{notice ? (zh ? "公告追踪" : "Announcement tracking") : "Codex Reset"}</span></div>
        <div className={styles.periods} aria-label={zh ? "预测时间窗口" : "Forecast window"}>
          {([24, 48] as const).map((period) => <button key={period} aria-pressed={period === hours} onClick={() => setHours(period)}>{period}h</button>)}
        </div>
      </div>
      <div className={styles.forecastBody}>
        <div className={styles.outlook}>
          {pendingNotice ? <p className={styles.pending} role="status">{statusLabel}</p> : <>
            <div className={styles.probability}><strong>{probability == null ? "—" : `${probability}%`}</strong><span>{windowLabel}<small>{committed ? (zh ? "按明确预告判定" : "Based on the announcement") : sourceStatus}</small></span></div>
            {announcement && <p className={styles.outlookCopy}>{statusLabel}</p>}
          </>}
          {notice ? <div className={styles.noticeMeta}>
            {scope && <p>{zh ? "适用范围：" : "For: "}{scope}</p>}
            <div className={shared.sourceRow}><EvidenceLink url={notice.url} label={zh ? "Tibo 原帖" : "Tibo’s announcement"} onOpen={onOpenSource} />
              <span>{zh ? "发布 " : "Published "}{publicEvidenceTime(new Date(notice.publishedAt).toISOString(), language)}</span></div>
            {!latest && <p>{notice.excerpt}</p>}
          </div> : <p className={styles.outlookCopy}>{zh ? "公共重置的概率估计，来自 Codex Reset。" : "Estimated probability of a public reset, from Codex Reset."}</p>}
        </div>
        <div className={styles.progress}>
          {notice ? <>
            <div className={styles.cardHeading}><h3>{zh ? "本次重置进程" : "Announcement progress"}</h3><span>{zh ? "本地时间" : "Local time"}</span></div>
            <ol className={styles.steps} data-corrected={noticeWithdrawn || undefined}>
              {(zh ? ["预告发布", "预告时间", noticeWithdrawn ? "已更正" : "报告完成"] : ["Announced", "Scheduled", noticeWithdrawn ? "Corrected" : "Reported complete"]).map((label, index) => <li key={index} data-reached={(noticeWithdrawn ? index !== 1 : index <= progressStage) || undefined} aria-current={index === progressStage ? "step" : undefined}>
                <span className={styles.stepDot}>{!noticeWithdrawn && (index < progressStage || (index === 2 && progressStage === 2)) ? <Check size={11} aria-hidden="true" /> : <i />}</span><span>{label}</span>
              </li>)}
            </ol>
            <div className={styles.announcementSummary}>
              {notice.reportedAt != null && <div className={styles.announcementTime}><Clock3 size={14} aria-hidden="true" /><span>{zh ? "报告完成" : "Reported complete"}</span><time>{publicEvidenceTime(new Date(notice.reportedAt).toISOString(), language)}</time></div>}
              {notice.timing && <div className={styles.announcementTime}><Clock3 size={14} aria-hidden="true" /><span>{zh ? "预告时间" : "Scheduled"}</span><time>{publicEvidenceTime(new Date(notice.timing.end).toISOString(), language)}</time></div>}
              {!notice.timing && <p>{zh ? "等待来源补充明确时间" : "Waiting for an explicit time from the source"}</p>}
            </div>
          </> : <>
            <div className={styles.cardHeading}><h3>{zh ? "按时间窗口看预测" : "Forecast by time window"}</h3><TrendingUp size={15} aria-hidden="true" /></div>
            <div className={styles.windowBars}>
              <ProbabilityBar label={zh ? "未来 24 小时" : "Next 24 hours"} value={probability24h} muted={stale} />
              <ProbabilityBar label={zh ? "未来 48 小时" : "Next 48 hours"} value={probability48h} muted={stale} />
            </div>
            {value?.lastResetAt && <p className={styles.meta}>{zh ? "来源记录的上次重置 · " : "Last reset in source · "}{publicEvidenceTime(value.lastResetAt, language)}</p>}
          </>}
        </div>
      </div>
    </article>

    <div className={styles.detailGrid}>
      <article className={styles.card}>
        <div className={styles.cardHeading}><h2>{zh ? "各方怎样判断" : "How sources compare"}</h2><span>{windowLabel}</span></div>
        <div className={styles.comparison}>
          {notice && <div className={styles.sourceEstimate}>
            <ProbabilityBar label={zh ? "Horizon 公告判定" : "Horizon announcement rule"} value={committed ? 100 : null} />
            <p>{announcement ? (committed ? (zh ? "明确预告覆盖此时间窗口" : "An explicit announcement covers this window") : (zh ? "预告时间在此窗口以外" : "The announced time falls outside this window")) : statusLabel}</p>
          </div>}
          <div className={styles.sourceEstimate}>
            <ProbabilityBar label="Codex Reset" value={sourceProbability} muted={stale} />
            <p>{zh ? "网站原始预测" : "Original source forecast"}{value && ` · ${zh ? "自评置信度" : "Source confidence"} ${confidence}`}</p>
          </div>
        </div>
        <div className={styles.sourceWindows}><span>{zh ? "来源原值" : "Source estimates"}</span><span>24h <b>{value ? `${value.probability24h}%` : "—"}</b></span><span>48h <b>{value ? `${value.probability48h}%` : "—"}</b></span></div>
        {announcement && <p className={styles.meta}>{committed
          ? (zh ? "本次采用明确预告，来源预测保留原值。" : "The announcement leads this outlook; source estimates are unchanged.")
          : (zh ? "所选窗口保留来源预测；预告时间见上方。" : "This window retains the source forecast; see the announced time above.")}</p>}
        {stale && <p className={styles.correction}>{zh ? "来源预测 · 上次结果待更新" : "Source forecast · saved result, update due"}</p>}
        {value ? <>
          <div className={styles.sourceMeta}><EvidenceLink url="https://codex-reset.com" label="Codex Reset" onOpen={onOpenSource} /><span>{zh ? "网站更新 " : "Source updated "}{publicEvidenceTime(value.updatedAt, language)}</span></div>
          {signalWithdrawn && <p className={styles.correction}>{zh ? "关联线索已更正；网站预测待复核。" : "The related lead was corrected; the source forecast needs review."}</p>}
          <details className={styles.signal}><summary>{zh ? "预测说明" : "About this forecast"}</summary>
            {notice && <p>{zh ? "Horizon：有效明确预告覆盖的时间窗口按 100% 判定；到期后追踪确认与更正。" : "Horizon assigns 100% to windows covered by an active explicit announcement, then tracks confirmation and corrections."}</p>}
            {notice?.timing && <p>{zh ? "时间换算依据：" : "Time interpretation: "}{notice.timing.timeZone} · QuotaResets</p>}
            {notice?.cohort && <p>{zh ? "来源范围原文：" : "Original scope: "}{notice.cohort}</p>}
            <p>{zh ? `网站自评置信度：${confidence}` : `Source confidence: ${confidence}`}</p>
            <p>{zh ? "本机读取：" : "Fetched: "}{publicEvidenceTime(value.checkedAt, language)}</p>
            {value.signalScore != null && <><p>{zh ? `网站另有信号评分 ${value.signalScore}/100` : `Separate source signal score: ${value.signalScore}/100`}
              {signalWithdrawn ? (zh ? " · 线索已有更正" : " · Lead corrected") : signalActive ? (zh ? " · 关注中" : " · Watching") : (zh ? " · 旧线索 / 待更新" : " · Older lead / update due")}</p>
              <p>{zh ? "发言强度评分，满分 100。" : "Statement-strength score, out of 100."}</p>
              {value.signalDeadline && <p>{zh ? "网站给出的参考期限：" : "Source reference deadline: "}{publicEvidenceTime(value.signalDeadline, language)}</p>}
              {value.signalUrl && <EvidenceLink url={value.signalUrl} label={zh ? "评分所依据的原帖" : "Post behind this score"} onOpen={onOpenSource} />}
            </>}
            <p><EvidenceLink url="https://codex-reset.com/forecast-method" label={zh ? "网站计算方法" : "Source methodology"} onOpen={onOpenSource} /></p>
          </details>
        </> : <p className={styles.meta} role="status">{timeline?.insights?.forecast.issue || failed
          ? (zh ? "预测来源暂不可用，将自动重试。" : "The forecast source is unavailable. Retrying automatically.")
          : (zh ? "正在获取网站预测…" : "Fetching the source forecast…")}</p>}
      </article>
      <article className={styles.card}>
        <div className={styles.cardHeading}><h2>{zh ? "Tibo · 相关发言" : "Tibo · related statements"}</h2><span>{zh ? "Codex Reset 收录" : "Via Codex Reset"}</span></div>
        {postsStale && <p className={styles.correction}>{zh ? "发言来源待更新，显示上次正文。" : "Post source update due; showing saved text."}</p>}
        {latest ? <PostConversation key={latest.id} post={latest} timeline={timeline} language={language} onOpenSource={onOpenSource} onRead={onRead} />
          : <p className={styles.meta}>{zh ? "暂无相关发言记录。" : "No matching posts collected yet."}</p>}
      </article>
    </div>

    {posts.some((post) => post.id !== latest?.id) && <article className={styles.card}>
      <div className={styles.cardHeading}><h2>{zh ? "关键动态" : "Key updates"}</h2>
        <div className={styles.periods} aria-label={zh ? "动态范围" : "Update filter"}>
          <button aria-pressed={!all} onClick={() => { setAll(false); setLimit(3); }}>{zh ? "重置相关" : "Reset-related"}</button>
          <button aria-pressed={all} onClick={() => { setAll(true); setLimit(3); }}>{zh ? "全部动态" : "All posts"}</button>
        </div>
      </div>
      <div className={styles.updates}>{otherPosts.slice(0, limit).map((post) => <PostUpdate key={post.id} post={post} timeline={timeline} language={language} onOpenSource={onOpenSource} onRead={onRead} />)}</div>
      {otherPosts.length === 0 && <p className={styles.meta}>{zh ? "暂无其他相关发言。" : "No other matching posts."}</p>}
      {otherPosts.length > limit && <button className={styles.moreButton} onClick={() => setLimit((count) => count + 4)}>{zh ? "更多发言" : "More statements"}<ChevronDown size={13} aria-hidden="true" /></button>}
    </article>}
    <footer className={styles.health} aria-label={zh ? "公开来源状态" : "Public source health"}>
      <div><span>{zh ? "自动采集 · 每 15 分钟" : "Auto-updates every 15 minutes"}</span>{checkedAt && <span>{zh ? "最近检查 " : "Last checked "}{publicEvidenceTime(checkedAt, language)}</span>}</div>
      <div>{timeline?.sources.map((source) => {
        const successAt = Date.parse(source.lastSuccessAt ?? "");
        const needsAttention = failed || !!source.issue || source.rejectedRecords > 0
          || !Number.isFinite(successAt) || successAt > now || now - successAt >= 15 * 60_000;
        return <span key={source.sourceId} className={styles.healthSource} data-issue={needsAttention || undefined}
        title={`${SOURCE_NAMES[source.sourceId]} · ${publicEvidenceTime(source.lastSuccessAt, language)}`}>
        <i aria-hidden="true" />{SOURCE_NAMES[source.sourceId]} · {failed ? (zh ? "显示缓存" : "Cached") : publicSourceLabel(source, language, now)}
      </span>; })}</div>
    </footer>
  </section>;
}
