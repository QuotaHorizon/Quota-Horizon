import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { DesktopHistoryEnvelope, DesktopHistoryPoint } from "../../../../capacity-preview/src/status";
import { CapacityHistory } from "./CapacityHistory";
import { buildHistoryCoordinates, chronologicalHistoryPoints, historyEmptyMessage } from "./presentation";
import { historyChartOption, historyLineStyles, historyRangePoints, historySeries } from "./historyChart";
import { historyActivity, historyCalendar, localHistoryDate } from "./historyActivity";

const point = (capturedAt: string, remainingPercent = 28.4, resetsAt = "2026-09-10T00:00:00Z"): DesktopHistoryPoint => ({
  snapshotId: capturedAt, capturedAt, limitId: "codex:primary", label: null, windowMinutes: 10080,
  usedPercent: 100 - remainingPercent, remainingPercent, resetsAt, availability: "complete", compatibility: "tested",
});
const history = (points: DesktopHistoryPoint[]): DesktopHistoryEnvelope => ({
  schemaVersion: "1.0", status: "available", reasonCode: "history_available", points,
});

describe("capacity history", () => {
  it("uses real quota precision in the summary, tooltip and accessible chart description", () => {
    const points = [point("2026-09-07T00:00:00Z", 98)];
    const markup = renderToStaticMarkup(<CapacityHistory history={history(points)} language="zh" />);
    expect(markup).toContain("98%");
    expect(markup).not.toContain("98.0%");
    const option = historyChartOption(points, "zh");
    expect(option.aria).toMatchObject({ label: { description: "额度历史，1 个样本。最新剩余 98%。" } });
    const formatter = (option.tooltip as { valueFormatter: (value: unknown) => string }).valueFormatter;
    expect(formatter(98)).toBe("98%");
    expect(formatter(28.4)).toBe("28.4%");
    expect(formatter(null)).toBe("—");
  });
  it("shows the first sample as a visible dot and exact value", () => {
    const html = renderToStaticMarkup(<CapacityHistory history={history([point("2026-09-07T00:00:00Z")])} language="zh" />);
    expect(html).toContain('role="img"');
    const option = historyChartOption([point("2026-09-07T00:00:00Z")], "zh");
    expect(option.series).toMatchObject([{ showSymbol: true, symbolSize: 6 }]);
    expect(html).toContain("1 个本地样本");
    expect(html).toContain("28.4%");
    expect(html).not.toContain("多个");
  });

  it("draws dense captures as one straight polyline without outlined point cutouts", () => {
    const points = Array.from({ length: 147 }, (_, i) => point(new Date(Date.UTC(2026, 8, 7, 0, i)).toISOString(), 80 - i / 10));
    const option = historyChartOption(points, "zh");
    expect(option.animation).toBe(false);
    expect(option.xAxis).toMatchObject({ type: "time" });
    expect(option.yAxis).toMatchObject({ min: 60, max: 85, axisLabel: { formatter: "{value}%" } });
    expect(option.series).toHaveLength(1);
    expect(option.series).toMatchObject([{ showSymbol: false, smooth: false, itemStyle: { borderWidth: 0 } }]);
  });

  it("joins observed increases across window boundaries without changing reset metadata", () => {
    const points = [point("2026-09-07T00:00:00Z", 12), point("2026-09-07T00:01:00Z", 99, "2026-09-17T00:00:00Z")];
    expect(historySeries(points)).toEqual([points]);
    expect(historyChartOption(points, "en").series).toMatchObject([{ lineStyle: { type: "dashed" }, data: [[Date.parse(points[0].capturedAt), 12], [Date.parse(points[1].capturedAt), 99]] }]);
    expect(historyChartOption(points, "en").series).toHaveLength(1);
    expect(historySeries([points[0], { ...points[0], resetsAt: "2026-09-10T08:00:00+08:00" }])).toHaveLength(1);
  });

  it("uses dashed edges only for rises, including small corrections, and never bridges separate runs", () => {
    const values = [80, 78, 99, 99.1, 98, 98, 99, 97];
    const points = values.map((value, index) => point(`2026-09-07T00:0${index}:00Z`, value));
    const series = historyLineStyles(points);
    expect(series).toHaveLength(2);
    expect(series[0].data.map((value) => value?.[1] ?? null)).toEqual([80, 78, null, 99.1, 98, 98, null, 99, 97]);
    expect(series[1].data.map((value) => value?.[1] ?? null)).toEqual([78, 99, 99.1, null, 98, 99]);
    expect(historyChartOption(points, "zh").series).toMatchObject([
      { connectNulls: false, showSymbol: false, lineStyle: { type: "solid" } },
      { connectNulls: false, showSymbol: false, lineStyle: { type: "dashed" } },
    ]);
    expect(historySeries(points).flat()).toHaveLength(8);
    expect(renderToStaticMarkup(<CapacityHistory history={history(points)} language="zh" />)).toContain("虚线：额度回升");
  });

  it("keeps every adjacent edge exactly once and deduplicates shared-endpoint tooltips", () => {
    const points = Array.from({ length: 1000 }, (_, index) => point(new Date(Date.UTC(2026, 8, 7, 0, index)).toISOString(), index % 2 ? 40 : 80));
    const series = historyLineStyles(points);
    expect(series).toHaveLength(2);
    let edgeCount = 0;
    for (const { style, data } of series) {
      for (let index = 1; index < data.length; index++) {
        const before = data[index - 1]; const after = data[index];
        if (before && after) {
          edgeCount++;
          expect(after[0] - before[0]).toBe(60_000);
          expect(after[1] > before[1]).toBe(style === "dashed");
        }
      }
    }
    expect(edgeCount).toBe(points.length - 1);
    const option = historyChartOption(points, "zh");
    const formatter = (option.tooltip as { formatter: (value: unknown) => string }).formatter;
    const value = [Date.parse(points[1].capturedAt), 40];
    expect(formatter([{ value }, { value }, { value: null }]).match(/40%/g)).toHaveLength(1);
    expect(formatter([{ value: [NaN, 20] }])).toBe("");
  });

  it("renders an actual dashed rise path with the existing ECharts renderer", async () => {
    const { init, use } = await import("echarts/core");
    const { SVGRenderer } = await import("echarts/renderers");
    use([SVGRenderer]);
    const chart = init(null, undefined, { renderer: "svg", ssr: true, width: 600, height: 240 });
    try {
      const points = [80, 30, 99, 80].map((value, index) => point(`2026-09-07T00:0${index}:00Z`, value));
      chart.setOption({ ...historyChartOption(points, "zh"), aria: { enabled: false }, tooltip: { show: false } });
      expect(chart.renderToSVGString()).toMatch(/stroke-dasharray="[\d., ]+"/);
    } finally { chart.dispose(); }
  });

  it("never uses sub-hour ticks, including a single sample, and uses daily ticks for long history", () => {
    const short = historyChartOption([point("2026-09-07T00:23:00Z")], "zh");
    expect(short.xAxis).toMatchObject({ minInterval: 3_600_000 });
    const axis = short.xAxis as { min: number; max: number };
    expect(axis.max - axis.min).toBeGreaterThanOrEqual(3_600_000);
    const long = historyChartOption([point("2026-09-01T00:00:00Z"), point("2026-09-07T00:00:00Z")], "en");
    expect(long.xAxis).toMatchObject({ minInterval: 86_400_000 });
    const label = (long.xAxis as { axisLabel: { formatter: (time: number) => string } }).axisLabel.formatter;
    expect(label(Date.parse("2026-09-07T00:00:00Z"))).not.toContain(":");
  });

  it("selects elapsed ranges relative to the latest saved observation without fabricating flat values", () => {
    const points = [point("2026-08-15T00:00:00Z"), point("2026-09-03T00:00:00Z"), point("2026-09-10T00:00:00Z")];
    expect(historyRangePoints(points, "24h")).toEqual([points[2]]);
    expect(historyRangePoints(points, "7d")).toEqual(points.slice(1));
    expect(historyRangePoints(points, "30d")).toEqual(points);
    const html = renderToStaticMarkup(<CapacityHistory history={history([point("2026-09-07T00:00:00Z"), point("2026-09-07T01:00:00Z")])} language="zh" />);
    expect(html).toContain("所选时段内已记录的额度没有变化");
    expect(html).toContain("30 天额度消耗热力图");
    expect(html).toContain('aria-pressed="true">7 天');
  });

  it("keeps a readable bounded vertical scale for tiny changes, empty data and the percentage extremes", () => {
    for (const [values, min, max] of [[[28, 29], 25, 35], [[100], 90, 100], [[0], 0, 10], [[], 0, 100]] as const) {
      expect(historyChartOption(values.map((value, i) => point(`2026-09-07T00:0${i}:00Z`, value)), "zh").yAxis).toMatchObject({ min, max });
    }
  });

  it("ignores invalid coordinates without turning unknown quota into zero", () => {
    expect(historySeries([point("invalid"), point("2026-09-07T00:00:00Z", NaN), point("2026-09-07T00:01:00Z", -1)])).toEqual([]);
  });

  it("uses elapsed time and separates changed reset-window boundaries", () => {
    const coordinates = buildHistoryCoordinates([
      point("2026-09-07T00:00:00Z"), point("2026-09-07T00:01:00Z"),
      point("2026-09-07T00:10:00Z", 98, "2026-09-17T00:00:00Z"),
    ], 100, 100, 10);
    expect(coordinates.map((value) => value.x)).toEqual([10, 18, 90]);
    expect(coordinates.map((value) => value.newCycle)).toEqual([false, false, true]);
  });

  it("distinguishes Keychain failure from an empty but working store", () => {
    const failure: DesktopHistoryEnvelope = { ...history([]), status: "binding_required", reasonCode: "history_keychain_unavailable" };
    expect(historyEmptyMessage(failure, "zh")).toContain("钥匙串暂不可用");
    expect(historyEmptyMessage(failure, "zh")).toContain("历史记录已暂停");
    expect(historyEmptyMessage(history([]), "zh")).toContain("首个样本");
    expect(historyEmptyMessage(null, "zh")).toContain("正在读取");
    expect(historyEmptyMessage({ ...failure, reasonCode: "history_keychain_corrupt" }, "zh")).toContain("历史密钥异常");
  });

  it("normalizes the backend's newest-first history without mutating it", () => {
    const newest = point("2026-09-07T00:10:00Z", 26);
    const oldest = point("2026-09-07T00:00:00Z", 28);
    const points = [newest, oldest];
    expect(chronologicalHistoryPoints(points)).toEqual([oldest, newest]);
    const coordinates = buildHistoryCoordinates(points);
    expect(coordinates[0].remainingPercent).toBe(28);
    expect(coordinates[1].remainingPercent).toBe(26);
    expect(points[0]).toBe(newest);
  });

  it("offers explicit authorization only for an access problem in the native host", () => {
    const failure: DesktopHistoryEnvelope = { ...history([]), status: "binding_required", reasonCode: "history_keychain_denied" };
    const props = { history: failure, language: "zh" as const };
    expect(renderToStaticMarkup(<CapacityHistory {...props} />)).not.toContain("<button");
    const html = renderToStaticMarkup(<CapacityHistory {...props} onAuthorize={() => undefined} />);
    expect(html).toContain("授权本地历史密钥");
    expect(html).toContain("请在 macOS 提示中选择");
    expect(html).toContain("始终允许");
    expect(html).toContain("授权后可查看旧历史并继续记录额度");
    expect(renderToStaticMarkup(<CapacityHistory {...props} onAuthorize={() => undefined} authorizing />)).toContain('disabled=""');
  });

  it("discloses rebuild authorization only for ad-hoc builds without promising lost history", () => {
    const failure: DesktopHistoryEnvelope = { ...history([]), status: "binding_required", reasonCode: "history_keychain_denied" };
    try {
      vi.stubEnv("VITE_HORIZON_SIGNING_MODE", "ad_hoc");
      const html = renderToStaticMarkup(<CapacityHistory history={failure} language="zh" onAuthorize={() => undefined} />);
      expect(html).toContain("为什么更新后需要授权？");
      expect(html).toContain("授权后即可继续查看和保存历史");
      expect(html).toContain("macOS 可能要求重新授权");
      vi.stubEnv("VITE_HORIZON_SIGNING_MODE", "certificate");
      expect(renderToStaticMarkup(<CapacityHistory history={failure} language="zh" onAuthorize={() => undefined} />)).not.toContain("为什么更新后需要授权？");
    } finally { vi.unstubAllEnvs(); }
  });
});

describe("quota activity heatmap", () => {
  const localPoint = (day: number, hour: number, minute: number, remaining: number, reset?: string) => point(new Date(2026, 8, day, hour, minute).toISOString(), remaining, reset);
  it("sums observed decreases before and after a rise, without treating the rise as negative usage", () => {
    const points = [localPoint(10, 9, 0, 40), localPoint(10, 9, 10, 38.4), localPoint(10, 9, 20, 99), localPoint(10, 9, 30, 98.3)];
    expect(historyActivity(history(points))).toEqual([{ date: "2026-09-10", sampleCount: 4, comparableIntervals: 3, consumedPercent: 2.3 }]);
  });
  it("does not estimate across midnight, long gaps, unknown or changed reset boundaries", () => {
    const points = [localPoint(9, 23, 50, 70), localPoint(10, 0, 10, 60), localPoint(10, 2, 0, 50),
      localPoint(10, 2, 10, 40, "2026-09-17T00:00:00Z"), { ...localPoint(10, 2, 20, 30), resetsAt: null }];
    expect(historyActivity(history(points)).every((day) => day.comparableIntervals === 0 && day.consumedPercent === 0)).toBe(true);
    const html = renderToStaticMarkup(<CapacityHistory history={history(points)} language="zh" />);
    expect(html).toContain("记录不足，消耗未知");
    expect(html).toContain("无记录");
  });
  it("uses raw native daily aggregates instead of recomputing from chart extrema", () => {
    const envelope = { ...history([localPoint(10, 9, 0, 50), localPoint(10, 10, 0, 40)]), overview: {
      sampleCount: 200, dailyActivity: [{ date: "2026-09-10", sampleCount: 200, comparableIntervals: 199, consumedPercent: 16.7 }],
    } };
    expect(historyActivity(envelope)).toEqual(envelope.overview.dailyActivity);
    const cells = historyCalendar(historyActivity(envelope), envelope.points[1].capturedAt);
    expect(cells.filter(Boolean)).toHaveLength(30);
    expect(cells.at(-1)).toMatchObject({ date: "2026-09-10", level: 4 });
    expect(cells.find((cell) => cell?.date === "2026-09-09")).toMatchObject({ level: -1 });
  });
  it("distinguishes observed zero from missing data and walks local calendar dates across DST", () => {
    const end = new Date(2026, 10, 10, 12);
    const days = [{ date: localHistoryDate(end), sampleCount: 2, comparableIntervals: 1, consumedPercent: 0 }];
    const cells = historyCalendar(days, end.toISOString()).filter((cell) => cell != null);
    expect(cells).toHaveLength(30);
    expect(new Set(cells.map((cell) => cell.date)).size).toBe(30);
    expect(cells[0].date).toBe("2026-10-12");
    expect(cells.at(-1)).toMatchObject({ date: "2026-11-10", level: 0 });
    expect(historyCalendar([], "invalid")).toEqual([]);
  });
});
