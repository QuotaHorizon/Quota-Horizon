import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { evidenceDispositionLabel, publicEvidenceTime } from "./presentation";
import type { PublicSourcesState } from "./publicSourcesStore";
import { publicSourcesRuntime as store } from "./publicSourcesRuntime";
import type { PublicReadReceipt, PublicResetArchive, PublicSourceId, PublicSourceStatus, PublicTimelineEntry } from "./types";
import { ResetCalendar } from "./ResetCalendar";
import { UpcomingReset } from "./UpcomingReset";
import { ResetBriefing } from "./ResetBriefing";
import { ResetChanges } from "./ResetChanges";
import { unreadResetProgress } from "./resetChangesModel";
import styles from "./index.module.less";

const SOURCE_NAMES: Record<PublicSourceId, string> = {
  quotaresets: "QuotaResets", codex_reset: "Codex Reset", codex_reset_posts: "Codex Reset · Tibo", openai_status: "OpenAI Status",
};

export function publicSourceLabel(source: PublicSourceStatus, language: Language, now: number) {
  const zh = language === "zh";
  if (source.issue) {
    const reasons: Record<string, [string, string]> = {
      request_failed: ["连接失败", "Connection failed"], http_error: ["来源暂不可用", "Source unavailable"],
      schema_changed: ["来源格式已变化", "Source format changed"], invalid_response: ["资料未通过校验", "Invalid source data"],
      response_too_large: ["资料超出读取上限", "Source exceeds read limit"],
      upstream_stale: ["来源缓存尚未更新", "Upstream cache is stale"],
    };
    return (reasons[source.issue] ?? ["暂不可用", "Unavailable"])[zh ? 0 : 1];
  }
  if (!source.lastSuccessAt) return zh ? "尚未读取" : "Not fetched yet";
  const age = now - Date.parse(source.lastSuccessAt);
  if (!Number.isFinite(age) || age < 0) return zh ? "需核对时间" : "Check timestamp";
  if (age >= 15 * 60_000) return zh ? "等待更新" : "Update due";
  return source.rejectedRecords > 0 ? (zh ? "部分资料可用" : "Partially available") : (zh ? "更新成功" : "Updated");
}

function LeadCard({ entry, language, onOpenSource }: {
  entry: PublicTimelineEntry; language: Language; onOpenSource?: (url: string) => void;
}) {
  const zh = language === "zh";
  const { signal } = entry;
  // This channel cannot promote a tracker label to a reviewed confirmation,
  // including when a future backend accidentally sends the stronger label.
  const disposition = ["confirmed", "announced", "possible"].includes(entry.disposition) ? "needs_review" : entry.disposition;
  return <article className={styles.evidenceCard}>
    <div className={styles.evidenceHeading}>
      <h3>{signal.title}</h3><span className={styles.badge}>{evidenceDispositionLabel(disposition, language)}</span>
    </div>
    <p className={styles.originalText}>{signal.summary}</p>
    <div className={styles.sourceRow}>
      <span>{zh ? "来源原文" : "Source text"}</span><span>{signal.source.author}</span>
      {signal.source.publishedAt && <span>{publicEvidenceTime(signal.source.publishedAt, language)}</span>}
      <EvidenceLink url={signal.source.canonicalUrl} label={zh ? "查看原始来源" : "Open original source"} onOpen={onOpenSource} />
    </div>
    <details className={styles.provenance}>
      <summary>{zh ? "来源与修订记录" : "Provenance and revision"}</summary>
      <p>{signal.scope === "personal"
        ? (zh ? "追踪网站收录的个人账号报告。" : "An individual account report collected by the tracker.")
        : (zh ? "追踪网站收录，原文待核实。" : "Collected by a tracker; original text awaits review.")}</p>
      <p>{zh ? `本地第 ${signal.revision} 版 · 此版本首次采集：` : `Local revision ${signal.revision} · first collected: `}{publicEvidenceTime(signal.source.collectedAt, language)}</p>
      {!signal.source.publishedAt && <p>{zh ? "发布时间未知，按收录时间排序。" : "Publication time unknown; sorted by collection time."}</p>}
      <div className={styles.sourceRow}>{signal.source.discoveredVia.map((url, index) => <EvidenceLink key={url} url={url}
        label={zh ? `采集来源 ${index + 1}` : `Collection source ${index + 1}`} onOpen={onOpenSource} />)}</div>
    </details>
  </article>;
}

export function LiveSourcesView({ state, archive = null, archiveContent, language, now, onRefresh, onOpenSource, onMarkRead, onRead, initialSection = "overview" }: {
  state: PublicSourcesState; archive?: PublicResetArchive | null; archiveContent?: ReactNode; language: Language; now: number; onRefresh?: () => void; onOpenSource?: (url: string) => void;
  onMarkRead?: () => void; onRead?: (receipts: PublicReadReceipt[]) => void;
  initialSection?: "overview" | "history" | "details";
}) {
  const zh = language === "zh";
  const { timeline, busy, failed } = state;
  const unreadCount = unreadResetProgress(timeline, now).length;
  const [section, setSection] = useState(initialSection);
  const [filter, setFilter] = useState<"leads" | "context" | "all">("leads");
  const [limit, setLimit] = useState(6);
  const entries = (timeline?.entries ?? []).filter((entry) => filter === "all"
    || (filter === "context" ? entry.disposition === "context" : entry.disposition !== "context"));
  const checkedTimes = (timeline?.sources ?? []).flatMap((source) => source.lastAttemptAt ? [Date.parse(source.lastAttemptAt)] : []).filter(Number.isFinite);
  const nextCheck = checkedTimes.length ? Math.min(...checkedTimes) + 15 * 60_000 : null;
  return <div className={styles.section}>
    <div className={styles.sectionToolbar}>
      <div className={styles.filters} aria-label={zh ? "雷达内容" : "Radar sections"}>
        {(["overview", "history", "details"] as const).map((value, index) => <button key={value} aria-pressed={section === value}
          onClick={() => setSection(value)}>{(zh ? ["概览", "重置历史", "详细资料"] : ["Overview", "Reset history", "Details"])[index]}</button>)}
      </div>
      <div className={styles.toolbarActions}>
        {(unreadCount > 0 || state.readFailed) && onMarkRead && <button className={styles.refreshButton} disabled={state.readBusy} onClick={onMarkRead}>
          {state.readBusy ? (zh ? "正在保存…" : "Saving…") : (zh ? "全部已读" : "Mark all read")}</button>}
        <button className={styles.refreshButton} disabled={busy} onClick={onRefresh}><RefreshCw size={14} aria-hidden="true" />
          {busy ? (zh ? "正在更新…" : "Updating…") : (zh ? "刷新" : "Refresh")}</button>
      </div>
    </div>
    {state.readFailed && <p className={styles.feedError} role="alert">{zh ? "已读状态保存失败，请点“全部已读”重试。" : "Reading state could not be saved. Use Mark all read to retry."}</p>}
    {failed && <p className={styles.feedError} role="alert">{zh ? "资料更新失败，显示上次结果。可点击刷新重试。" : "Update failed. Showing saved results; refresh to retry."}</p>}
    {section === "overview" && <ResetBriefing timeline={timeline} language={language} now={now} failed={failed} onOpenSource={onOpenSource} onRead={state.readFailed ? undefined : onRead} />}
    {section === "history" && <ResetCalendar archive={archive} timeline={timeline} now={now} language={language} onOpenSource={onOpenSource} />}
    {section === "details" && <>
    <details className={styles.sourceDisclosure}><summary>{zh ? "近期预告的时间依据" : "Timing behind recent announcements"}</summary>
      <UpcomingReset timeline={timeline} language={language} failed={failed} onOpenSource={onOpenSource} />
    </details>
    <ResetChanges timeline={timeline} now={now} language={language} onOpenSource={onOpenSource}
      onRead={state.readFailed ? undefined : onRead} />
    {timeline && <div className={styles.sourceSummary} aria-label={zh ? "公开来源状态" : "Public source health"}>
      {timeline.sources.map((source) => <span key={source.sourceId} data-issue={!!source.issue || undefined}>
        {SOURCE_NAMES[source.sourceId] ?? (zh ? "公开来源" : "Public source")} · {publicSourceLabel(source, language, now)}</span>)}
    </div>}
    <p className={styles.note} role="status">{zh ? "后台每 15 分钟更新" : "Updates every 15 minutes in the background"}
      {nextCheck != null && (nextCheck > now
        ? (zh ? " · 下次约 " : " · Next around ") + publicEvidenceTime(new Date(nextCheck).toISOString(), language)
        : (zh ? " · 等待更新" : " · Update due"))}</p>
    {!timeline && <p className={styles.note} role="status">{busy ? (zh ? "正在读取上次资料…" : "Loading cached sources…") : (zh ? "尚无可读取的公开资料。" : "No public cache is available yet.")}</p>}
    <details className={styles.sourceDisclosure}>
      <summary>{zh ? "公开来源与原文资料" : "Sources and original records"}</summary>
    {timeline && <>
      <div className={styles.sourceGrid}>{timeline.sources.map((source) => <article className={styles.sourceCard} key={source.sourceId}>
        <div className={styles.sectionToolbar}><strong>{SOURCE_NAMES[source.sourceId] ?? (zh ? "公开来源" : "Public source")}</strong>
          <span className={styles.badge}>{publicSourceLabel(source, language, now)}</span></div>
        <p>{zh ? "上次成功：" : "Last success: "}{publicEvidenceTime(source.lastSuccessAt, language)}</p>
        {source.issue && <p>{zh ? "本次尝试：" : "Last attempt: "}{publicEvidenceTime(source.lastAttemptAt, language)}</p>}
        <p>{zh ? `${source.acceptedRecords} 条可用 · ${source.rejectedRecords} 条未通过校验` : `${source.acceptedRecords} usable · ${source.rejectedRecords} rejected`}</p>
        <div className={styles.sourceRow}><EvidenceLink url={source.sourceUrl} label={zh ? "公开数据地址" : "Public endpoint"} onOpen={onOpenSource} /></div>
      </article>)}</div>
      <div className={styles.sectionToolbar}>
        <p className={styles.note}>{zh ? `${timeline.entries.length} 条资料 · ${timeline.evidenceFamilyCount} 组原始来源 · ${timeline.revisionCount} 个版本`
          : `${timeline.entries.length} records · ${timeline.evidenceFamilyCount} original-source groups · ${timeline.revisionCount} revisions`}</p>
        <div className={styles.filters} aria-label={zh ? "筛选公开动态" : "Filter source updates"}>
          {(["leads", "context", "all"] as const).map((value, index) => <button key={value} aria-pressed={filter === value}
            onClick={() => { setFilter(value); setLimit(6); }}>{(zh ? ["重置线索", "背景动态", "全部"] : ["Reset leads", "Context", "All"])[index]}</button>)}
        </div>
      </div>
      <div className={styles.timeline}>{entries.slice(0, limit).map((entry) => <LeadCard key={entry.signal.signalId} entry={entry} language={language} onOpenSource={onOpenSource} />)}</div>
      {entries.length === 0 && <p className={styles.note}>{zh ? "暂无此类记录。" : "No matching records collected yet."}</p>}
      {entries.length > limit && <button className={styles.refreshButton} onClick={() => setLimit((value) => value + 6)}>{zh ? `显示更多（还有 ${entries.length - limit} 条）` : `Show more (${entries.length - limit} remaining)`}</button>}
    </>}
    </details>
    {archiveContent}
    </>}
  </div>;
}

export function LiveSourcesPanel({ archive, archiveContent, language, onOpenSource }: { archive: PublicResetArchive | null; archiveContent?: ReactNode; language: Language; onOpenSource?: (url: string) => void }) {
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const check = () => {
      setNow(Date.now());
      if (document.visibilityState === "visible") void store.refresh(false);
    };
    check();
    const timer = window.setInterval(check, 15 * 60_000);
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", check);
      document.removeEventListener("visibilitychange", check);
    };
  }, []);
  return <LiveSourcesView state={state} archive={archive} archiveContent={archiveContent} language={language} now={Math.max(now, Date.now())} onOpenSource={onOpenSource}
    onMarkRead={() => { if (state.timeline) void store.markRead(state.timeline); }}
    onRead={(receipts) => { void store.markReceipts(receipts); }}
    onRefresh={() => { setNow(Date.now()); void store.refresh(true); }} />;
}
