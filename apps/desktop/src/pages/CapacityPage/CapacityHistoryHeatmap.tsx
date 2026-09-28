import { useMemo } from "react";
import type { DesktopHistoryEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { historyActivity, historyCalendar, percentagePoints } from "./historyActivity";
import styles from "./index.module.less";

export function CapacityHistoryHeatmap({ history, latest, language }: {
  history: DesktopHistoryEnvelope; latest: string; language: Language;
}) {
  const cells = useMemo(() => historyCalendar(historyActivity(history), latest), [history, latest]);
  const zh = language === "zh";
  const weekdays = zh ? ["一", "二", "三", "四", "五", "六", "日"] : ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
  const recorded = cells.flatMap((cell) => cell?.activity ? [cell.activity] : []);
  const total = recorded.reduce((sum, day) => sum + day.consumedPercent, 0);
  const hasComparable = recorded.some((day) => day.comparableIntervals > 0);
  const dateLabel = new Intl.DateTimeFormat(zh ? "zh-CN" : "en-US", { month: "short", day: "numeric" });
  const firstDate = cells.find((cell) => cell)?.date;
  return <div className={styles.historyHeatmap}>
    <div className={styles.heatmapCopy}>
      <h4>{zh ? "每日额度消耗" : "Daily quota activity"}</h4>
      <span>{zh ? "最近 30 天 · " : "Last 30 days · "}{firstDate && dateLabel.format(new Date(`${firstDate}T12:00:00`))} — {dateLabel.format(new Date(latest))}</span>
      <strong>{hasComparable ? percentagePoints(total, language) : "—"} <small>{zh ? "个百分点" : "percentage points"}</small></strong>
      <span>{zh ? `${recorded.length} 天有本地记录 · 已观测消耗合计` : `${recorded.length} days recorded · Total observed decrease`}</span>
      <details><summary>{zh ? "如何统计" : "How it’s counted"}</summary><p>{zh
        ? "累计同日、同额度窗口的已记录下降。回升单独记录；跨日、超过一小时的采样间隔和缺失时段排除。"
        : "Adds recorded decreases within each day and quota window. Rises are tracked separately. Cross-day intervals, gaps over an hour and missing periods are excluded."}</p></details>
    </div>
    <div className={styles.heatmapCalendar}>
      <div className={styles.heatmapGrid} role="list" aria-label={zh ? "30 天额度消耗热力图" : "30-day quota activity heatmap"}>
        {weekdays.map((day) => <span className={styles.heatmapWeekday} key={day} aria-hidden="true">{day}</span>)}
        {cells.map((cell, index) => {
          if (!cell) return <span key={`blank-${index}`} aria-hidden="true" />;
          const activity = cell.activity;
          const value = activity?.comparableIntervals ? `${percentagePoints(activity.consumedPercent, language)}${zh ? " 个百分点" : " percentage points"}` : null;
          const label = `${cell.date} · ${value ?? (activity ? (zh ? "记录不足，消耗未知" : "Insufficient observations; consumption unknown") : (zh ? "无记录" : "No observations"))}${activity ? ` · ${activity.sampleCount}${zh ? " 个样本" : " samples"}` : ""}`;
          return <div key={cell.date} role="listitem" aria-label={label} title={label}
            className={styles.heatmapCell} data-level={cell.level}>
            <span>{cell.day === 1 || index === cells.findIndex((item) => item) ? `${Number(cell.date.slice(5, 7))}/${cell.day}` : cell.day}</span>
            <small>{activity?.comparableIntervals ? percentagePoints(activity.consumedPercent, language) : "—"}</small>
          </div>;
        })}
      </div>
      <div className={styles.heatmapLegend} aria-label={zh ? "颜色表示消耗，灰纹表示未知" : "Color shows consumption; gray stripes mean unknown"}>
        <i data-level={-1} /><span>{zh ? "未知" : "Unknown"}</span><span>0</span>
        {[0, 1, 2, 3, 4].map((level) => <i data-level={level} key={level} />)}
        <span>{zh ? ">10 个百分点" : ">10 pp"}</span>
      </div>
    </div>
  </div>;
}
