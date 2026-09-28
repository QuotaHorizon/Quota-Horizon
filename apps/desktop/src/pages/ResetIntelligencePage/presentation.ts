import type { DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { planHasNoShortQuotaWindow } from "../../utils/accountUsageWindows";
import type { PublicEvidenceDisposition, PublicResetArchiveEntry } from "./types";

export function evidenceDispositionLabel(disposition: PublicEvidenceDisposition | string, language: Language) {
  const labels: Record<string, readonly [string, string]> = {
    confirmed: ["已核实公告", "Reviewed confirmation"],
    announced: ["明确预告", "Explicit announcement"],
    possible: ["可能相关", "Possible signal"],
    negative: ["否定消息", "Negative signal"],
    context: ["背景说明", "Context"],
    needs_review: ["待核实线索", "Needs review"],
    corrected: ["已更正", "Corrected"],
    retracted: ["已撤回", "Retracted"],
  };
  return (labels[disposition] ?? labels.needs_review)[language === "zh" ? 0 : 1];
}

export function publicEvidenceTime(value: string | null | undefined, language: Language) {
  const timestamp = value ? Date.parse(value) : NaN;
  if (!Number.isFinite(timestamp)) return "—";
  return new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-US", {
    year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", timeZoneName: "short",
  }).format(new Date(timestamp));
}

export function reviewedEvidence(entry: PublicResetArchiveEntry) {
  return entry.signal.source.review === "primary_reviewed";
}

export function publicEvidenceUrl(value: string): string | null {
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || url.username || url.password || url.port
      || /[\s\u0000-\u001f]/u.test(value)) return null;
    const allowed = ["help.openai.com", "openai.com", "status.openai.com", "x.com",
      "quotaresets.com", "codex-reset.com"];
    return allowed.includes(url.hostname) ? url.href : null;
  } catch { return null; }
}

/** Only canonical Codex windows belong in the account panel, never Spark. */
export function localResetFacts(envelope: DesktopStatusEnvelope | null) {
  const status = envelope?.status;
  const canonical = status?.quotaWindows.filter((window) => (
    window.limitId === "codex" || window.limitId.startsWith("codex:")
  )) ?? [];
  const noShortWindow = planHasNoShortQuotaWindow(status?.account?.planType);
  return {
    weekly: canonical.find((window) => window.windowMinutes === 10_080) ?? null,
    short: noShortWindow ? null : canonical.find((window) => window.windowMinutes === 300) ?? null,
    noShortWindow,
    observedAt: status?.quotaObservedAt ?? status?.capturedAt,
    fresh: status != null && envelope?.lifecycle === "ready"
      && (status.quotaFreshness ?? status.dataStatus.freshness) === "live",
  };
}
