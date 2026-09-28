import { useState } from "react";
import { MessageCircle, TrendingUp } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { publicEvidenceTime } from "./presentation";
import { forecastPresentation, postReading, resetRelated } from "./resetBriefingModel";
import type { PublicPost, PublicReadReceipt, PublicResetTimeline } from "./types";
import { postReadReceipts, useVisibleResetRead } from "./visibleResetRead";
import shared from "./index.module.less";
import styles from "./resetBriefing.module.less";

function PostConversation({ post, timeline, language, onOpenSource, onRead }: {
  post: PublicPost; timeline: PublicResetTimeline | null; language: Language; onOpenSource?: (url: string) => void;
  onRead?: (receipts: PublicReadReceipt[]) => void;
}) {
  const zh = language === "zh";
  const reading = postReading(post, language, timeline);
  const translated = zh && post.translatedText && post.translatedText.trim() !== post.text.trim();
  const seen = useVisibleResetRead(postReadReceipts(timeline, post), onRead);
  return <article className={styles.post} data-tone={reading.tone} ref={seen}>
    <div className={shared.sectionToolbar}><h3>{reading.title}</h3><time>{publicEvidenceTime(post.publishedAt, language)}</time></div>
    <p className={styles.meaning}>{reading.meaning}</p>
    {post.isReply && <div className={styles.parent}>
      <div className={styles.byline}><MessageCircle size={13} aria-hidden="true" /><strong>{zh ? "父帖" : "Parent post"}{post.parent && ` · @${post.parent.author}`}</strong>
        {post.parent && <EvidenceLink url={post.parent.url} label={zh ? "父帖来源" : "Parent source"} onOpen={onOpenSource} />}</div>
      <p>{post.parent?.text ?? (zh ? "父帖暂未取得，可打开原帖查看上文。" : "The parent is unavailable. Open the original post for context.")}</p>
    </div>}
    <div className={styles.reply}>
      <div className={styles.byline}><strong>{post.isReply ? (zh ? "Tibo 回复" : "Tibo’s reply") : (zh ? "Tibo 原帖" : "Tibo’s post")}</strong>
        <EvidenceLink url={post.url} label={zh ? "原帖来源" : "Original post"} onOpen={onOpenSource} /></div>
      <p>{translated ? post.translatedText : post.text}</p>
      {translated && <details><summary>查看原文 · 译文来自 Codex Reset</summary><p>{post.text}</p></details>}
    </div>
  </article>;
}

export function ResetBriefing({ timeline, language, now, failed = false, onOpenSource, onRead }: {
  timeline: PublicResetTimeline | null; language: Language; now: number; failed?: boolean; onOpenSource?: (url: string) => void;
  onRead?: (receipts: PublicReadReceipt[]) => void;
}) {
  const zh = language === "zh";
  const { value, stale, signalActive, signalWithdrawn } = forecastPresentation(timeline?.insights, now, failed, timeline);
  const feed = timeline?.insights?.posts;
  const posts = feed?.posts ?? [];
  const [all, setAll] = useState(false);
  const [more, setMore] = useState(false);
  const [limit, setLimit] = useState(3);
  const latest = posts.find(resetRelated);
  const otherPosts = (all ? posts : posts.filter(resetRelated)).filter((post) => post.id !== latest?.id);
  const feedSource = timeline?.sources.find((source) => source.sourceId === "codex_reset_posts");
  const feedAge = feed ? now - Date.parse(feed.fetchedAt) : NaN;
  const postsStale = !!feed && (failed || !!feedSource?.issue || !Number.isFinite(feedAge) || feedAge < -60_000 || feedAge >= 30 * 60_000);
  const confidence = value ? ({ low: ["低", "Low"], medium: ["中", "Medium"], high: ["高", "High"] }[value.confidence] ?? ["未说明", "Unspecified"])[zh ? 0 : 1] : "—";
  return <section className={styles.briefing} aria-label={zh ? "重置情报速览" : "Reset briefing"}>
      <article className={styles.forecast} data-stale={stale || undefined}>
        <div className={styles.byline}><TrendingUp size={16} aria-hidden="true" /><h3>{zh ? "第三方重置预测" : "Third-party reset forecast"}</h3>
          {stale && <span>{zh ? "上次结果 · 待更新" : "Last result · update due"}</span>}</div>
        <div className={styles.probabilities}>{([24, 48] as const).map((hours) => <div key={hours}>
          <span>{zh ? `未来 ${hours} 小时` : `Next ${hours} hours`}</span>
          <strong>{value ? `${hours === 24 ? value.probability24h : value.probability48h}%` : "—"}</strong>
          <div className={styles.track} aria-hidden="true"><i style={{ width: `${value ? hours === 24 ? value.probability24h : value.probability48h : 0}%` }} /></div>
        </div>)}</div>
        {value ? <>
          <div className={shared.sourceRow}><EvidenceLink url="https://codex-reset.com" label="Codex Reset" onOpen={onOpenSource} />
            <span>{zh ? "网站更新 " : "Source updated "}{publicEvidenceTime(value.updatedAt, language)}</span></div>
          {signalWithdrawn && <p className={styles.correction}>{zh ? "关联线索已更正；网站预测待复核。"
            : "The related lead was corrected; the source forecast needs review."}</p>}
          <details className={styles.signal}><summary>{zh ? "预测说明" : "About this forecast"}</summary>
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
        </> : <p role="status">{timeline?.insights?.forecast.issue || failed
          ? (zh ? "预测来源暂不可用，将自动重试。" : "The forecast source is unavailable. Retrying automatically.")
          : (zh ? "正在获取网站预测…" : "Fetching the source forecast…")}</p>}
      </article>
    <div className={shared.sectionToolbar}><h2><MessageCircle size={18} aria-hidden="true" />{zh ? "Tibo · 相关发言" : "Tibo · related statements"}</h2>
      <span className={styles.feedTime}>{zh ? "Codex Reset 收录 · 本地时间" : "Collected by Codex Reset · local time"}</span></div>
    {postsStale && <p className={shared.note}>{zh ? "发言来源待更新，显示上次正文。" : "Post source update due; showing saved text."}</p>}
    {latest ? <PostConversation post={latest} timeline={timeline} language={language} onOpenSource={onOpenSource} onRead={onRead} />
      : <p className={shared.note}>{zh ? "暂无相关发言记录。" : "No matching posts collected yet."}</p>}
    {posts.some((post) => post.id !== latest?.id) && <details className={shared.sourceDisclosure} onToggle={(event) => setMore(event.currentTarget.open)}>
      <summary>{zh ? "更多发言" : "More statements"}</summary>
      {more && <>
        <div className={shared.filters}><button aria-pressed={!all} onClick={() => { setAll(false); setLimit(3); }}>{zh ? "重置相关" : "Reset-related"}</button>
          <button aria-pressed={all} onClick={() => { setAll(true); setLimit(3); }}>{zh ? "全部动态" : "All posts"}</button></div>
        <div className={styles.posts}>{otherPosts.slice(0, limit).map((post) => <PostConversation key={post.id} post={post} timeline={timeline} language={language} onOpenSource={onOpenSource} onRead={onRead} />)}</div>
        {otherPosts.length > limit && <button className={shared.refreshButton} onClick={() => setLimit((value) => value + 4)}>{zh ? "显示更多" : "Show more"}</button>}
      </>}
    </details>}
  </section>;
}
