import { describe, expect, it } from "vitest";
import { resetCreditExpiryHint, summarizeResetCreditExpiry } from "./resetCreditExpiry";

const now = Date.parse("2026-10-30T00:00:00Z");
const soon = "2026-10-30T08:00:00Z";

describe("reset-card expiry", () => {
  it("uses only available future cards and keeps incomplete coverage visible", () => {
    const result = summarizeResetCreditExpiry([
      { available: true, expiresAt: "2026-11-02T00:00:00Z" },
      { available: true, expiresAt: soon },
      { available: false, expiresAt: "2026-10-30T01:00:00Z" },
      { expiresAt: "2026-10-30T02:00:00Z" },
      { available: true, expiresAt: null },
      { available: true, expiresAt: "2026-10-30T00:00:00Z" },
    ], now);
    expect(result).toEqual({ nextExpiresAt: "2026-10-30T08:00:00.000Z", knownExpiryCount: 2, expiryDetailsComplete: false });
    const hint = resetCreditExpiryHint(result, 5, "en", { now });
    expect(hint?.text).toContain("Known expiry");
    expect(hint?.title).toContain("2 of 5");
  });

  it.each(["zh", "en"] as const)("formats complete deadlines and saved captures in %s", (language) => {
    const summary = summarizeResetCreditExpiry([{ available: true, expiresAt: soon }], now);
    expect(summary.expiryDetailsComplete).toBe(true);
    expect(resetCreditExpiryHint(summary, 1, language, { now })?.urgent).toBe(true);
    const saved = resetCreditExpiryHint(summary, 1, language, { now, saved: true, capturedAt: "2026-10-29T23:58:00Z" });
    expect(saved?.text).toContain(language === "zh" ? "记录" : "Saved");
    expect(saved?.title).toContain(language === "zh" ? "读取于" : "Read");
    expect(saved?.urgent).toBe(false);
  });

  it("keeps unknown and elapsed captures out of upcoming expiry hints", () => {
    const unknown = summarizeResetCreditExpiry([{ expiresAt: soon }, { available: true, expiresAt: "invalid" }], now);
    expect(unknown.knownExpiryCount).toBe(0);
    expect(resetCreditExpiryHint(unknown, 2, "zh", { now })?.text).toBe("到期时间未知");
    const elapsed = resetCreditExpiryHint({ nextExpiresAt: soon, knownExpiryCount: 1, expiryDetailsComplete: true },
      1, "en", { now: Date.parse(soon), saved: true });
    expect(elapsed?.text).toBe("Saved · Expiry needs refresh");
    expect(elapsed?.urgent).toBe(false);
    expect(resetCreditExpiryHint({}, 0, "en", { now })).toBeNull();
  });
});
