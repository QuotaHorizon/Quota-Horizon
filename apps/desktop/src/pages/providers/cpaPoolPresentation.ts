import type { TranslationKey } from "../../i18n";
import type { CpaPoolIssueCode, CpaPoolMember, Provider } from "../../types";

const ISSUE_KEYS: Record<CpaPoolIssueCode, TranslationKey> = {
  bridge_disabled: "providers.cpa.issue.bridgeDisabled",
  invalid_configuration: "providers.cpa.issue.invalidConfiguration",
  unsupported_platform: "providers.cpa.issue.unsupportedPlatform",
  bridge_unavailable: "providers.cpa.issue.bridgeUnavailable",
  bridge_timeout: "providers.cpa.issue.bridgeTimeout",
  bridge_rejected: "providers.cpa.issue.bridgeRejected",
  output_too_large: "providers.cpa.issue.outputTooLarge",
  invalid_response: "providers.cpa.issue.invalidResponse",
  no_records: "providers.cpa.issue.noRecords",
};

export function cpaIssueKey(code: CpaPoolIssueCode): TranslationKey {
  return ISSUE_KEYS[code];
}

export function isLoopbackProvider(provider: Provider): boolean {
  try {
    const hostname = new URL(provider.baseUrl).hostname.toLowerCase();
    return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "[::1]";
  } catch {
    return false;
  }
}

export function cpaRouteKeys(member: CpaPoolMember): TranslationKey[] {
  const keys: TranslationKey[] = [];
  if (member.isCurrentRoute) keys.push("providers.cpa.route.current");
  if (member.isRoutePreferred) keys.push("providers.cpa.route.preferred");
  if (member.isLatestRequestRoute) keys.push("providers.cpa.route.latest");
  return keys;
}

export function remainingPercent(value?: number | null): string | null {
  if (value === null || value === undefined || !Number.isFinite(value)) return null;
  return `${Math.round(Math.max(0, Math.min(100, value)))}%`;
}

export function cpaGuardStateKey(state?: string | null): TranslationKey | null {
  if (!state || state === "active") return null;
  if (state === "quota_quarantine") return "providers.cpa.guard.quarantine";
  if (state === "canary") return "providers.cpa.guard.canary";
  return "providers.cpa.guard.unknown";
}

export function cpaGuardReasonKey(reason?: string | null): TranslationKey | null {
  switch (reason) {
    case "usage_limit_reached": return "providers.cpa.guard.reason.limitReached";
    case "rate_limited": return "providers.cpa.guard.reason.rateLimited";
    case "primary_quota_near_limit": return "providers.cpa.guard.reason.primaryNearLimit";
    case "weekly_quota_near_limit": return "providers.cpa.guard.reason.weeklyNearLimit";
    default: return null;
  }
}
