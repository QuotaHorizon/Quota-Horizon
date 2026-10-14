import { useState } from "react";
import { ArrowRight, History } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { publicEvidenceTime } from "./presentation";
import { calendarEventTime, exactPublicTime } from "./resetCalendarModel";
import { isNewResetProgress, resetChangeLabel, resetSourceChanges, resetStageLabel, resetTopicLabel, type ResetSourceChange } from "./resetChangesModel";
import { sourceDateBounds } from "./upcomingResetModel";
import type { PublicReadReceipt, PublicResetTimeline, PublicRevisionChange } from "./types";
import { useVisibleResetRead } from "./visibleResetRead";
import styles from "./resetChanges.module.less";

function Version({ signal, label, language, showTitle }: { signal: PublicRevisionChange["current"]; label: string; language: Language; showTitle: boolean }) {
  const zh = language === "zh";
  const hint = signal.announcementTiming;
  const exactTarget = exactPublicTime(hint?.expectedAt);
  const bounds = hint?.expectedOn ? sourceDateBounds(hint.expectedOn, hint.timeZone) : null;
  return <div className={styles.version}>
    <strong>{label}</strong>
    {showTitle && <span>{zh ? "来源标题：" : "Source title: "}{signal.title}</span>}
    <p>{signal.summary}</p>
    <span>{zh ? "原帖发布时间：" : "Post published: "}{calendarEventTime(exactPublicTime(signal.source.publishedAt), language)}</span>
    {exactTarget != null && <span>{zh ? "明确承诺时间（本地）：" : "Promised time (local): "}{calendarEventTime(exactTarget, language)}</span>}
    {hint?.cohort && <span>{zh ? "承诺适用范围：" : "Announced for: "}{hint.cohort}</span>}
    {signal.occurredAt && <span>{zh ? "来源标注的发生/确认时间（待核实）：" : "Source-reported occurrence/confirmation (unverified): "}{calendarEventTime(exactPublicTime(signal.occurredAt), language)}</span>}
    {bounds && <span>{zh ? "追踪站预计日期，换算到本地：" : "Tracker’s expected day in local time: "}
      {publicEvidenceTime(new Date(bounds.start).toISOString(), language)}{zh ? " 至 " : " to "}{publicEvidenceTime(new Date(bounds.end).toISOString(), language)}
      {zh ? "（不含结束时刻）" : " (end exclusive)"}</span>}
  </div>;
}

function ChangeRow({ change, language, onOpenSource, onRead }: { change: ResetSourceChange; language: Language; onOpenSource?: (url: string) => void; onRead?: (receipts: PublicReadReceipt[]) => void }) {
  const zh = language === "zh";
  const { current, previous } = change;
  const [open, setOpen] = useState(false);
  const seen = useVisibleResetRead(change.unread ? [{ signalId: current.signalId, revision: current.revision }] : [], open ? onRead : undefined);
  const showTitle = !!previous && previous.title !== current.title;
  const left = previous ? change.kind === "kind" ? resetTopicLabel(previous, language) : resetStageLabel(change.previousStage!, language)
    : zh ? "尚未收录" : "Not collected";
  const right = change.kind === "kind" ? resetTopicLabel(current, language) : resetStageLabel(change.stage, language);
  return <li className={styles.change} data-warning={change.withdrawn || undefined}>
    <div className={styles.heading}><h3>{resetChangeLabel(change, language)}</h3><span>{resetTopicLabel(current, language)}</span>
      {isNewResetProgress(change) && <span className={styles.unread}>{zh ? "新进展" : "New update"}</span>}</div>
    <div className={styles.transition}>{change.kind === "text" || change.kind === "timing"
      ? <span>{zh ? "状态未变：" : "Status unchanged: "}</span>
      : <><span>{left}</span><ArrowRight size={14} aria-hidden="true" /></>}<strong>{right}</strong></div>
    {change.withdrawn && <p className={styles.warning}>{zh ? "旧预告已失效，展开查看更正。" : "The earlier notice is no longer active. Expand to see the correction."}</p>}
    {change.kind === "timing" && <p>{zh ? "来源时间已调整，可展开对比。" : "Source timing changed. Expand to compare."}</p>}
    <div className={styles.meta}><span>{zh ? "本机记录：" : "Recorded here: "}<time dateTime={current.recordedAt}>{publicEvidenceTime(current.recordedAt, language)}</time></span>
      <span>{current.source.author}</span>{change.sources.length > 1 && <span>{zh ? "相同原文已合并转载" : "Mirrors of the same original merged"}</span>}
      <EvidenceLink url={current.source.canonicalUrl} label={zh ? "原始来源" : "Original source"} onOpen={onOpenSource} /></div>
    <details className={styles.details} onToggle={(event) => setOpen(event.currentTarget.open)}><summary>{previous ? (zh ? "对比前后内容与时间" : "Compare wording and timing") : (zh ? "原文与原始时间" : "Source text and original time")}</summary>
      <div className={styles.versions} ref={seen}>
        {previous && <Version signal={previous} label={zh ? "此前记录" : "Previous record"} language={language} showTitle={showTitle} />}
        <Version signal={current} label={previous ? (zh ? "此版本记录" : "This revision") : (zh ? "首次收录内容" : "First collected text")} language={language} showTitle={showTitle} />
      </div>
    </details>
  </li>;
}

export function ResetChanges({ timeline, language, now, onOpenSource, onRead }: {
  timeline: PublicResetTimeline | null; language: Language; now: number; onOpenSource?: (url: string) => void;
  onRead?: (receipts: PublicReadReceipt[]) => void;
}) {
  const zh = language === "zh";
  const [expanded, setExpanded] = useState(false);
  const [onlyUnread, setOnlyUnread] = useState(false);
  const changes = resetSourceChanges(timeline, now);
  const unreadCount = changes.filter(isNewResetProgress).length;
  if (!timeline?.changes) return null;
  const selected = onlyUnread ? changes.filter(isNewResetProgress) : changes;
  const shown = expanded ? selected.slice(0, 24) : selected.slice(0, 3);
  const truncated = timeline.changes.totalCount > timeline.changes.items.length || (expanded && selected.length > shown.length);
  return <section className={styles.panel} aria-labelledby="reset-changes-title">
    <h2 id="reset-changes-title" tabIndex={-1}><History size={18} aria-hidden="true" />{zh ? "情报有什么变化" : "What changed"}</h2>
    {(unreadCount > 0 || onlyUnread) && <button className={styles.more} aria-pressed={onlyUnread} onClick={() => setOnlyUnread((value) => !value)}>
      {onlyUnread ? (zh ? "查看全部近期变化" : "Show all recent changes") : (zh ? "只看新进展" : "Only new updates")}</button>}
    <p className={styles.note}>{zh ? "追踪来源的更新，按本机收录时间排列。" : "Source updates, ordered by local collection time."}</p>
    {shown.length ? <ol className={styles.list}>{shown.map((change) => <ChangeRow key={change.key} change={change} language={language} onOpenSource={onOpenSource} onRead={onRead} />)}</ol>
      : <p className={styles.note}>{onlyUnread ? (zh ? "近期新进展已全部读完。" : "All recent updates are read.")
        : (zh ? "暂无重置相关更新。" : "No reset-related updates yet.")}</p>}
    {selected.length > 3 && <button className={styles.more} onClick={() => setExpanded((value) => !value)} aria-expanded={expanded}>
      {expanded ? (zh ? "收起变化" : "Show fewer changes") : (zh ? `展开近期变化（${Math.min(selected.length, 24)} 条）` : `Show recent changes (${Math.min(selected.length, 24)})`)}</button>}
    {truncated && <p className={styles.note}>{zh ? "显示近期变化，较早版本已存档。" : "Showing recent changes. Earlier revisions are archived."}</p>}
  </section>;
}
