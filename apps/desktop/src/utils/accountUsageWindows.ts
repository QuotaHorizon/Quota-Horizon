import type { UsageSummary, UsageWindow } from "../types";

export interface AccountUsageWindows {
  short: UsageWindow | null;
  weekly: UsageWindow | null;
}

const SHORT_WINDOW_MAX_MINUTES = 24 * 60;

export function planHasNoShortQuotaWindow(plan: string | null | undefined) {
  const normalized = plan?.trim().toLocaleLowerCase().replace(/[^a-z0-9]/g, "");
  return normalized === "pro"
    || normalized === "prolite"
    || normalized === "chatgptpro"
    || normalized === "chatgptprolite";
}

export function accountUsageWindows(
  usage: UsageSummary,
  plan: string | null | undefined,
): AccountUsageWindows {
  const entries = [usage.primary, usage.secondary].filter(
    (window): window is UsageWindow => Boolean(window),
  );
  const withDuration = entries.filter((window) => (
    typeof window.windowMinutes === "number"
      && Number.isFinite(window.windowMinutes)
      && window.windowMinutes > 0
  ));
  const weekly = withDuration
    .filter((window) => (window.windowMinutes ?? 0) > SHORT_WINDOW_MAX_MINUTES)
    .sort((left, right) => (right.windowMinutes ?? 0) - (left.windowMinutes ?? 0))[0]
    ?? (planHasNoShortQuotaWindow(plan) ? usage.primary ?? usage.secondary ?? null : null)
    ?? (usage.secondary?.windowMinutes ? null : usage.secondary ?? null);
  const short = planHasNoShortQuotaWindow(plan)
    ? null
    : withDuration
      .filter((window) => (window.windowMinutes ?? Number.POSITIVE_INFINITY) <= SHORT_WINDOW_MAX_MINUTES)
      .sort((left, right) => (left.windowMinutes ?? 0) - (right.windowMinutes ?? 0))[0]
      ?? (usage.primary === weekly ? null : usage.primary ?? null);

  return { short, weekly };
}
