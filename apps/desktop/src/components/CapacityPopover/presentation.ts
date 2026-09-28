import type {
  DesktopQuotaWindow,
  DesktopStatusEnvelope,
} from "../../../../capacity-preview/src/status";
import {
  selectPrimaryQuotaWindow,
  selectShortestQuotaWindow,
} from "../../pages/CapacityPage/presentation";
import { planHasNoShortQuotaWindow } from "../../utils/accountUsageWindows";
import { quotaPercentLabel as percentValueLabel } from "../../utils/quotaPercent";

export type CapacityTone = "healthy" | "attention" | "critical" | "unknown";
export type PaceTone = "on-pace" | "guard" | "over" | "unknown";

export function idleCapacityEnvelope(): DesktopStatusEnvelope {
  return {
    schemaVersion: "1.0",
    sequence: 0,
    lifecycle: "idle",
    status: null,
    candidates: [],
    selectedExecutableId: null,
    issue: null,
    persistenceEnabled: false,
  };
}

export function acceptNewerEnvelope(
  current: DesktopStatusEnvelope,
  incoming: DesktopStatusEnvelope,
) {
  return incoming.sequence >= current.sequence ? incoming : current;
}

export function popoverWindows(
  windows: DesktopQuotaWindow[],
  planType?: string | null,
) {
  const canonical = windows.filter((window) => (
    window.limitId === "codex" || window.limitId.startsWith("codex:")
  ));
  const visible = canonical.length ? canonical : windows;
  const primary = selectPrimaryQuotaWindow(visible);
  const shortest = planHasNoShortQuotaWindow(planType)
    ? null
    : selectShortestQuotaWindow(visible);
  return {
    primary,
    secondary: shortest?.limitId === primary?.limitId ? null : shortest,
  };
}

export function capacityTone(remaining: number | null): CapacityTone {
  if (remaining === null || !Number.isFinite(remaining)) return "unknown";
  if (remaining <= 10) return "critical";
  if (remaining <= 30) return "attention";
  return "healthy";
}

export function percentLabel(window: DesktopQuotaWindow | null) {
  return percentValueLabel(window?.remainingPercent);
}

export { percentValueLabel };
export { planPercentLabel } from "../../utils/quotaPercent";

export function durationLabel(window: DesktopQuotaWindow | null, language: "en" | "zh") {
  const minutes = window?.windowMinutes;
  if (minutes === 10_080) return language === "zh" ? "周额度" : "Weekly";
  if (minutes === 300) return language === "zh" ? "5 小时窗口" : "5-hour window";
  if (typeof minutes === "number" && minutes % 10_080 === 0) {
    return language === "zh" ? `${minutes / 10_080} 周窗口` : `${minutes / 10_080}-week window`;
  }
  if (typeof minutes === "number" && minutes % 60 === 0) {
    return language === "zh" ? `${minutes / 60} 小时窗口` : `${minutes / 60}-hour window`;
  }
  return window?.label || (language === "zh" ? "额度窗口" : "Quota window");
}

export function resetCountdown(
  resetsAt: string | null | undefined,
  now: number,
  language: "en" | "zh",
) {
  if (!resetsAt) return language === "zh" ? "未提供重置时间" : "Reset time unavailable";
  const milliseconds = Date.parse(resetsAt) - now;
  if (!Number.isFinite(milliseconds)) return language === "zh" ? "重置时间无效" : "Invalid reset time";
  if (milliseconds <= 0) return language === "zh" ? "等待官方刷新" : "Awaiting official refresh";
  const totalMinutes = Math.ceil(milliseconds / 60_000);
  const days = Math.floor(totalMinutes / 1_440);
  const hours = Math.floor((totalMinutes % 1_440) / 60);
  const minutes = totalMinutes % 60;
  if (language === "zh") {
    if (days > 0) return `${days} 天 ${hours} 小时后`;
    if (hours > 0) return `${hours} 小时 ${minutes} 分后`;
    return `${minutes} 分钟后`;
  }
  if (days > 0) return `in ${days}d ${hours}h`;
  if (hours > 0) return `in ${hours}h ${minutes}m`;
  return `in ${minutes}m`;
}

export function localResetTime(resetsAt: string | null | undefined, language: "en" | "zh") {
  if (!resetsAt || !Number.isFinite(Date.parse(resetsAt))) return "—";
  return new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(resetsAt));
}

export function updatedLabel(
  capturedAt: string | null | undefined,
  now: number,
  language: "en" | "zh",
) {
  if (!capturedAt) return language === "zh" ? "尚未刷新" : "Not refreshed yet";
  const elapsed = Math.max(0, now - Date.parse(capturedAt));
  if (!Number.isFinite(elapsed)) return language === "zh" ? "更新时间未知" : "Update time unknown";
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return language === "zh" ? "刚刚更新" : "Updated just now";
  if (minutes < 60) return language === "zh" ? `${minutes} 分钟前更新` : `Updated ${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  return language === "zh" ? `${hours} 小时前更新` : `Updated ${hours}h ago`;
}

export function accountUsageFreshnessLabel(
  fetchedAt: string | null | undefined,
  hasError: boolean,
  now: number,
  language: "en" | "zh",
) {
  if (hasError) return language === "zh" ? "刷新失败 · 已保留旧数据" : "Refresh failed · cached data";
  return updatedLabel(fetchedAt, now, language);
}

export function accountResetCountdown(
  resetsAt: number | null | undefined,
  now: number,
  language: "en" | "zh",
) {
  if (!resetsAt || !Number.isFinite(resetsAt)) {
    return language === "zh" ? "重置时间未知" : "Reset unknown";
  }
  const value = new Date(resetsAt * 1_000);
  if (!Number.isFinite(value.getTime())) {
    return language === "zh" ? "重置时间无效" : "Invalid reset time";
  }
  return resetCountdown(value.toISOString(), now, language);
}

export function accountWeeklyResetDate(resetsAt: number | null | undefined, language: "en" | "zh") {
  if (resetsAt == null || !Number.isFinite(resetsAt)) return "—";
  const date = new Date(resetsAt * 1_000);
  if (!Number.isFinite(date.getTime())) return "—";
  return new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-GB", {
    year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
    hourCycle: "h23",
  }).format(date);
}

export function paceTone(state: string | null | undefined): PaceTone {
  if (state === "on_pace") return "on-pace";
  if (state === "within_guard") return "guard";
  if (state === "over_guard") return "over";
  return "unknown";
}

export function minuteToTimeValue(minute: number) {
  const normalized = ((Math.trunc(minute) % 1_440) + 1_440) % 1_440;
  return `${String(Math.floor(normalized / 60)).padStart(2, "0")}:${String(normalized % 60).padStart(2, "0")}`;
}

export function timeValueToMinute(value: string): number | null {
  const match = /^(\d{2}):(\d{2})$/.exec(value);
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  if (hour > 23 || minute > 59) return null;
  return hour * 60 + minute;
}

export function minuteSpanLabel(minutes: number, language: "en" | "zh") {
  const safe = Math.max(0, Math.min(1_440, Math.round(minutes)));
  const hours = Math.floor(safe / 60);
  const remainder = safe % 60;
  if (language === "zh") return remainder ? `${hours} 小时 ${remainder} 分` : `${hours} 小时`;
  return remainder ? `${hours}h ${remainder}m` : `${hours}h`;
}

export function targetTimeLabel(
  targetAt: string | null | undefined,
  language: "en" | "zh",
) {
  if (!targetAt || !Number.isFinite(Date.parse(targetAt))) return "—";
  return new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en", {
    weekday: "short",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(targetAt));
}
