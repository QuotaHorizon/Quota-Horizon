import { describe, expect, it } from "vitest";
import { canFetchRelayModels } from "./providerUtils";

describe("canFetchRelayModels", () => {
  it("allows a new credential for a configured endpoint", () => {
    expect(canFetchRelayModels("https://relay.example.com/v1", "sk-new", undefined, false))
      .toBe(true);
  });

  it("allows an existing provider to reuse its saved credential", () => {
    expect(canFetchRelayModels("https://relay.example.com/v1", "", "provider-a", true))
      .toBe(true);
  });

  it("requires an endpoint and a usable credential source", () => {
    expect(canFetchRelayModels("", "sk-new", undefined, false)).toBe(false);
    expect(canFetchRelayModels("https://relay.example.com/v1", "", "provider-a", false))
      .toBe(false);
  });
});
