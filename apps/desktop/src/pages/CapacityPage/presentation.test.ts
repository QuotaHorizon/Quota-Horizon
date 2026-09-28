import { describe, expect, it } from "vitest";
import type { DesktopHistoryPoint, DesktopQuotaWindow } from "../../../../capacity-preview/src/status";
import {
  buildHistoryPath,
  clampPercent,
  selectPrimaryQuotaWindow,
  selectShortestQuotaWindow,
} from "./presentation";

const windowFixture = (limitId: string, windowMinutes: number): DesktopQuotaWindow => ({
  limitId,
  label: null,
  windowMinutes,
  usedPercent: 40,
  remainingPercent: 60,
  resetsAt: null,
});

describe("capacity presentation", () => {
  it("selects the longest quota window as the planning horizon", () => {
    const windows = [windowFixture("five-hour", 300), windowFixture("weekly", 10_080)];
    expect(selectPrimaryQuotaWindow(windows)?.limitId).toBe("weekly");
    expect(selectShortestQuotaWindow(windows)?.limitId).toBe("five-hour");
  });

  it("clamps invalid percentages before drawing history", () => {
    expect(clampPercent(-20)).toBe(0);
    expect(clampPercent(140)).toBe(100);
    expect(clampPercent(Number.NaN)).toBe(0);

    const points = [
      { remainingPercent: 100 },
      { remainingPercent: 50 },
      { remainingPercent: 0 },
    ] as DesktopHistoryPoint[];
    expect(buildHistoryPath(points, 100, 100, 10)).toBe("M10.0,10.0 L50.0,50.0 L90.0,90.0");
  });

  it("keeps the main app on the same canonical account quota as the popover", () => {
    const weekly = { ...windowFixture("codex:primary", 10_080), remainingPercent: 28 };
    const windows = [
      { ...windowFixture("codex_bengalfox:secondary", 10_080), remainingPercent: 100 },
      windowFixture("codex_bengalfox:primary", 300),
      weekly,
    ];
    expect(selectPrimaryQuotaWindow(windows)).toBe(weekly);
    expect(selectShortestQuotaWindow(windows)).toBe(weekly);
    expect(windows[0].limitId).toBe("codex_bengalfox:secondary");
  });
});
