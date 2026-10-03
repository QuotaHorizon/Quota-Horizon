import { useState } from "react";
import { ChevronDown, MessageCircle } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { publicEvidenceTime } from "./presentation";
import { resetRelated, selectBriefingPost } from "./resetBriefingModel";
import { PostConversation } from "./ResetPosts";
import type { PublicReadReceipt, PublicResetTimeline } from "./types";
import type { UpcomingResetNotice } from "./upcomingResetModel";
import styles from "./resetBriefing.module.less";

export function TiboBriefing({ timeline, language, notice, now, failed, onOpenSource, onRead }: {
  timeline: PublicResetTimeline; language: Language; notice: UpcomingResetNotice | null; now: number; failed?: boolean;
  onOpenSource?: (url: string) => void; onRead?: (receipts: PublicReadReceipt[]) => void;
}) {
  const zh = language === "zh";
  const posts = [...(timeline.insights?.posts?.posts ?? [])].sort((a, b) => Date.parse(b.publishedAt) - Date.parse(a.publishedAt));
  const lead = selectBriefingPost(posts, notice) ?? posts[0];
  const [scope, setScope] = useState<"all" | "related" | "replies">("all");
  const [selected, setSelected] = useState<string | null>(null);
  const [limit, setLimit] = useState(4);
  const other = posts.filter(p => p.id !== lead?.id && (scope === "all" || (scope === "related" ? resetRelated(p) : p.isReply || /^@\w+/u.test(p.text))));
  const source = timeline.sources.find(s => s.sourceId === "codex_reset_posts");
  const fetched = timeline.insights?.posts?.fetchedAt;
  const stale = failed || !!source?.issue || !fetched || now - Date.parse(fetched) >= 30 * 60_000;
  const active = other.find(p => p.id === selected);
  return <article className={`${styles.card} ${styles.tiboCard}`} aria-label={zh ? "Tibo 发言与互动" : "Tibo statements and interactions"}>
    <div className={styles.cardHeading}><h2><MessageCircle size={17} aria-hidden="true" /> Tibo · {zh ? "发言与互动" : "Statements & interactions"}</h2>
      <div className={styles.tiboLinks}><EvidenceLink url="https://x.com/thsottiaux" label={zh ? "X 主页" : "X profile"} onOpen={onOpenSource} />
        <EvidenceLink url="https://x.com/thsottiaux/with_replies" label={zh ? "X 上的全部回复" : "All replies on X"} onOpen={onOpenSource} /></div>
    </div>
    {stale && <p className={styles.correction}>{zh ? "发言来源待更新，保留完整正文与原发布时间。" : "Post source update due; complete text and original publication times are retained."}</p>}
    <div className={styles.tiboGrid}>
      <div className={styles.tiboLead}><div className={styles.sectionLabel}>{zh ? "重置最新进展" : "Latest reset development"}</div>
        {lead ? <PostConversation key={lead.id} post={lead} timeline={timeline} language={language} onOpenSource={onOpenSource} onRead={onRead} />
          : <p className={styles.meta}>{zh ? "正在获取公开发言…" : "Fetching public statements…"}</p>}
      </div>
      <div className={styles.tiboActivity}>
        <div className={styles.activityHeading}><strong>{zh ? "其他动态与回复" : "Other posts and replies"}</strong><span>{posts.length} {zh ? "条已收录" : "collected"}</span></div>
        <div className={styles.periods} aria-label={zh ? "Tibo 动态范围" : "Tibo activity filter"}>{(["all", "related", "replies"] as const).map((v, i) => <button key={v} aria-pressed={scope === v} onClick={() => { setScope(v); setLimit(4); setSelected(null); }}>{(zh ? ["全部动态", "重置相关", "回复与互动"] : ["All posts", "Reset-related", "Replies & interactions"])[i]}</button>)}</div>
        <div className={styles.activityList}>{other.slice(0, limit).map(post => <button key={post.id} className={styles.activityItem} aria-pressed={selected === post.id} onClick={() => setSelected(selected === post.id ? null : post.id)}>
          <span>{zh ? post.translatedText || post.text : post.text}</span><span className={styles.activityMeta}>{post.parent ? `↳ @${post.parent.author}` : post.text.match(/^@\w+/u)?.[0] ?? (zh ? "公开发言" : "Public post")}<time dateTime={post.publishedAt}>{publicEvidenceTime(post.publishedAt, language)}</time></span>
        </button>)}</div>
        {!other.length && <p className={styles.meta}>{zh ? "本次收录中暂无其他匹配发言。" : "No other matching posts in this collection."}</p>}
        {other.length > limit && <button className={styles.moreButton} onClick={() => setLimit(limit + 6)}>{zh ? "更多动态" : "More posts"}<ChevronDown size={14} aria-hidden="true" /></button>}
      </div>
    </div>
    {active && <div className={styles.selectedConversation}><PostConversation key={active.id} post={active} timeline={timeline} language={language} onOpenSource={onOpenSource} onRead={onRead} /></div>}
    <p className={styles.tiboFootnote}>{zh ? "Codex Reset 收录 · 点击任一动态阅读完整原文、译文及已取得的父帖；X 入口可查看完整对话和更多互动。" : "Collected by Codex Reset. Select a post for its full text and available parent context; use X for the complete conversation and more interactions."}</p>
  </article>;
}
