import { useState } from "react";
import { ChevronDown, MessageCircle } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { publicEvidenceTime } from "./presentation";
import { postReading } from "./resetBriefingModel";
import type { PublicPost, PublicReadReceipt, PublicResetTimeline } from "./types";
import { postReadReceipts, useVisibleResetRead } from "./visibleResetRead";
import styles from "./resetBriefing.module.less";

type PostProps = {
  post: PublicPost; timeline: PublicResetTimeline | null; language: Language;
  onOpenSource?: (url: string) => void; onRead?: (receipts: PublicReadReceipt[]) => void;
};

export function PostConversation({ post, timeline, language, onOpenSource, onRead }: PostProps) {
  const zh = language === "zh";
  const reading = postReading(post, language, timeline);
  const translated = zh && post.translatedText && post.translatedText.trim() !== post.text.trim();
  const seen = useVisibleResetRead(postReadReceipts(timeline, post), onRead);
  return <article className={styles.post} data-tone={reading.tone} ref={seen}>
    <div className={styles.postHeading}><h3>{reading.title}</h3><time dateTime={post.publishedAt}>{publicEvidenceTime(post.publishedAt, language)}</time></div>
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

export function PostUpdate(props: PostProps) {
  const { post, timeline, language } = props;
  const [open, setOpen] = useState(false);
  const reading = postReading(post, language, timeline);
  const unread = postReadReceipts(timeline, post).length > 0;
  return <div className={styles.update}>
    <button className={styles.updateButton} aria-expanded={open} onClick={() => setOpen(!open)}>
      <span className={styles.updateMarker} data-tone={reading.tone} data-unread={unread || undefined} aria-label={unread ? (language === "zh" ? "未读" : "Unread") : undefined} />
      <span className={styles.updateText}><strong>{reading.title}</strong><span>{language === "zh" ? post.translatedText || post.text : post.text}</span></span>
      <time dateTime={post.publishedAt}>{publicEvidenceTime(post.publishedAt, language)}</time><ChevronDown size={14} aria-hidden="true" />
    </button>
    {open && <PostConversation {...props} />}
  </div>;
}
