import { memo, useState } from "react";
import { ArrowUpRight, BookOpen, Radar, Ticket } from "lucide-react";
import type { Language } from "../../i18n";
import { evidenceDispositionLabel, publicEvidenceTime, publicEvidenceUrl, reviewedEvidence } from "./presentation";
import type { PublicResetArchive, PublicResetArchiveEntry } from "./types";
import styles from "./index.module.less";

interface ArchiveViewProps {
  archive: PublicResetArchive;
  language: Language;
  onOpenSource?: (url: string) => void;
}

export function EvidenceLink({ url, label, onOpen }: { url: string; label: string; onOpen?: (url: string) => void }) {
  const safeUrl = publicEvidenceUrl(url);
  if (!safeUrl) return null;
  return <a href={safeUrl} target="_blank" rel="noopener noreferrer" onClick={(event) => {
    if (onOpen) { event.preventDefault(); onOpen(safeUrl); }
  }}>{label}<ArrowUpRight size={13} aria-hidden="true" /></a>;
}

function EvidenceCard({ entry, language, onOpenSource }: {
  entry: PublicResetArchiveEntry;
  language: Language;
  onOpenSource?: (url: string) => void;
}) {
  const zh = language === "zh";
  const source = entry.signal.source;
  return <article className={styles.evidenceCard}>
    <div className={styles.evidenceHeading}>
      <h3>{entry.title[language]}</h3>
      <span className={styles.badge} data-tone={entry.disposition === "confirmed" ? "reviewed" : "neutral"}>
        {evidenceDispositionLabel(entry.disposition, language)}
      </span>
    </div>
    <p>{entry.summary[language]}</p>
    <div className={styles.sourceRow}>
      <span>{source.author}</span>
      {source.publishedAt && <span>{zh ? "来源标注时间：" : "Source timestamp: "}{publicEvidenceTime(source.publishedAt, language)}</span>}
      <EvidenceLink url={source.canonicalUrl} label={zh ? "原始来源" : "Original source"} onOpen={onOpenSource} />
    </div>
    <details className={styles.provenance}>
      <summary>{zh ? "查看核对依据" : "Review details"}</summary>
      <p>{source.review === "primary_reviewed"
        ? (zh ? "已核对官方原文。" : "Official original reviewed.")
        : (zh ? "间接来源，原文待核对。" : "Indirect source; original awaits review.")}</p>
      <div className={styles.interpretation}>{entry.interpretation[language]}</div>
      {!source.publishedAt && <p>{zh ? "原始发布时间未知。" : "Original publication time unknown."}</p>}
      <p>{zh ? "本次核对：" : "Reviewed: "}{publicEvidenceTime(source.collectedAt, language)}</p>
      {source.discoveredVia.length > 0 && <div className={styles.sourceRow}>
        {source.discoveredVia.map((url, index) => <EvidenceLink key={url} url={url}
          label={zh ? `追踪记录 ${index + 1}` : `Tracker record ${index + 1}`} onOpen={onOpenSource} />)}
      </div>}
    </details>
  </article>;
}

export const PublicResetArchiveView = memo(function PublicResetArchiveView({ archive, language, onOpenSource }: ArchiveViewProps) {
  const zh = language === "zh";
  const [filter, setFilter] = useState<"all" | "reviewed" | "leads">("all");
  const confirmed = archive.entries.filter((entry) => entry.disposition === "confirmed");
  const full = confirmed.find((entry) => entry.signal.kind === "global_full_reset");
  const banked = confirmed.find((entry) => entry.signal.kind === "global_banked_reset_grant");
  const visible = archive.entries.filter((entry) => filter === "all"
    || (filter === "reviewed" ? reviewedEvidence(entry) : !reviewedEvidence(entry)))
    .sort((left, right) => Date.parse(right.signal.source.publishedAt ?? right.signal.recordedAt)
      - Date.parse(left.signal.source.publishedAt ?? left.signal.recordedAt));

  return <>
    <section className={styles.archiveNotice} aria-label={zh ? "资料状态" : "Source status"}>
      <BookOpen size={19} aria-hidden="true" />
      <div><strong>{zh ? "应用附带资料" : "Bundled source archive"}</strong>
        <p>{zh ? "归档截至 " : "Archived through "}{publicEvidenceTime(archive.reviewedAt, language)}</p></div>
    </section>

    <section className={styles.section} aria-labelledby="public-reset-facts">
      <h2 id="public-reset-facts"><Radar size={19} aria-hidden="true" />{zh ? "公共重置消息" : "Public reset news"}</h2>
      <div className={styles.factGrid}>
        <article className={styles.factCard}>
          <span className={styles.kicker}>{zh ? "直接恢复用量窗口" : "Automatic quota refill"}</span>
          <h3>{zh ? "官方广泛重置" : "Broad official resets"}</h3>
          <p>{full ? full.title[language] : zh ? "这份资料中尚无已核实的完整重置公告。" : "No reviewed full-reset confirmation is included in this archive."}</p>
        </article>
        <article className={styles.factCard}>
          <span className={styles.kicker}><Ticket size={14} aria-hidden="true" />{zh ? "可保存、主动使用" : "Saved for manual use"}</span>
          <h3>{zh ? "重置卡发放" : "Banked-reset grants"}</h3>
          <p>{banked ? banked.title[language] : zh ? "这份资料中尚无已核实的发放说明。" : "No reviewed grant announcement is included."}</p>
        </article>
      </div>
    </section>

    <section className={styles.section} aria-labelledby="public-reset-evidence">
      <div className={styles.sectionToolbar}>
        <h2 id="public-reset-evidence">{zh ? "归档记录" : "Archived records"}</h2>
        <div className={styles.filters} aria-label={zh ? "筛选资料" : "Filter evidence"}>
          {(["all", "reviewed", "leads"] as const).map((value, index) => <button key={value}
            aria-pressed={filter === value} onClick={() => setFilter(value)}>
            {(zh ? ["全部", "已核对", "待核实"] : ["All", "Reviewed", "Leads"])[index]}
          </button>)}
        </div>
      </div>
      <p className={styles.note}>{zh ? `${archive.entries.length} 条资料 · ${archive.evidenceFamilyCount} 组原始来源`
        : `${archive.entries.length} records · ${archive.evidenceFamilyCount} original-source groups`}</p>
      <div className={styles.timeline}>
        {visible.map((entry) => <EvidenceCard key={entry.signal.signalId} entry={entry} language={language} onOpenSource={onOpenSource} />)}
        {visible.length === 0 && <p className={styles.note}>{zh ? "此筛选下没有资料。" : "No records match this filter."}</p>}
      </div>
    </section>
  </>;
});
