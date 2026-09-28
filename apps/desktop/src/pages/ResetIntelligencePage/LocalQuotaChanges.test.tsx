import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { DesktopHistoryEnvelope, DesktopQuotaRiseObservation } from "../../../../capacity-preview/src/status";
import { LocalQuotaChangesView, quotaRiseExplanation, quotaRiseLabel } from "./LocalQuotaChanges";
import { settleHistory, visibleHistory } from "../CapacityPage/historyState";

vi.mock("../../api/backend", () => ({ isDesktopApp: false }));

function observation(): DesktopQuotaRiseObservation {
  return {
    beforeSnapshotId: "internal_before", firstAfterSnapshotId: "internal_after", confirmationSnapshotId: "internal_followup",
    beforeAt: "2026-09-12T10:00:00Z", firstAfterAt: "2026-09-12T10:05:00Z", confirmedAt: "2026-09-12T10:06:00Z",
    beforeRemainingPercent: 18.4, afterRemainingPercent: 99.2, confirmedRemainingPercent: 98.7,
    beforeResetsAt: "2026-09-13T00:00:00Z", afterResetsAt: "2026-09-19T00:00:00Z",
    classification: "before_scheduled_boundary", limitation: null,
  };
}

function history(): DesktopHistoryEnvelope {
  return { schemaVersion: "1.0", historyContextId: "account-context-a", status: "available", reasonCode: "history_available", points: [],
    overview: { sampleCount: 4, dailyActivity: [], quotaChanges: {
      algorithmVersion: "internal_algorithm", validSamples: 4, comparableIntervals: 3, excludedIntervals: 0,
      dateOnlyChanges: 0, totalRises: 1, observations: [observation()],
    } } };
}

describe("local quota-rise presentation", () => {
  it("shows observed fractions and distinguishes a repeated early rise from its cause", () => {
    const html = renderToStaticMarkup(<LocalQuotaChangesView history={history()} language="zh" />);
    expect(html).toContain("18.4%");
    expect(html).toContain("99.2%");
    expect(html).toContain("98.7%");
    expect(html).toContain("原窗口到期前回升");
    expect(html).toContain("原因待确认");
    expect(html).toContain("原定到期前额度回升");
    for (const code of ["internal_algorithm", "internal_before", "internal_after", "internal_followup", "before_scheduled_boundary"]) expect(html).not.toContain(code);
  });

  it("keeps normal rollover wording separate from before-expiry changes", () => {
    const item = { ...observation(), classification: "around_scheduled_boundary" as const };
    expect(quotaRiseLabel(item, "zh")).toBe("窗口到期附近回升");
    expect(quotaRiseExplanation(item, "zh")).toContain("符合正常周期更新的时间");
    expect(quotaRiseExplanation(item, "zh")).toContain("原定到期前后");
  });
  it("describes a sub-threshold time shift without claiming that the time is unchanged", () => {
    const item = { ...observation(), limitation: "window_not_reanchored" as const };
    expect(quotaRiseExplanation(item, "zh")).toContain("未明显延后");
    expect(quotaRiseExplanation(item, "zh")).not.toContain("保持不变");
    expect(quotaRiseExplanation(item, "en")).toContain("without a material delay");
  });

  it("never treats an empty or interrupted observation history as proof no reset occurred", () => {
    const data = history();
    data.overview!.quotaChanges!.observations = [];
    const html = renderToStaticMarkup(<LocalQuotaChangesView history={data} language="zh" issue />);
    expect(html).toContain("当前记录中没有达到 5 个百分点的回升");
    expect(html).toContain("显示上次分析");
    data.overview!.quotaChanges!.validSamples = 2;
    expect(renderToStaticMarkup(<LocalQuotaChangesView history={data} language="zh" />)).toContain("采样不足");
  });

  it("explains drift without a material rise and does not invent an observation", () => {
    const data = history();
    data.overview!.quotaChanges!.observations = [];
    data.overview!.quotaChanges!.dateOnlyChanges = 3;
    const html = renderToStaticMarkup(<LocalQuotaChangesView history={data} language="zh" />);
    expect(html).toContain("3 次窗口日期变化");
    expect(html).toContain("没有按重置处理");
    expect(html).not.toContain("99.2%");
  });

  it("uses actionable authorization text without invoking the keychain or hiding its limitation", () => {
    const data = { ...history(), status: "binding_required" as const, reasonCode: "history_keychain_interaction_required" };
    const html = renderToStaticMarkup(<LocalQuotaChangesView history={data} language="zh" />);
    expect(html).toContain("额度规划");
    expect(html).toContain("授权后可查看旧历史");
    expect(html).toContain("请前往「额度规划」授权");
    expect(html).not.toContain("history_keychain_interaction_required");
    expect(html).not.toContain("99.2%");
  });

  it("does not fall back to compressed chart points when the new analysis is absent", () => {
    const data = history(); delete data.overview!.quotaChanges;
    const html = renderToStaticMarkup(<LocalQuotaChangesView history={data} language="zh" />);
    expect(html).toContain("暂时没有可用的回升分析");
    expect(html).not.toContain("未发现");
  });

  it("maps every limitation to product language and treats unknown labels cautiously", () => {
    const reasons: NonNullable<DesktopQuotaRiseObservation["limitation"]>[] = [
      "awaiting_followup", "observation_gap", "window_unavailable", "window_not_reanchored", "window_changed_again",
      "not_sustained", "ambiguous_timing", "source_changed", "invalid_sample",
    ];
    for (const limitation of reasons) {
      expect(quotaRiseExplanation({ ...observation(), limitation }, "zh")).not.toContain("_");
      expect(quotaRiseExplanation({ ...observation(), limitation }, "en")).not.toContain("_");
    }
    const unknown = { ...observation(), classification: "new_internal_state" } as unknown as DesktopQuotaRiseObservation;
    expect(quotaRiseLabel(unknown, "zh")).toBe("回升证据不足");
    expect(quotaRiseExplanation(unknown, "zh")).toContain("等待后续采样");
  });

  it("limits initial rendering and does not expose an old account's observations after context changes", () => {
    const data = history();
    data.overview!.quotaChanges!.observations = Array.from({ length: 8 }, (_, index) => ({ ...observation(), firstAfterSnapshotId: `fixture_${index}` }));
    const html = renderToStaticMarkup(<LocalQuotaChangesView history={data} language="zh" />);
    expect(html.match(/99\.2%/g)).toHaveLength(3);
    expect(html).toContain("还有 5 条");
    const previous = { key: "account-a", history: data, issue: null };
    expect(visibleHistory(previous, "account-b").history).toBeNull();
    expect(settleHistory(previous, "account-a", "account-context-a", { ...data, historyContextId: "account-context-b" }).history).toBeNull();
  });
});
