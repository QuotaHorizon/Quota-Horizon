import { BellRing } from "lucide-react";
import type { Language } from "../../i18n";
import { calendarEventTime, localCalendarDate } from "../../pages/ResetIntelligencePage/resetCalendarModel";
import type { UpcomingResetNotice } from "../../pages/ResetIntelligencePage/upcomingResetModel";
import type { PublicInsights, PublicResetTimeline } from "../../pages/ResetIntelligencePage/types";
import { forecastPresentation } from "../../pages/ResetIntelligencePage/resetBriefingModel";
import styles from "./index.module.less";

export function ResetNoticeChip({ notice, insights, timeline, sourceFailed = false, now, language, failed, onOpen, newUpdates = 0 }: {
  notice?: UpcomingResetNotice; insights?: PublicInsights; timeline?: PublicResetTimeline | null; sourceFailed?: boolean; now: number; language: Language; failed?: boolean; onOpen: () => void;
  newUpdates?: number;
}) {
  const zh = language === "zh";
  const forecast = forecastPresentation(insights, now, sourceFailed, timeline);
  const probability = forecast.value ? `${zh ? "第三方 24h" : "Source 24h"} ${forecast.value.probability24h}%${forecast.stale ? (zh ? " · 缓存" : " · Cached") : ""}` : null;
  const label = failed ? (zh ? "未能打开 · 重试" : "Retry opening") : !notice ? forecast.signalWithdrawn ? (zh ? "线索有更正" : "Lead corrected") : (zh ? "重置情报" : "Reset briefing") : notice.state === "withdrawn"
    ? (zh ? "预告有更正" : "Notice corrected") : notice.state === "reported"
      ? (zh ? "重置新动态" : "Reset update") : notice.state === "announced_delivery"
        ? (zh ? "发卡新动态" : "Reset-card update") : notice.state === "elapsed"
        ? (zh ? "预告待确认" : "Awaiting confirmation") : notice.kind === "grant"
          ? (zh ? "发卡预告" : "Grant announced") : notice.kind === "unknown" ? (zh ? "重置相关预告" : "Reset-related notice") : (zh ? "重置预告" : "Reset announced");
  const target = notice?.state === "upcoming" && notice.timing?.deadline ? notice.timing.end : null;
  const displayState = notice?.state ?? (forecast.signalWithdrawn ? "withdrawn" : undefined);
  const today = target != null && localCalendarDate(new Date(target)) === localCalendarDate(new Date(now));
  const time = target == null ? null : new Intl.DateTimeFormat(zh ? "zh-CN" : "en-US", {
    ...(today ? {} : { month: "2-digit", day: "2-digit" }), hour: "2-digit", minute: "2-digit", hourCycle: "h23",
  }).format(target);
  const detail = `${label}${newUpdates > 0 ? (zh ? " · 有新动态" : " · New update") : ""}${probability ? ` · Codex Reset · ${probability}` : ""}${target == null ? "" : ` · ${zh ? "追踪站预计期限：" : "Tracker’s estimated deadline: "}${calendarEventTime(target, language)}`}${notice?.stale ? ` · ${zh ? "上次资料" : "Saved information"}` : ""} · ${zh ? "查看详情" : "View details"}`;
  return <button type="button" className={styles.resetChip} data-state={displayState} title={detail} aria-label={detail} onClick={onOpen}>
    <BellRing size={12} aria-hidden="true" /><span>{label}{probability ? <small>{probability}</small> : time ? <small>{zh ? `预计 ${time} 前` : `Est. by ${time}`}</small>
      : notice?.stale ? <small>{zh ? "含缓存" : "Cached"}</small> : null}</span>
    {newUpdates > 0 && <b className={styles.resetUnreadBadge} aria-hidden="true" />}
  </button>;
}
