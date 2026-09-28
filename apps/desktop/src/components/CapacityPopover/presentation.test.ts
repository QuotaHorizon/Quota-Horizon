import { describe, expect, it } from "vitest";
import type { DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import {
  acceptNewerEnvelope,
  accountResetCountdown,
  accountWeeklyResetDate,
  accountUsageFreshnessLabel,
  capacityTone,
  idleCapacityEnvelope,
  popoverWindows,
  minuteSpanLabel,
  minuteToTimeValue,
  paceTone,
  percentValueLabel,
  planPercentLabel,
  resetCountdown,
  timeValueToMinute,
  updatedLabel,
} from "./presentation";

const envelope = (sequence: number): DesktopStatusEnvelope => ({
  schemaVersion: "1.0",
  sequence,
  lifecycle: "ready",
  status: null,
  candidates: [],
  selectedExecutableId: null,
  issue: null,
  persistenceEnabled: false,
});

describe("capacity popover presentation", () => {
  it("separates observed quota precision from calculated plan values", () => {
    expect(percentValueLabel(28)).toBe("28%");
    expect(percentValueLabel(28.36)).toBe("28.4%");
    expect(percentValueLabel(0)).toBe("0%");
    expect(planPercentLabel(28)).toBe("28.0%");
    expect(percentValueLabel(null)).toBe("—");
    expect(percentValueLabel(Number.NaN)).toBe("—");
  });
  it("never boots the native popover with fixture quota", () => {
    const initial = idleCapacityEnvelope();
    expect(initial.lifecycle).toBe("idle");
    expect(initial.status).toBeNull();
    expect(initial.sequence).toBe(0);
  });

  it("keeps the newest monitor envelope", () => {
    expect(acceptNewerEnvelope(envelope(4), envelope(3)).sequence).toBe(4);
    expect(acceptNewerEnvelope(envelope(4), envelope(5)).sequence).toBe(5);
  });

  it("selects the longest planning window and shortest companion window", () => {
    const windows = [
      { limitId: "short", label: null, windowMinutes: 300, usedPercent: 12, remainingPercent: 88, resetsAt: null },
      { limitId: "week", label: null, windowMinutes: 10_080, usedPercent: 28, remainingPercent: 72, resetsAt: null },
    ];
    const selected = popoverWindows(windows);
    expect(selected.primary?.limitId).toBe("week");
    expect(selected.secondary?.limitId).toBe("short");
  });

  it("keeps model-specific limits out of the canonical Codex summary", () => {
    const windows = [
      { limitId: "codex:primary", label: null, windowMinutes: 10_080, usedPercent: 68, remainingPercent: 32, resetsAt: null },
      { limitId: "codex_bengalfox:primary", label: null, windowMinutes: 300, usedPercent: 0, remainingPercent: 100, resetsAt: null },
      { limitId: "codex_bengalfox:secondary", label: null, windowMinutes: 10_080, usedPercent: 0, remainingPercent: 100, resetsAt: null },
    ];

    const selected = popoverWindows(windows, "prolite");
    expect(selected.primary?.remainingPercent).toBe(32);
    expect(selected.secondary).toBeNull();
  });

  it("uses visible thresholds and honest countdown states", () => {
    expect(capacityTone(72)).toBe("healthy");
    expect(capacityTone(30)).toBe("attention");
    expect(capacityTone(10)).toBe("critical");
    expect(resetCountdown("2026-09-04T03:00:00Z", Date.parse("2026-09-02T00:00:00Z"), "zh"))
      .toBe("2 天 3 小时后");
    expect(resetCountdown("2026-09-01T00:00:00Z", Date.parse("2026-09-02T00:00:00Z"), "en"))
      .toBe("Awaiting official refresh");
  });

  it("describes snapshot age without false precision", () => {
    expect(updatedLabel("2026-09-02T00:00:00Z", Date.parse("2026-09-02T00:00:30Z"), "zh"))
      .toBe("刚刚更新");
    expect(updatedLabel("2026-09-02T00:00:00Z", Date.parse("2026-09-02T00:07:00Z"), "en"))
      .toBe("Updated 7m ago");
    expect(accountUsageFreshnessLabel(
      "2026-09-02T00:00:00Z",
      true,
      Date.parse("2026-09-02T00:07:00Z"),
      "zh",
    )).toBe("刷新失败 · 已保留旧数据");
    expect(accountResetCountdown(
      Date.parse("2026-09-02T02:00:00Z") / 1_000,
      Date.parse("2026-09-02T00:00:00Z"),
      "en",
    )).toBe("in 2h 0m");
  });

  it("formats and validates work schedule controls", () => {
    expect(minuteToTimeValue(120)).toBe("02:00");
    expect(minuteToTimeValue(1_500)).toBe("01:00");
    expect(timeValueToMinute("23:45")).toBe(1_425);
    expect(timeValueToMinute("24:00")).toBeNull();
    expect(minuteSpanLabel(960, "zh")).toBe("16 小时");
    expect(paceTone("on_pace")).toBe("on-pace");
    expect(paceTone("over_guard")).toBe("over");
  });

  it("includes the year and exact minute for weekly account resets", () => {
    const timestamp = Date.parse("2026-09-07T05:04:14Z") / 1000;
    const formatted = accountWeeklyResetDate(timestamp, "zh");
    expect(formatted).toContain("2026");
    expect(formatted).toMatch(/\d{2}:04/);
    expect(formatted).not.toMatch(/:14\b/);
    expect(accountWeeklyResetDate(null, "zh")).toBe("—");
  });
});
