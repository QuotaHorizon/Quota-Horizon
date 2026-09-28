import type { DesktopHistoryDayActivity, DesktopHistoryEnvelope } from "../../../../capacity-preview/src/status";
import { historySeries } from "./historyChart";

export function localHistoryDate(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function historyActivity(history: DesktopHistoryEnvelope): DesktopHistoryDayActivity[] {
  // Native activity is calculated before hourly reduction. Never calculate
  // usage from the reduced chart when the raw-observation summary is present.
  if (history.overview) return history.overview.dailyActivity;
  const days = new Map<string, DesktopHistoryDayActivity>();
  const points = historySeries(history.points).flat();
  for (let i = 0; i < points.length; i += 1) {
    const point = points[i];
    const date = localHistoryDate(new Date(point.capturedAt));
    const day = days.get(date) ?? { date, sampleCount: 0, comparableIntervals: 0, consumedPercent: 0 };
    day.sampleCount += 1;
    const previous = points[i - 1];
    const gap = previous ? Date.parse(point.capturedAt) - Date.parse(previous.capturedAt) : 0;
    if (previous && gap > 0 && gap <= 3_600_000
      && date === localHistoryDate(new Date(previous.capturedAt))
      && point.resetsAt != null && previous.resetsAt != null
      && Date.parse(point.resetsAt) === Date.parse(previous.resetsAt)
      && point.windowMinutes === previous.windowMinutes) {
      day.comparableIntervals += 1;
      day.consumedPercent = Math.round((day.consumedPercent + Math.max(0, previous.remainingPercent - point.remainingPercent)) * 100) / 100;
    }
    days.set(date, day);
  }
  return [...days.values()];
}

export interface HistoryCalendarDay {
  date: string;
  day: number;
  activity?: DesktopHistoryDayActivity;
  level: number;
}

export function historyCalendar(days: DesktopHistoryDayActivity[], latest: string): (HistoryCalendarDay | null)[] {
  const end = new Date(latest);
  if (!Number.isFinite(end.getTime())) return [];
  // Calendar arithmetic, not 24-hour subtraction, keeps local dates correct
  // across DST. Noon avoids ambiguous or nonexistent midnight instants.
  const cursor = new Date(end.getFullYear(), end.getMonth(), end.getDate() - 29, 12);
  const cells: (HistoryCalendarDay | null)[] = Array.from({ length: (cursor.getDay() + 6) % 7 }, () => null);
  const byDate = new Map(days.map((day) => [day.date, day]));
  for (let i = 0; i < 30; i += 1) {
    const date = localHistoryDate(cursor);
    const activity = byDate.get(date);
    const value = activity?.comparableIntervals ? activity.consumedPercent : null;
    const level = value == null ? -1 : value === 0 ? 0 : value <= 2 ? 1 : value <= 5 ? 2 : value <= 10 ? 3 : 4;
    cells.push({ date, day: cursor.getDate(), activity, level });
    cursor.setDate(cursor.getDate() + 1);
  }
  return cells;
}

export function percentagePoints(value: number, language: "zh" | "en") {
  return new Intl.NumberFormat(language === "zh" ? "zh-CN" : "en-US", { maximumFractionDigits: 1 }).format(value);
}
