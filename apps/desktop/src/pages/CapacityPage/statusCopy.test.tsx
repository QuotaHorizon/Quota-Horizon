import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { DesktopVaultMutationStatusEnvelope } from "../../../../capacity-preview/src/status";
import { CapacityDetails } from "./CapacityDetails";
import { CapacityHistory } from "./CapacityHistory";
import { capacityAuthLabel, capacityIssueMessage, capacityPlanLabel, capacityStateLabel, safeSwitchPresentation } from "./statusCopy";

function vault(reasonCode: string, status: DesktopVaultMutationStatusEnvelope["status"] = "blocked"): DesktopVaultMutationStatusEnvelope {
  return { schemaVersion: "1.0", status, reasonCode,
    coordination: { lockState: "active", legacyViewerState: "running" },
    catalog: { managedAccountCount: 0, pendingAccountOperationCount: 0, pendingKeyRotationCount: 0, needsReviewCount: 0 },
    mutationCommandsEnabled: false, identifiersRedacted: true, pathsRedacted: true };
}

describe("quota page uses product language, not machine statuses", () => {
  it.each(["expected_compatible", "not_tested", "not_applicable", "not_running", "known_broken", "ephemeral", "new_internal_state"]) (
    "does not expose %s as a user label", (state) => {
      for (const language of ["zh", "en"] as const) {
        const label = capacityStateLabel(state, language);
        expect(label).not.toContain("_");
        expect(label).not.toBe(state);
      }
    },
  );

  it("normalizes known account names without inventing an unknown plan", () => {
    expect(capacityPlanLabel("prolite", "zh")).toBe("Pro Lite");
    expect(capacityPlanLabel("pro_lite", "en")).toBe("Pro Lite");
    expect(capacityPlanLabel("future_internal_tier", "zh")).toBe("其他套餐");
    expect(capacityAuthLabel("chatgpt_auth_tokens", "zh")).toBe("ChatGPT 账号");
    expect(capacityIssueMessage("app_server_timeout", "zh")).toContain("超时");
    expect(capacityIssueMessage("future_internal_failure", "zh")).not.toContain("_");
  });

  it("explains the verified legacy-app guard without calling quota monitoring broken", () => {
    const view = safeSwitchPresentation(vault("legacy_viewer_running"), "zh");
    expect(view.kind).toBe("info");
    expect(view.title).toBe("QuotaViewer 正在运行");
    expect(view.description).toContain("可同时查看额度");
    expect(view.description).toContain("请先退出 QuotaViewer");
    expect(view.description).toContain("不会自动关闭");
  });

  it("keeps real recovery and unknown failures visible instead of treating them as ready", () => {
    for (const reason of ["mutation_lock_stale", "mutation_lock_unverifiable", "vault_review_required", "vault_recovery_required", "vault_store_unavailable", "new_internal_failure"]) {
      const view = safeSwitchPresentation(vault(reason, "recovery_required"), "zh");
      expect(view.kind).toBe("warning");
      expect(view.title + view.description).not.toContain("_");
      expect(view.description).toContain("不受影响");
    }
    expect(safeSwitchPresentation(vault("mutation_status_available", "available"), "zh").description).toContain("由你确认");
    expect(safeSwitchPresentation(vault("mutation_status_available", "failed"), "zh").kind).toBe("warning");
    expect(safeSwitchPresentation(null, "en").kind).toBe("warning");
  });

  it("starts diagnostics collapsed and reserves raw reason codes for a second explicit technical disclosure", () => {
    const markup = renderToStaticMarkup(<CapacityDetails envelope={null} diagnostics={null}
      vault={vault("legacy_viewer_running")} language="zh" onInspect={() => undefined} />);
    expect(markup).toContain("运行状态与问题排查");
    expect(markup).toContain("技术详情（仅用于排查）");
    expect(markup).not.toMatch(/<details[^>]*\bopen(?:[ =>])/);
    expect(markup).toContain("<code>legacy_viewer_running</code>");
    const labels = markup.replace(/<code>[\s\S]*?<\/code>/g, "").replace(/<[^>]+>/g, "");
    expect(labels).not.toContain("_");
    for (const internal of ["blocked", "容量服务", "安全切换就绪度", "托管账户", "wpr4a"]) expect(labels).not.toContain(internal);
  });

  it("explains history authorization without printing its internal error code", () => {
    const markup = renderToStaticMarkup(<CapacityHistory language="zh" history={{
      schemaVersion: "1.0", status: "binding_required", reasonCode: "history_keychain_denied", points: [],
    }} onAuthorize={() => undefined} />);
    expect(markup).toContain("授权后可查看旧历史并继续记录额度");
    expect(markup).toContain("授权本地历史密钥");
    expect(markup).not.toContain("history_keychain_denied");
  });
});
