import {renderToStaticMarkup} from "react-dom/server";
import {describe, expect, it, vi} from "vitest";
import type {DesktopPaceEnvelope, PaceEstimate} from "../../../../capacity-preview/src/status";
import {CapacityPaceView, paceIsCurrent, paceMessage, paceRangeLabel} from "./CapacityPaceView";
import {createPaceStore} from "./paceStore";
const now = Date.parse("2026-09-26T06:00:00Z");
function estimate(): PaceEstimate {
  return {paceEstimateId: "private-estimate", algorithmVersion: "recent-block-pace-experimental-v1", state: "pace_only", reasonCode: "pace_scenario", limitId: "codex:weekly", windowMinutes: 10080, generatedAt: new Date(now).toISOString(), observedAt: new Date(now).toISOString(), forecastHorizon: "2026-09-26T12:00:00Z", currentRemainingPercent: 25, inputSnapshotIds: ["private-snapshot"], historyCoverage: {sampleCount: 25, coveredSeconds: 21600, requiredSeconds: 21600, maximumGapSeconds: 900}, observedBlockRates: [1, 2, 1.5], rateRange: {status: "available", lower: 1, upper: 2, reasonCode: "pace_scenario"}, balanceAtHorizon: {status: "available", lower: 13, upper: 19, reasonCode: "pace_scenario"}, estimatedCodexActiveHoursRange: {status: "unavailable", lower: null, upper: null, reasonCode: "missing_calibrated_active_time"}, depletionTimeRange: {status: "unavailable", earliestAt: null, latestAt: null, reasonCode: "beyond_horizon"}, compatibilityUnverified: false, cachedSource: false, dataStability: "recent_observed_range", assumptions: ["elapsed_not_active_time"], invalidationFactors: ["other_device_usage"], decision: "not_assessed", backtestStatus: "not_started"};
}
function envelope(): DesktopPaceEnvelope {return {schemaVersion: "1.0", historyContextId: "a", status: "available", reasonCode: "pace_available", estimates: [estimate()], generatedAt: new Date(now).toISOString()};}
function render(value: DesktopPaceEnvelope | null, props: Partial<Parameters<typeof CapacityPaceView>[0]> = {}) {return renderToStaticMarkup(<CapacityPaceView value={value} language="zh" contextId="a" onRetry={() => undefined} now={now} {...props}/>);}
function deferred<T>() {let resolve!: (value: T) => void; const promise = new Promise<T>(yes => {resolve = yes;}); return {resolve, promise};}
describe("experimental recent pace", () => {
  it("shows scenario ranges and evidence without claiming active hours or plan fit", () => {
    const html = render(envelope());expect(html).toContain("13–19%");expect(html).toContain("1–2");expect(html).toContain("日历小时");expect(html).toContain("结果为实验估算");expect(html).toContain("未校准");expect(html).toContain("准确性尚待校准");expect(html).toContain("按日历时间计算");
    for (const raw of ["private-estimate", "private-snapshot", "pace_only", "pace_scenario", "not_started", "not_assessed", "codex:weekly", "recent-block-pace-experimental-v1"]) expect(html).not.toContain(raw);
  });
  it("explains missing plan context and never renders abstention as zero balance", () => {
    const value = envelope();Object.assign(value.estimates[0], {state: "abstained", reasonCode: "plan_context_missing", balanceAtHorizon: {status: "unavailable", lower: null, upper: null, reasonCode: "plan_context_missing"}});
    const html = render(value);expect(html).toContain("旧记录缺少当时的套餐信息");expect(html).not.toContain("13–19%");expect(html).not.toContain("0%");
    for (const code of ["unstable_pace", "balance_increased", "window_changed", "insufficient_usage", "source_unavailable", "clock_discontinuity", "observation_gap"]) expect(paceMessage(code, "zh")).not.toContain(code);
    for (const code of ["plan_binding_required", "plan_store_unavailable", "future_internal_code"]) {
      expect(paceMessage(code, "zh")).not.toMatch(/保存计划|预算|future_internal_code/);
    }
  });
  it("withdraws numerical bands after failed reads, expiry or source age", () => {
    expect(render(envelope(), {failed: true})).not.toContain("13–19%");expect(render(envelope(), {failed: true})).toContain("暂时撤下");
    expect(render(envelope(), {now: now + 1_800_001})).toContain("超过 30 分钟");expect(render(envelope(), {now: now + 1_800_001})).not.toContain("13–19%");
    const ended = envelope();ended.estimates[0].forecastHorizon = new Date(now).toISOString();expect(render(ended)).toContain("已经到期");expect(render(ended)).not.toContain("13–19%");
    expect(paceIsCurrent(estimate(), now - 1)).toBe(false);
    expect(render(envelope(), {now: now - 1})).toContain("晚于当前电脑时间");
  });
  it("keeps account changes and authorization loss from showing the previous band", () => {
    expect(render({...envelope(), historyContextId: "b"})).not.toContain("13–19%");
    expect(render({...envelope(), status: "unavailable", estimates: [], reasonCode: "history_keychain_denied"})).toContain("本地历史密钥尚未授权");
  });
  it("shows bounded depletion and marks cached/unverified inputs", () => {
    const value = envelope();Object.assign(value.estimates[0], {cachedSource: true, compatibilityUnverified: true, depletionTimeRange: {status: "available", earliestAt: "2026-09-26T08:00:00Z", latestAt: "2026-09-26T09:00:00Z", reasonCode: "pace_scenario"}});
    const html = render(value);expect(html).toContain("可能耗尽于");expect(html).toContain("缓存读数");expect(html).toContain("兼容性待验证");expect(render(value, {language: "en"})).toContain("Reader compatibility is unverified");
  });
  it("does not invent a Pro short window and distinguishes empty scope", () => {
    expect(render(envelope())).not.toContain("5 小时额度");
    expect(render({...envelope(), estimates: []})).toContain("没有可用于推算的官方固定额度窗口");
  });
  it("labels a narrow band as approximate without padded zero decimals", () => {
    const range = {...estimate().rateRange, lower: 2, upper: 2};expect(paceRangeLabel(range, "zh")).toBe("≈ 2");expect(paceRangeLabel({...range, lower: null}, "zh")).toBe("—");
  });
  it("coalesces refreshes and stops trailing work when unmounted", async () => {
    for (const close of [true, false]) {
      const pending = deferred<DesktopPaceEnvelope>();const read = vi.fn().mockImplementationOnce(() => pending.promise).mockResolvedValue(envelope());const store = createPaceStore("a", read);const dispose = store.subscribe(() => undefined);
      const task = store.reload();void store.reload();if (close) dispose();pending.resolve(envelope());await task;expect(read).toHaveBeenCalledTimes(close ? 1 : 2);dispose();
    }
  });
  it("retains evidence on failures but clears it on authorization or context loss", async () => {
    const read = vi.fn().mockResolvedValueOnce(envelope()).mockRejectedValueOnce(new Error("private detail")).mockResolvedValueOnce({...envelope(), status: "unavailable", estimates: []}).mockResolvedValueOnce({...envelope(), historyContextId: "b"});
    const store = createPaceStore("a", read);await store.reload();await store.reload();expect(store.getSnapshot().failed).toBe(true);expect(store.getSnapshot().value?.estimates.length).toBe(1);
    await store.reload();expect(store.getSnapshot().value?.estimates).toEqual([]);await store.reload();expect(store.getSnapshot().value).toBeNull();
  });
});
