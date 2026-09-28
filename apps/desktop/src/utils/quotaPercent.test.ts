import { describe, expect, it } from "vitest";
import { planPercentLabel, quotaPercentLabel } from "./quotaPercent";

describe("observed quota and calculated plan precision", () => {
  it("does not pad integer quota or turn real zero into missing quota", () => {
    expect(quotaPercentLabel(98)).toBe("98%");
    expect(quotaPercentLabel(0)).toBe("0%");
    expect(quotaPercentLabel(-0)).toBe("0%");
    expect(quotaPercentLabel(100)).toBe("100%");
  });
  it("keeps real fractional quota to one decimal without floating-point tails", () => {
    expect(quotaPercentLabel(28.4)).toBe("28.4%");
    expect(quotaPercentLabel(28.36)).toBe("28.4%");
    expect(quotaPercentLabel(28.35)).toBe("28.4%");
    expect(quotaPercentLabel(100 - 71.6)).toBe("28.4%");
    expect(quotaPercentLabel(28.01)).toBe("28%");
  });
  it("keeps one decimal on calculated plan targets", () => {
    expect(planPercentLabel(28)).toBe("28.0%");
    expect(planPercentLabel(100 / 7)).toBe("14.3%");
    expect(planPercentLabel(0)).toBe("0.0%");
  });
  it("does not pretend unavailable or invalid input is a real zero", () => {
    for (const value of [null, undefined, NaN, Infinity, -Infinity, -1, 101]) {
      expect(quotaPercentLabel(value)).toBe("—");
      expect(planPercentLabel(value)).toBe("—");
    }
  });
});
