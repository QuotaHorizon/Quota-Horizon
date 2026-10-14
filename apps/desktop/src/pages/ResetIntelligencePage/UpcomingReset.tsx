import { useEffect, useMemo, useState } from "react";
import { BellRing, Clock3, ExternalLink } from "lucide-react";
import type { Language } from "../../i18n";
import { EvidenceLink } from "./ArchiveView";
import { calendarEventTime } from "./resetCalendarModel";
import { upcomingResetNotices } from "./upcomingResetModel";
import type { PublicResetTimeline } from "./types";
import shared from "./index.module.less";
import styles from "./upcomingReset.module.less";

export function UpcomingResetView({ timeline, now, language, failed = false, onOpenSource }: {
  timeline: PublicResetTimeline | null; now: number; language: Language; failed?: boolean; onOpenSource?: (url: string) => void;
}) {
  const notices = useMemo(() => upcomingResetNotices(timeline, now), [timeline, now]);
  const notice = notices[0];
  if (!notice) return null;
  const zh = language === "zh";
  const minutes = notice.timing ? Math.max(0, Math.ceil((notice.timing.end - now) / 60_000)) : null;
  const remaining = minutes == null ? null : minutes >= 60
    ? (zh ? `约 ${Math.floor(minutes / 60)} 小时 ${minutes % 60} 分钟` : `about ${Math.floor(minutes / 60)}h ${minutes % 60}m`)
    : (zh ? `约 ${minutes} 分钟` : `about ${minutes} minutes`);
  const heading = notice.state === "withdrawn" ? (zh ? "近期预告有更正或撤回" : "Recent announcement corrected or withdrawn")
    : notice.state === "reported" ? (zh ? "追踪站已记录后续确认" : "A tracker has recorded a follow-up confirmation")
      : notice.state === "announced_delivery" ? (zh ? "Tibo 宣布正在发放重置卡" : "Tibo says reset cards are being issued")
      : notice.kind === "grant" ? (zh ? "Tibo 明确预告重置卡发放" : "Tibo announced a reset-card grant")
        : notice.kind === "unknown" ? (zh ? "Tibo 提及重置 · 类型尚未明确" : "Tibo mentioned a reset · type not specified")
        : (zh ? "Tibo 明确预告额度重置" : "Tibo explicitly announced a quota reset");
  return <section className={styles.notice} aria-labelledby="upcoming-reset-heading" data-state={notice.state}>
    <div className={styles.heading}><BellRing size={19} aria-hidden="true" /><h2 id="upcoming-reset-heading">{heading}</h2>
      <span className={shared.badge}>{zh ? "公开发言" : "Public statement"}</span></div>
    <blockquote>{notice.excerpt}</blockquote>
    {notice.state === "upcoming" && notice.timing && <div className={styles.timing}>
      <Clock3 size={17} aria-hidden="true" /><div>
        <span>{notice.timing.explicit ? (zh ? "官方承诺时间" : "Officially promised time")
          : notice.timing.deadline ? (zh ? "追踪站预计最晚时间" : "Tracker’s estimated deadline")
          : (zh ? "追踪站预计日期" : "Tracker’s estimated date range")}</span>
        <strong>{notice.timing.explicit || notice.timing.deadline ? calendarEventTime(notice.timing.end, language)
          : `${calendarEventTime(notice.timing.start, language)} → ${calendarEventTime(notice.timing.end, language)}`}</strong>
        {(notice.timing.explicit || notice.timing.deadline) && <p>{zh ? `${notice.timing.explicit ? "距离承诺时间" : "距离预计期限"} ${remaining}` : `${remaining} until the ${notice.timing.explicit ? "promised time" : "estimated deadline"}`}</p>}
      </div>
    </div>}
    {notice.state === "elapsed" && <p className={styles.stateMessage} role="status">{zh ? "预计时间已过，等待确认。" : "The estimated time has passed. Awaiting confirmation."}</p>}
    {notice.state === "reported" && <p className={styles.stateMessage}>{zh ? "追踪站报告已发生。" : "Reported as occurred by the tracker."}</p>}
    {notice.state === "announced_delivery" && <p className={styles.stateMessage}>{zh ? "公告：已开始发放重置卡。可用卡数见「当前账号」。" : "Announcement: reset-card rollout started. See Your account for available cards."}</p>}
    {notice.state === "withdrawn" && <p className={styles.stateMessage}>{zh ? "预告已失效，请查看最新更正。" : "This notice is no longer active. See the latest correction."}</p>}
    {notice.cohort && <p className={styles.stateMessage}>{zh ? `适用范围：${notice.cohort}` : `Covered population: ${notice.cohort}`}</p>}
    {!notice.timing && notice.state === "upcoming" && <p className={styles.stateMessage}>{notice.conflictingTiming
      ? (zh ? "不同来源的时间解释有冲突，暂不显示倒计时。" : "Source timing interpretations conflict; no countdown is shown.")
      : (zh ? "具体时间待确认。" : "Exact timing is not confirmed.")}</p>}
    {(notice.stale || failed) && <p className={styles.warning} role="status">{zh ? "来源待更新，显示上次资料。" : "Source update due. Showing saved information."}</p>}
    <div className={shared.sourceRow}>
      <span>{zh ? "发言发布：" : "Published: "}{calendarEventTime(notice.publishedAt, language)}</span>
      <span>{zh ? `${notice.trackerCount} 个追踪网站收录 · 同一条原始发言` : `${notice.trackerCount} tracker sites · one original statement`}</span>
      <EvidenceLink url={notice.url} label={zh ? "Tibo 原帖" : "Tibo’s original post"} onOpen={onOpenSource} />
    </div>
    <details className={styles.details}>
      <summary><ExternalLink size={12} aria-hidden="true" />{zh ? "时间依据与对照来源" : "Timing basis and source comparison"}</summary>
      {notice.timing && <p>{notice.timing.explicit
        ? (zh ? `原帖给出明确承诺时间；以 ${notice.timing.timeZone} 解释并换算为电脑当前时区。` : `The original gives a dated commitment, interpreted in ${notice.timing.timeZone} and converted to your computer’s time zone.`)
        : (zh ? `追踪站按 ${notice.timing.timeZone} 解释原帖日期，已换算为电脑当前时区。` : `The tracker interprets the post’s date in ${notice.timing.timeZone}, converted here to your computer’s time zone.`)}</p>}
      <div className={shared.sourceRow}>{notice.collectedVia.map((url, index) => <EvidenceLink key={url} url={url} label={zh ? `采集对照 ${index + 1}` : `Collection source ${index + 1}`} onOpen={onOpenSource} />)}</div>
    </details>
  </section>;
}

/** Minute updates change only this notice, not the page, history or quota. */
export function UpcomingReset({ timeline, language, failed, onOpenSource }: {
  timeline: PublicResetTimeline | null; language: Language; failed?: boolean; onOpenSource?: (url: string) => void;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const update = () => { if (document.visibilityState === "visible") setNow(Date.now()); };
    const timer = window.setInterval(update, 60_000);
    window.addEventListener("focus", update); document.addEventListener("visibilitychange", update);
    return () => { window.clearInterval(timer); window.removeEventListener("focus", update); document.removeEventListener("visibilitychange", update); };
  }, []);
  return <UpcomingResetView timeline={timeline} now={now} language={language} failed={failed} onOpenSource={onOpenSource} />;
}
