import { describe, expect, it } from "vitest";
import { accountUsageWindows, planHasNoShortQuotaWindow } from "./accountUsageWindows";

describe("account usage window selection", () => {
  it("treats a Pro weekly-only response as weekly and leaves 5h empty", () => {
    const weekly = { usedPercent: 51, remainingPercent: 49, windowMinutes: 10_080 };
    const selected = accountUsageWindows({ primary: weekly, secondary: null }, "prolite");

    expect(selected.weekly).toBe(weekly);
    expect(selected.short).toBeNull();
    expect(planHasNoShortQuotaWindow("ChatGPT Pro")).toBe(true);
    expect(planHasNoShortQuotaWindow("pro")).toBe(true);
  });

  it("selects Plus windows by duration instead of primary/secondary position", () => {
    const weekly = { usedPercent: 43, remainingPercent: 57, windowMinutes: 10_080 };
    const short = { usedPercent: 28, remainingPercent: 72, windowMinutes: 300 };

    expect(accountUsageWindows({ primary: weekly, secondary: short }, "plus"))
      .toEqual({ short, weekly });
  });

  it("keeps the legacy primary/secondary fallback when durations are missing", () => {
    const primary = { usedPercent: 10, remainingPercent: 90 };
    const secondary = { usedPercent: 20, remainingPercent: 80 };

    expect(accountUsageWindows({ primary, secondary }, "plus"))
      .toEqual({ short: primary, weekly: secondary });
  });
});
