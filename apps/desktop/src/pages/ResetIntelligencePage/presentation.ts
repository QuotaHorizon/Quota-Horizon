import type { DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { planHasNoShortQuotaWindow } from "../../utils/accountUsageWindows";
import type { PublicEvidenceDisposition, PublicResetArchiveEntry, PublicSourceId, PublicSourceStatus } from "./types";

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

export const SOURCE_NAMES: Record<PublicSourceId, string> = {
  quotaresets: "QuotaResets", codex_reset: "Codex Reset", codex_reset_posts: "Codex Reset · Tibo", openai_status: "OpenAI Status",
};

export function publicSourceLabel(source: PublicSourceStatus, language: Language, now: number) {
  const zh = language === "zh";
  if (source.issue) {
    const reasons: Record<string, [string, string]> = {
      request_failed: ["连接失败", "Connection failed"], http_error: ["来源暂不可用", "Source unavailable"],
      schema_changed: ["来源格式已变化", "Source format changed"], invalid_response: ["资料未通过校验", "Invalid source data"],
      response_too_large: ["资料超出读取上限", "Source exceeds read limit"],
      upstream_stale: ["来源缓存尚未更新", "Upstream cache is stale"],
    };
    return (reasons[source.issue] ?? ["暂不可用", "Unavailable"])[zh ? 0 : 1];
  }
  if (!source.lastSuccessAt) return zh ? "尚未读取" : "Not fetched yet";
  const age = now - Date.parse(source.lastSuccessAt);
  if (!Number.isFinite(age) || age < 0) return zh ? "需核对时间" : "Check timestamp";
  if (age >= 15 * 60_000) return zh ? "等待更新" : "Update due";
  return source.rejectedRecords > 0 ? (zh ? "部分资料可用" : "Partially available") : (zh ? "更新成功" : "Updated");
}
