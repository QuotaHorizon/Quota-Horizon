import type { EChartsCoreOption } from "echarts/core";
import type { DesktopHistoryPoint } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { chronologicalHistoryPoints } from "./presentation";
import { quotaPercentLabel } from "../../utils/quotaPercent";

export function historySeries(points: DesktopHistoryPoint[]) {
  const valid = chronologicalHistoryPoints(points).filter((point) => (
    Number.isFinite(Date.parse(point.capturedAt)) && Number.isFinite(point.remainingPercent)
    && point.remainingPercent >= 0 && point.remainingPercent <= 100
  ));
  // Join observed increases as well as decreases. The line describes recorded
  // levels, not a continuous measurement or proof of an official reset.
  return valid.length ? [valid] : [];
}

export type HistoryRange = "24h" | "7d" | "30d";
const HOUR = 3_600_000;
const RANGE_HOURS: Record<HistoryRange, number> = { "24h": 24, "7d": 168, "30d": 720 };

export function historyRangePoints(points: DesktopHistoryPoint[], range: HistoryRange) {
  const all = historySeries(points).flat();
  const end = Date.parse(all.at(-1)?.capturedAt ?? "");
  return all.filter((point) => Date.parse(point.capturedAt) >= end - RANGE_HOURS[range] * HOUR);
}

type HistoryCoordinate = [number, number];

/** Two bounded series, sharing only transition endpoints. A null prevents a
 * style from jumping over edges owned by the other style. Statistics still use
 * the original observations, never these duplicated drawing endpoints. */
export function historyLineStyles(points: DesktopHistoryPoint[]) {
  const coordinates: HistoryCoordinate[] = historySeries(points).flat()
    .map((point) => [Date.parse(point.capturedAt), point.remainingPercent]);
  const solid: (HistoryCoordinate | null)[] = [];
  const dashed: (HistoryCoordinate | null)[] = [];
  let previousStyle: "solid" | "dashed" | undefined;
  for (let index = 1; index < coordinates.length; index++) {
    const style = coordinates[index][1] > coordinates[index - 1][1] ? "dashed" : "solid";
    const data = style === "dashed" ? dashed : solid;
    if (previousStyle !== style) {
      if (data.length) data.push(null);
      data.push(coordinates[index - 1]);
    }
    data.push(coordinates[index]);
    previousStyle = style;
  }
  if (coordinates.length === 1) solid.push(coordinates[0]);
  return [{ style: "solid" as const, data: solid }, { style: "dashed" as const, data: dashed }]
    .filter((series) => series.data.length > 0);
}

export function historyChartOption(points: DesktopHistoryPoint[], language: Language, dark = false): EChartsCoreOption {
  const locale = language === "zh" ? "zh-CN" : "en-US";
  const shortTime = new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
  const dateTime = new Intl.DateTimeFormat(locale, { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
  const dateOnly = new Intl.DateTimeFormat(locale, { month: "short", day: "numeric" });
  const segments = historySeries(points);
  const all = segments.flat();
  const start = Date.parse(all[0]?.capturedAt ?? "");
  const end = Date.parse(all.at(-1)?.capturedAt ?? "");
  const spansDays = Number.isFinite(start) && Number.isFinite(end)
    && new Date(start).toDateString() !== new Date(end).toDateString();
  const dailyTicks = end - start >= 48 * HOUR;
  const axisStart = new Date(start);
  axisStart.setMinutes(0, 0, 0);
  const axisEnd = Math.max(axisStart.getTime() + HOUR, Math.ceil(end / HOUR) * HOUR);
  const lowest = all.length ? Math.min(...all.map((point) => point.remainingPercent)) : 0;
  const highest = all.length ? Math.max(...all.map((point) => point.remainingPercent)) : 100;
  // Expose small real changes without inventing fractional observations. The
  // labelled axis always retains at least a ten-percentage-point span.
  const yMin = Math.max(0, Math.min(90, Math.floor((lowest - 2) / 5) * 5));
  const yMax = Math.min(100, Math.max(yMin + 10, Math.ceil((highest + 2) / 5) * 5));
  const text = dark ? "#a6b5aa" : "#718078";
  return {
    // Background sampling is an in-place update, not a replay of the line.
    animation: false,
    aria: { enabled: true, label: { description: language === "zh"
      ? `额度历史，${all.length} 个样本。最新剩余 ${quotaPercentLabel(all.at(-1)?.remainingPercent)}。`
      : `Capacity history, ${all.length} samples. Latest remaining ${quotaPercentLabel(all.at(-1)?.remainingPercent)}.` } },
    color: [dark ? "#80c996" : "#4b8760"],
    grid: { left: 8, right: 16, top: 14, bottom: 6, containLabel: true },
    tooltip: {
      trigger: "axis", confine: true, renderMode: "richText",
      valueFormatter: (value: unknown) => quotaPercentLabel(typeof value === "number" ? value : null),
      // Shared style endpoints are one capture, not two observations in the tooltip.
      formatter: (params: unknown) => {
        const captures = new Map<string, HistoryCoordinate>();
        for (const item of Array.isArray(params) ? params : [params]) {
          const value: unknown = item?.value;
          if (Array.isArray(value) && value.length === 2 && value.every((part) => typeof part === "number" && Number.isFinite(part))) {
            captures.set(`${value[0]}:${value[1]}`, value as HistoryCoordinate);
          }
        }
        return [...captures.values()].map(([time, value]) => `${dateTime.format(time)}\n${language === "zh" ? "剩余额度" : "Remaining quota"}  ${quotaPercentLabel(value)}`).join("\n");
      },
      axisPointer: { type: "line", label: { formatter: (params: { value: string | number | Date }) => dateTime.format(new Date(params.value)) } },
    },
    xAxis: {
      type: "time", boundaryGap: [0, 0], splitNumber: 5,
      minInterval: dailyTicks ? 24 * HOUR : HOUR,
      ...(Number.isFinite(start) ? { min: axisStart.getTime(), max: axisEnd } : {}),
      axisTick: { show: false }, axisLine: { show: false },
      axisLabel: { color: text, hideOverlap: true, formatter: (value: number) => (dailyTicks ? dateOnly : spansDays ? dateTime : shortTime).format(value) },
    },
    yAxis: {
      type: "value", min: yMin, max: yMax, interval: yMax - yMin > 50 ? 25 : yMax - yMin > 25 ? 10 : 5,
      axisLabel: { color: text, formatter: "{value}%" },
      splitLine: { lineStyle: { color: dark ? "#344238" : "#e8ede8" } },
    },
    series: historyLineStyles(all).map(({ style, data }) => ({
      id: `quota-${style}`, name: style === "dashed"
        ? (language === "zh" ? "额度回升" : "Quota rise") : (language === "zh" ? "下降或持平" : "Decrease or unchanged"),
      type: "line", smooth: false, connectNulls: false,
      showSymbol: all.length === 1, symbol: "circle", symbolSize: 6,
      // Dense samples must not cut holes in the line with white point borders.
      itemStyle: { borderWidth: 0, color: dark ? "#80c996" : "#4b8760" },
      lineStyle: { width: 2.5, type: style, color: dark ? "#80c996" : "#4b8760" },
      emphasis: { scale: false },
      data,
    })),
  };
}
