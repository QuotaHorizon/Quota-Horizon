import { describe, expect, it } from "vitest";
import { resolveUsageResetWindow } from "./UsageMeter";

describe("usage meter reset semantics", () => {
  it("uses the upstream window duration instead of a mismatched presentation hint", () => {
    expect(resolveUsageResetWindow({
      usedPercent: 20,
      remainingPercent: 80,
      windowMinutes: 300,
    }, "oneWeek")).toBe("fiveHours");
    expect(resolveUsageResetWindow({
      usedPercent: 40,
      remainingPercent: 60,
      windowMinutes: 10_080,
    }, "fiveHours")).toBe("oneWeek");
  });

  it("keeps the column fallback for legacy cached windows without a duration", () => {
    expect(resolveUsageResetWindow({
      usedPercent: 20,
      remainingPercent: 80,
    }, "fiveHours")).toBe("fiveHours");
  });
});
