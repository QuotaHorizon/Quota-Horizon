import { describe, expect, it } from "vitest";
import type { DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import { capacityIssueMessage, capacityQuotaNotice, capacityRefreshFeedback } from "./statusCopy";

function snapshot(cached = true): DesktopStatusEnvelope {
  return {
    schemaVersion: "1.0", sequence: 1, lifecycle: cached ? "stale" : "ready",
    selectedExecutableId: null, candidates: [], issue: null, persistenceEnabled: false,
    status: {
      schemaVersion: "1.0", capturedAt: "2026-09-08T01:00:00Z", codexVersion: null,
      account: { authMode: "chatgpt", planType: "prolite", bindingStatus: "ephemeral" },
      dataStatus: { availability: cached ? "partial" : "complete", freshness: cached ? "stale" : "live",
        compatibility: "not_tested", reasonCodes: cached ? ["managed_quota_only"] : [] },
      quotaSource: cached ? "managed_account_cache" : undefined,
      quotaWindows: [{ limitId: "codex:primary", label: null, remainingPercent: 0, usedPercent: 100,
        windowMinutes: 10_080, resetsAt: "2026-09-14T05:28:00Z" }],
      resetCredits: { summaryStatus: "unavailable", availableCount: null, detailsStatus: "unavailable" },
      usage: { availability: "unsupported", hasSummary: false, reasonCodes: [] }, diagnosticCodes: [],
    },
  };
}

describe("quota notice state and product language", () => {
  it("confirms completed live reads without fabricating a quota change", () => {
    const data = snapshot(false);
    expect(capacityRefreshFeedback(data, "zh")).toMatchObject({ kind: "success" });
    expect(capacityRefreshFeedback(data, "zh").message).toContain("额度已更新");
    expect(data.status!.quotaWindows[0].remainingPercent).toBe(0);
  });
  it("never reports a cached or failed refresh as success", () => {
    const data = snapshot();
    expect(capacityRefreshFeedback(data, "zh").kind).toBe("warning");
    data.status!.quotaFreshness = "live";
    data.lifecycle = "ready";
    expect(capacityRefreshFeedback(data, "zh").kind).toBe("warning");
    const failed = snapshot(false);
    failed.issue = { code: "app_server_timeout", message: "private_raw", retryAfterMs: null };
    expect(capacityRefreshFeedback(failed, "zh").message).toContain("超时");
    expect(capacityRefreshFeedback(failed, "zh").message).not.toContain("private_raw");
  });
  it("explains a cache-only bootstrap as information, not a failed refresh", () => {
    const envelope = snapshot();
    const notice = capacityQuotaNotice(envelope, true, "zh");
    expect(notice).toEqual({ kind: "info", message: "正在更新，暂时显示上次额度。" });
    expect(envelope.status?.quotaWindows[0].remainingPercent).toBe(0);
    expect(envelope.status?.dataStatus.reasonCodes).toEqual(["managed_quota_only"]);
  });

  it("does not claim a request is in flight just because saved data is displayed", () => {
    expect(capacityQuotaNotice(snapshot(), false, "zh"))
      .toEqual({ kind: "info", message: "显示上次保存的额度。" });
    expect(capacityQuotaNotice(snapshot(), true, "en")?.message).toContain("Updating");
    expect(capacityQuotaNotice(snapshot(), false, "en")?.message).not.toContain("Updating");
  });

  it("keeps a timeout visible during retry, including retained-quota context", () => {
    const envelope = snapshot();
    envelope.issue = { code: "app_server_timeout", message: "RAW_INTERNAL_MESSAGE", retryAfterMs: null };
    for (const language of ["zh", "en"] as const) {
      const notice = capacityQuotaNotice(envelope, true, language);
      expect(notice?.kind).toBe("warning");
      expect(notice?.message).toContain(capacityIssueMessage("app_server_timeout", language));
      expect(notice?.message).toContain(language === "zh" ? "上次保存的额度" : "last saved quota");
      expect(notice?.message).not.toContain("_");
    }
  });

  it("shows issues even when the previous status was complete and live", () => {
    const envelope = snapshot(false);
    envelope.issue = { code: "new_internal_failure", message: "raw internal details", retryAfterMs: null };
    expect(capacityQuotaNotice(envelope, false, "en")).toEqual({ kind: "warning",
      message: capacityIssueMessage("new_internal_failure", "en") });
  });

  it("handles an empty error state without claiming a saved quota exists", () => {
    const envelope = snapshot();
    envelope.status = null;
    envelope.issue = { code: "app_server_timeout", message: "raw_message", retryAfterMs: null };
    expect(capacityQuotaNotice(envelope, false, "zh")?.message)
      .toBe(capacityIssueMessage("app_server_timeout", "zh"));
  });

  it("finds actionable reasons after the cache marker without demanding an unproven login", () => {
    const envelope = snapshot();
    envelope.status!.dataStatus.reasonCodes.push("authentication_required");
    expect(capacityQuotaNotice(envelope, false, "zh")?.message).toContain("确认登录状态");
    envelope.status!.dataStatus.reasonCodes = ["managed_quota_only"];
    expect(capacityQuotaNotice(envelope, false, "zh")?.message).not.toContain("登录");
  });

  it.each(["failed", "unsupported"] as const)("does not downgrade %s data to a cache-loading notice", (availability) => {
    const envelope = snapshot();
    envelope.status!.dataStatus.availability = availability;
    expect(capacityQuotaNotice(envelope, true, "zh")?.kind).toBe("warning");
  });

  it.each(["known_broken", "unsupported"] as const)("keeps %s compatibility visible", (compatibility) => {
    const envelope = snapshot();
    envelope.status!.dataStatus.compatibility = compatibility;
    expect(capacityQuotaNotice(envelope, true, "zh")?.message).toContain("数据格式");
  });

  it("respects quota-specific freshness instead of assuming all fields share it", () => {
    const envelope = snapshot(false);
    envelope.status!.quotaFreshness = "stale";
    expect(capacityQuotaNotice(envelope, false, "zh")?.kind).toBe("info");
    envelope.status!.quotaFreshness = "live";
    envelope.status!.dataStatus.freshness = "stale";
    expect(capacityQuotaNotice(envelope, false, "zh")).toBeNull();
  });

  it("describes incomplete data even with empty or unknown reason codes", () => {
    const envelope = snapshot(false);
    envelope.status!.dataStatus.availability = "partial";
    for (const reasons of [[], ["future_internal_reason"]]) {
      envelope.status!.dataStatus.reasonCodes = reasons;
      for (const language of ["zh", "en"] as const) {
        const notice = capacityQuotaNotice(envelope, false, language);
        expect(notice?.kind).toBe("warning");
        expect(notice?.message).not.toContain("_");
        expect(notice?.message.length).toBeGreaterThan(0);
      }
    }
  });

  it("removes the cached notice after a complete live result, without changing observations", () => {
    expect(capacityQuotaNotice(snapshot(), false, "zh")).not.toBeNull();
    expect(capacityQuotaNotice(snapshot(false), false, "zh")).toBeNull();
    const envelope = snapshot();
    envelope.status = null;
    envelope.lifecycle = "idle";
    expect(capacityQuotaNotice(envelope, true, "zh")).toBeNull();
    envelope.lifecycle = "error";
    expect(capacityQuotaNotice(envelope, false, "zh")?.kind).toBe("warning");
  });
});
