import { useEffect, useMemo, useState, type ReactNode } from "react";
import { CalendarDays, MessageCircle, RotateCcw, Ticket } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { calendarEventTime, calendarTimeLabel, exactPublicTime, localCalendarDate, resetCalendarMonths,
  resetCalendarRecords, type CalendarKind, type CalendarMode, type ResetCalendarRecord } from "./resetCalendarModel";
import type { PublicResetArchive, PublicResetTimeline } from "./types";
import shared from "./index.module.less";
import styles from "./resetCalendar.module.less";

const kindLabel = (kind: CalendarKind, zh: boolean) => kind === "reset"
  ? (zh ? "强制重置记录" : "Full-reset record") : kind === "grant" ? (zh ? "重置卡发放" : "Reset-card grant") : (zh ? "预告与讨论" : "Announcements & discussion");

function CalendarRecord({ record, language, onOpenSource }: {
  record: ResetCalendarRecord; language: Language; onOpenSource?: (url: string) => void;
}) {
  const zh = language === "zh";
  return <article className={styles.record}>
    <div className={styles.recordHeading}>
      <strong>{record.kind === "reset" ? <RotateCcw size={14} aria-hidden="true" /> : record.kind === "grant" ? <Ticket size={15} aria-hidden="true" /> : <MessageCircle size={14} aria-hidden="true" />}{kindLabel(record.kind, zh)}</strong>
      <span className={shared.badge}>{record.withdrawn ? (zh ? "有更正、撤回或否定" : "Correction, retraction or denial") : record.conflictingTimes
        ? (zh ? "来源时间有分歧" : "Conflicting source times") : record.reviewed ? (zh ? "发生时间已核实" : "Reviewed occurrence")
          : record.timeBasis === "grant_announcement" ? (zh ? "已宣布开始发放" : "Rollout announced")
          : record.mode === "events" ? (zh ? "追踪记录 · 待核实" : "Tracker record · unverified") : (zh ? "预告 / 线索" : "Announcement / lead")}</span>
    </div>
    <time className={styles.eventTime} dateTime={record.time == null ? undefined : new Date(record.time).toISOString()}>{calendarEventTime(record.time, language)}</time>
    <p>{record.withdrawn || record.conflictingTimes
      ? (zh ? "日期待核对，来源详情见下方。" : "Date under review. See source details below.")
      : calendarTimeLabel(record.timeBasis, language)}</p>
    {record.timeBasis === "grant_announcement" && !record.withdrawn && !record.conflictingTimes && <p>{zh
      ? "Tibo 宣布开始向订阅账号发放重置卡。"
      : "Tibo announced a reset-card rollout to subscription accounts."}</p>}
    <details className={styles.recordDetails}>
      <summary>{zh ? "查看原文与时间依据" : "Source and timestamp details"}</summary>
      <p className={styles.originalTitle}>{record.title}</p>
      <p className={styles.originalSummary}>{record.summary}</p>
      {exactPublicTime(record.publishedAt) != null && <p>{zh ? "原文发布时间：" : "Original publication: "}{calendarEventTime(exactPublicTime(record.publishedAt), language)}</p>}
      {record.reportedAt && record.timeBasis === "published" && <p>{zh ? "来源另标时间（未确认发生）：" : "Additional source time (occurrence unconfirmed): "}{calendarEventTime(exactPublicTime(record.reportedAt), language)}</p>}
      {record.time == null && (record.reportedAt || record.publishedAt) && <p>{zh ? "来源标注时间：" : "Source timestamp: "}{record.reportedAt ?? record.publishedAt}</p>}
      <div className={shared.sourceRow}><EvidenceLink url={record.url} label={zh ? "打开原始来源" : "Open original source"} onOpen={onOpenSource} /></div>
      {record.sources.length > 1 && <div className={styles.sourceTimes}>{record.sources.map((source, index) => <div key={`${source.url}:${source.time}:${source.basis}`}>
        <p>{calendarTimeLabel(source.basis, language)} · {calendarEventTime(source.time, language)}</p>
        <div className={shared.sourceRow}><EvidenceLink url={source.url} label={zh ? `对照来源 ${index + 1}` : `Compare source ${index + 1}`} onOpen={onOpenSource} /></div>
      </div>)}</div>}
    </details>
  </article>;
}

export function ResetCalendar({ archive, timeline, now, language, action, onOpenSource }: {
  archive: PublicResetArchive | null; timeline: PublicResetTimeline | null; now: number; language: Language;
  action?: ReactNode; onOpenSource?: (url: string) => void;
}) {
  const zh = language === "zh";
  const [mode, setMode] = useState<CalendarMode | "all">("events");
  const [kind, setKind] = useState<"all" | CalendarKind>("all");
  const [reviewedOnly, setReviewedOnly] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [limit, setLimit] = useState(6);
  const zone = new Intl.DateTimeFormat().resolvedOptions().timeZone;
  const today = localCalendarDate(new Date(now));
  const months = useMemo(() => resetCalendarMonths(now), [today, zone]);
  const records = useMemo(() => resetCalendarRecords(archive?.entries ?? [], timeline?.entries ?? [], language, now),
    [archive, timeline, language, now]);
  const firstDay = months[0]?.days.find((day) => day != null)?.key ?? today;
  const matches = (record: ResetCalendarRecord) => (mode === "all" || record.mode === mode) && (kind === "all" || record.kind === kind)
    && (!reviewedOnly || record.reviewed);
  const byDay = new Map<string, ResetCalendarRecord[]>();
  for (const record of records) {
    if (record.time == null || !matches(record)) continue;
    const day = localCalendarDate(new Date(record.time));
    if (day < firstDay || day > today) continue;
    byDay.set(day, [...(byDay.get(day) ?? []), record]);
  }
  const fallback = [...byDay.keys()].sort().at(-1) ?? today;
  const selectedDay = selected && selected >= firstDay && selected <= today ? selected : fallback;
  useEffect(() => { if (!selected || selected < firstDay || selected > today) setSelected(fallback); }, [fallback, firstDay, selected, today]);
  const selectedRecords = byDay.get(selectedDay) ?? [];
  const missing = records.filter((record) => record.time == null && (kind === "all" || record.kind === kind));
  const countDays = (target: CalendarKind) => [...byDay.values()].filter((items) => items.some((item) => item.kind === target && item.mode === "events")).length;
  const changeFilters = () => { setSelected(null); setLimit(6); };
  const monthTitle = new Intl.DateTimeFormat(zh ? "zh-CN" : "en-US", { year: "numeric", month: "long" });
  return <section className={styles.calendar} aria-labelledby="reset-calendar-title">
    <div className={shared.sectionToolbar}>
      <h2 id="reset-calendar-title"><CalendarDays size={19} aria-hidden="true" />{zh ? "重置日期热图" : "Reset calendar"}</h2>{action}
    </div>
    <div className={styles.overview}>
      <div><strong>{zh ? "近三个月 · 含本月" : "Three months · including this month"}</strong>
        <p>{zh ? "本地时区：" : "Local time zone: "}<span>{zone}</span></p></div>
      <div className={styles.counts}>
        <span><RotateCcw size={14} aria-hidden="true" /><b>{countDays("reset")}</b>{zh ? "重置记录日" : "reset-record days"}</span>
        <span><Ticket size={15} aria-hidden="true" /><b>{countDays("grant")}</b>{zh ? "发卡记录日" : "grant-record days"}</span>
      </div>
    </div>
    <div className={styles.controls}>
      <div className={shared.filters} role="group" aria-label={zh ? "记录类别" : "Record category"}>
        {(["all", "events", "announcements"] as const).map((value) => <button key={value} aria-pressed={mode === value}
          onClick={() => { setMode(value); setReviewedOnly(false); changeFilters(); }}>
          {value === "all" ? (zh ? "全部动态" : "All updates") : value === "events" ? (zh ? "发生记录" : "Occurrence records") : (zh ? "预告与线索" : "Announcements & leads")}</button>)}
      </div>
      <div className={shared.filters} role="group" aria-label={zh ? "事件类型" : "Event type"}>
        {(["all", "reset", "grant", "notice"] as const).map((value) => <button key={value} aria-pressed={kind === value}
          onClick={() => { setKind(value); changeFilters(); }}>{value === "all" ? (zh ? "全部" : "All") : kindLabel(value, zh)}</button>)}
      </div>
      {mode === "events" && <label className={styles.reviewFilter}><input type="checkbox" checked={reviewedOnly}
        onChange={(event) => { setReviewedOnly(event.target.checked); changeFilters(); }} />{zh ? "仅已核实发生" : "Reviewed occurrences only"}</label>}
    </div>
    <div className={styles.legend}>
      <span><i data-kind="reset" aria-hidden="true">↺</i>{zh ? "强制重置" : "Full reset"}</span>
      <span><i data-kind="grant" aria-hidden="true">◇</i>{zh ? "重置卡发放" : "Reset-card grant"}</span>
      <span><i data-kind="notice" aria-hidden="true">·</i>{zh ? "预告 / 讨论" : "Announcement / discussion"}</span>
      <span><i data-reviewed aria-hidden="true" />{zh ? "填色：发生时间已核实" : "Filled: reviewed occurrence"}</span>
      <span><i aria-hidden="true" />{zh ? "虚线框：发放公告 / 待核实 / 预告" : "Dashed: rollout announcement / unverified / preview"}</span>
    </div>
    <p className={shared.note}>{mode === "announcements"
      ? (zh ? "按消息发布日期显示，点击日期查看原文。" : "Shown by publication date. Select a day to read the source.")
      : (zh ? "点击日期查看重置记录、发卡公告和时间来源。" : "Select a day for reset reports, card announcements and timestamp sources.")}</p>
    <div className={styles.months}>{months.map((month) => <section className={styles.month} key={month.key} aria-label={monthTitle.format(month.first)}>
      <h3>{monthTitle.format(month.first)}</h3>
      <div className={styles.weekdays} aria-hidden="true">{(zh ? ["一", "二", "三", "四", "五", "六", "日"] : ["M", "T", "W", "T", "F", "S", "S"]).map((day, index) => <span key={index}>{day}</span>)}</div>
      <div className={styles.days}>{month.days.map((day, index) => {
        if (!day) return <span key={`pad-${index}`} />;
        const entries = byDay.get(day.key) ?? [];
        const types = (["reset", "grant", "notice"] as const).filter((target) => entries.some((item) => item.kind === target));
        const label = `${day.key} · ${day.future ? (zh ? "未来日期" : "Future date") : entries.length
          ? types.map((target) => kindLabel(target, zh)).join(" / ") + ` · ${entries.length} ${zh ? "条去重记录" : "deduplicated records"} · `
            + (entries.some((item) => item.mode === "announcements") ? (zh ? "预告与线索 · " : "Announcements & leads · ") : "")
            + (entries.some((item) => item.timeBasis === "grant_announcement") ? (zh ? "已宣布开始发放 · " : "Rollout announced · ") : "")
            + (entries.every((item) => item.reviewed) ? (zh ? "已核实发生" : "Reviewed occurrence") : (zh ? "含待核实或预告" : "Includes unverified or announced records"))
          : (zh ? "未收录记录" : "No records collected")}`;
        return <button type="button" key={day.key} className={styles.day} aria-label={label} title={label}
          aria-pressed={selectedDay === day.key} disabled={day.future} data-today={day.key === today || undefined}
          data-activity={types.length > 1 ? "both" : types[0]} data-reviewed={entries.some((item) => item.reviewed) || undefined}
          onClick={() => { setSelected(day.key); setLimit(6); }}>
          <span>{day.day}</span><span className={styles.markers} aria-hidden="true">{types.map((target) => <i key={target} data-kind={target}
            data-reviewed={entries.some((item) => item.kind === target && item.reviewed) || undefined}>{target === "reset" ? "↺" : target === "grant" ? "◇" : "·"}</i>)}</span>
        </button>;
      })}</div>
    </section>)}</div>
    <div className={styles.dayDetail} aria-label={zh ? "所选日期记录" : "Selected date records"}>
      <div className={shared.sectionToolbar}><h3>{selectedDay}</h3><span className={shared.badge}>{selectedRecords.length} {zh ? "条记录" : "records"}</span></div>
      {selectedRecords.length ? <div className={styles.records}>{selectedRecords.slice(0, limit).map((record) => <CalendarRecord key={record.key} record={record} language={language} onOpenSource={onOpenSource} />)}</div>
        : <p className={shared.note}>{zh ? "当前筛选下未收录该日记录。可选择着色日期，或切换「预告与线索」。" : "No record is collected for this day and filter. Select a highlighted day or switch to announcements and leads."}</p>}
      {selectedRecords.length > limit && <button className={shared.refreshButton} onClick={() => setLimit((value) => value + 6)}>{zh ? `显示更多（还有 ${selectedRecords.length - limit} 条）` : `Show more (${selectedRecords.length - limit} remaining)`}</button>}
    </div>
    {missing.length > 0 && <details className={styles.undated}>
      <summary>{zh ? `${missing.length} 条日期待核对的资料` : `${missing.length} records with unresolved dates`}</summary>
      {missing.slice(0, 6).map((record) => <CalendarRecord key={record.key} record={record} language={language} onOpenSource={onOpenSource} />)}
      {missing.length > 6 && <p className={shared.note}>{zh ? "其余资料可在下方「公开来源与原文资料」查看。" : "Other records remain in Sources and original records below."}</p>}
    </details>}
  </section>;
}
