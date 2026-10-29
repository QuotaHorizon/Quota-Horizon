import { describe, expect, it, vi } from "vitest";
import { cachedStateFrom } from "./useResetCredits";

vi.mock("../api/backend", () => ({ fetchResetCredits: vi.fn() }));

describe("reset-card cache", () => {
  it("preserves availability and drops unavailable cards without caching private fields", () => {
    const result = cachedStateFrom({ status: "loaded", fetchedAt: "2026-10-30T00:00:00Z", data: { credits: [
      { available: true, expiresAt: "2026-11-01T00:00:00Z", creditId: "private-id" },
      { available: false, expiresAt: "2026-11-01T00:00:00Z" },
      { available: "true", expiresAt: "2026-11-02T00:00:00Z" },
    ] } });
    expect(result?.data.credits).toEqual([
      { issuedAt: null, available: true, expiresAt: "2026-11-01T00:00:00Z" },
      { issuedAt: null, available: null, expiresAt: "2026-11-02T00:00:00Z" },
    ]);
    expect(JSON.stringify(result)).not.toContain("private-id");
  });

  it("keeps old date-only cards readable with unknown availability", () => {
    const result = cachedStateFrom({ status: "loaded", fetchedAt: "2026-10-30T00:00:00Z",
      data: { credits: [{ expiresAt: "2026-11-01T00:00:00Z" }] } });
    expect(result?.data.credits).toHaveLength(1);
    expect(result?.data.credits[0].available).toBeNull();
    expect(cachedStateFrom({ status: "loading" })).toBeNull();
  });
});
