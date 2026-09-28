import { describe, expect, it } from "vitest";
import type { CpaPoolMember, Provider } from "../../types";
import {
  cpaGuardReasonKey, cpaGuardStateKey, cpaIssueKey, cpaRouteKeys, isLoopbackProvider,
  remainingPercent,
} from "./cpaPoolPresentation";

const provider = (baseUrl: string): Provider => ({
  id: "provider",
  kind: "custom",
  name: "Provider",
  group: "",
  baseUrl,
  model: "gpt",
  models: ["gpt"],
  modelReasoningEfforts: {},
  modelContextWindows: {},
  modelApiFormats: {},
  imageInputModels: [],
  imageInputModelsConfigured: false,
  modelSelectionControlledByCodex: false,
  apiFormat: "openaiResponses",
  active: true,
  autoSwitchEnabled: false,
  hasApiKey: true,
  supportsDirectSwitch: true,
  balanceQueryUsesApiKey: false,
  hasBalanceQueryToken: false,
  hasWalletQueryToken: false,
  hasWalletLoginCredentials: false,
});

const member = (values: Partial<CpaPoolMember>): CpaPoolMember => ({
  id: "member",
  displayName: "Member",
  isCurrentRoute: false,
  isRoutePreferred: false,
  isLatestRequestRoute: false,
  isStale: false,
  refreshSkipped: false,
  ...values,
});

describe("CPA pool presentation", () => {
  it("recognizes only loopback Provider endpoints", () => {
    expect(isLoopbackProvider(provider("http://localhost:8317/v1"))).toBe(true);
    expect(isLoopbackProvider(provider("http://127.0.0.1:8317/v1"))).toBe(true);
    expect(isLoopbackProvider(provider("http://[::1]:8317/v1"))).toBe(true);
    expect(isLoopbackProvider(provider("https://api.example.com/v1"))).toBe(false);
  });

  it("preserves independent route roles", () => {
    expect(cpaRouteKeys(member({
      isCurrentRoute: true,
      isRoutePreferred: true,
      isLatestRequestRoute: true,
    }))).toEqual([
      "providers.cpa.route.current",
      "providers.cpa.route.preferred",
      "providers.cpa.route.latest",
    ]);
  });

  it("does not invent a percentage for an unknown quota", () => {
    expect(remainingPercent(null)).toBe(null);
    expect(remainingPercent(Number.NaN)).toBe(null);
    expect(remainingPercent(53.4)).toBe("53%");
  });

  it("maps stable backend issue codes to localized copy", () => {
    expect(cpaIssueKey("bridge_disabled")).toBe("providers.cpa.issue.bridgeDisabled");
    expect(cpaIssueKey("bridge_timeout")).toBe("providers.cpa.issue.bridgeTimeout");
    expect(cpaIssueKey("invalid_response")).toBe("providers.cpa.issue.invalidResponse");
  });

  it("shows only known guard reasons and hides active guard noise", () => {
    expect(cpaGuardStateKey("active")).toBe(null);
    expect(cpaGuardStateKey("quota_quarantine")).toBe("providers.cpa.guard.quarantine");
    expect(cpaGuardReasonKey("weekly_quota_near_limit"))
      .toBe("providers.cpa.guard.reason.weeklyNearLimit");
    expect(cpaGuardReasonKey("internal_probe_path=/secret"))
      .toBe(null);
  });
});
