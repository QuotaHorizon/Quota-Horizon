import type { DesktopStatusEnvelope, DesktopVaultMutationStatusEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";

const STATES: Record<string, readonly [string, string]> = {
  available: ["可用", "Available"], complete: ["完整", "Complete"], partial: ["部分可用", "Partial"],
  unavailable: ["暂不可用", "Unavailable"], failed: ["检查失败", "Check failed"],
  stable: ["已按账户保存", "Saved per account"], ephemeral: ["暂未保存新历史", "New history is not saved"],
  persistent: ["已开启", "Enabled"], unsupported: ["暂不支持", "Unsupported"],
  not_applicable: ["不适用", "Not applicable"], not_running: ["未运行", "Not running"],
  running: ["正在运行", "Running"], unlocked: ["无操作占用", "Not in use"],
  active: ["有操作占用", "In use"], stale: ["上次操作待检查", "Previous operation needs checking"],
  unverifiable: ["无法确认", "Cannot verify"], tested: ["已验证", "Verified"],
  expected_compatible: ["预计兼容，尚未验证", "Expected compatible; not verified"],
  not_tested: ["尚未验证", "Not verified"], known_broken: ["当前版本存在兼容问题", "Known compatibility issue"],
};

export function capacityStateLabel(value: string | null | undefined, language: Language) {
  return STATES[value ?? ""]?.[language === "zh" ? 0 : 1] ?? (language === "zh" ? "未知" : "Unknown");
}

export function capacityPlanLabel(value: string | null | undefined, language: Language) {
  const plans: Record<string, string> = { free: "Free", plus: "Plus", pro: "Pro", prolite: "Pro Lite", pro_lite: "Pro Lite",
    team: "Team", business: "Business", enterprise: "Enterprise", edu: "Edu" };
  return plans[value?.toLowerCase() ?? ""] ?? (language === "zh" ? "其他套餐" : "Other plan");
}

export function capacityAuthLabel(value: string | null | undefined, language: Language) {
  if (["chatgpt", "chatgpt_auth_tokens"].includes(value ?? "")) return language === "zh" ? "ChatGPT 账号" : "ChatGPT account";
  if (["apiKey", "api_key", "apikey"].includes(value ?? "")) return "API Key";
  return language === "zh" ? "尚未确认" : "Not confirmed";
}

const ISSUES: Record<string, readonly [string, string]> = {
  authentication_required: ["需要在 Codex 中确认登录状态，暂时无法更新额度。", "Check your sign-in status in Codex. Quota cannot be updated yet."],
  codex_selection_required: ["请先选择要读取额度的 Codex 安装。", "Choose the Codex installation to read quota from."],
  codex_not_found: ["未找到可用的 Codex，请检查 Codex 是否已安装。", "No supported Codex installation was found."],
  codex_verification_failed: ["无法验证 Codex 安装，请检查安装是否完整。", "The Codex installation could not be verified."],
  codex_candidate_invalid: ["此前选择的 Codex 已不可用，请重新选择。", "The selected Codex installation is unavailable. Select it again."],
  codex_candidate_changed: ["Codex 安装已更新，请重新确认读取来源。", "Codex has changed. Confirm the installation again."],
  app_server_timeout: ["读取 Codex 额度超时，将自动重试。", "Reading Codex quota timed out. Retrying automatically."],
  app_server_process_failed: ["本次额度读取未完成，将自动重试。", "This quota read did not finish. Retrying automatically."],
  app_server_protocol_error: ["当前 Codex 返回了暂不支持的数据格式，不能据此更新额度。", "Codex returned an unsupported data format. Quota cannot be updated from it."],
  account_changed_during_read: ["账号在刷新期间发生变化，正在等待新账号的额度。", "The account changed during refresh. Waiting for its quota."],
  account_boundary_invalid: ["无法确认当前账号，已停止使用上一个账号的数据。", "The current account could not be verified. Previous-account data is no longer used."],
  account_binding_failed: ["本地历史暂未保存，实时额度仍可用。", "Local history is not being saved. Live quota is still available."],
  refresh_backoff: ["连续读取失败，正在短暂等待后自动重试。", "Reads failed repeatedly. Retrying after a short delay."],
};

export function capacityIssueMessage(code: string | undefined, language: Language) {
  return ISSUES[code ?? ""]?.[language === "zh" ? 0 : 1]
    ?? (language === "zh" ? "本次读取未完成，请稍后重试。" : "This read did not finish. Try again later.");
}

/** A completed request is not necessarily a successful live quota read. */
export function capacityRefreshFeedback(envelope: DesktopStatusEnvelope, language: Language): {
  kind: "success" | "warning"; message: string;
} {
  const status = envelope.status;
  const live = status?.quotaWindows.length && status.quotaSource !== "managed_account_cache"
    && (status.quotaFreshness ?? status.dataStatus.freshness) === "live"
    && !envelope.issue && envelope.lifecycle === "ready";
  if (!live) return { kind: "warning", message: capacityQuotaNotice(envelope, false, language)?.message
    ?? capacityIssueMessage(envelope.issue?.code, language) };
  const at = new Date(status.quotaObservedAt ?? status.capturedAt);
  const time = Number.isFinite(at.getTime()) ? new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-US", {
    hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23",
  }).format(at) : "";
  return { kind: "success", message: language === "zh"
    ? `额度已更新${time ? ` · ${time}` : ""}`
    : `Quota updated${time ? ` · ${time}` : ""}` };
}

/** User-facing quota notices never fall back to backend messages or reason codes. */
export function capacityQuotaNotice(envelope: DesktopStatusEnvelope, refreshing: boolean, language: Language): {
  kind: "info" | "warning";
  message: string;
} | null {
  const zh = language === "zh";
  const status = envelope.status;
  const hasQuota = Boolean(status?.quotaWindows.length);
  const cached = hasQuota && (status?.quotaSource === "managed_account_cache"
    || (status?.quotaFreshness ?? status?.dataStatus.freshness) === "stale");
  const warning = (message: string) => ({ kind: "warning" as const,
    message: cached ? `${message} ${zh ? "当前显示上次保存的额度。" : "Showing the last saved quota."}` : message });

  // A known failure must not become a reassuring loading notice during retry.
  if (envelope.issue) return warning(capacityIssueMessage(envelope.issue.code, language));
  const issueCode = status?.dataStatus.reasonCodes.find((code) => ISSUES[code]?.[0]);
  if (issueCode) return warning(capacityIssueMessage(issueCode, language));
  if (status?.dataStatus.compatibility === "known_broken" || status?.dataStatus.compatibility === "unsupported") {
    return warning(capacityIssueMessage("app_server_protocol_error", language));
  }
  if (status?.dataStatus.availability === "failed" || envelope.lifecycle === "error") {
    return warning(zh ? "额度刷新失败，请稍后重试。" : "Quota refresh failed. Try again later.");
  }
  if (status?.dataStatus.availability === "unsupported") {
    return warning(zh ? "暂不支持读取当前额度，请在仪表板查看详情。" : "Current quota reads are unsupported. See the dashboard for details.");
  }
  if (cached) return { kind: "info", message: refreshing
    ? (zh ? "正在更新，暂时显示上次额度。" : "Updating; showing the last saved quota.")
    : (zh ? "显示上次保存的额度。" : "Showing the last saved quota.") };
  if (status?.dataStatus.availability === "partial") {
    if (status.dataStatus.reasonCodes.includes("reset_credit_summary_unavailable")) {
      return { kind: "info", message: zh ? "额度已读取；当前读取来源未提供重置卡数据。"
        : "Quota was read; the current reader did not supply reset-card data." };
    }
    return warning(zh ? "部分信息暂未取得，请稍后重试。" : "Some information is not available yet. Try again later.");
  }
  return null;
}

export function safeSwitchPresentation(vault: DesktopVaultMutationStatusEnvelope | null, language: Language) {
  const zh = language === "zh";
  const item = (kind: "success" | "info" | "warning", title: string, description: string) => ({ kind, title, description });
  if (!vault) return item("warning", zh ? "切换保护状态暂不可用" : "Switch protection status unavailable",
    zh ? "额度监控不受影响；执行账号切换时会重新检查。" : "Quota monitoring is unaffected. Switching will run a fresh check.");
  switch (vault.reasonCode) {
    case "legacy_viewer_running":
      return item("info", zh ? "QuotaViewer 正在运行" : "QuotaViewer is running",
        zh ? "可同时查看额度；切换账号或修改 Codex 配置前，请先退出 QuotaViewer。Horizon 不会自动关闭它。"
          : "Both apps can monitor quota. Quit QuotaViewer before switching accounts or changing Codex settings. Horizon will not close it automatically.");
    case "mutation_lock_active":
      return item("info", zh ? "另一项本地安全操作正在进行" : "Another local protected operation is in progress",
        zh ? "该操作结束后可再次尝试切换账号，额度刷新不受影响。" : "Retry switching when it finishes. Quota refresh is unaffected.");
    case "mutation_lock_stale":
    case "mutation_lock_unverifiable":
    case "vault_review_required":
    case "vault_recovery_required":
      return item("warning", zh ? "上次账户操作需要检查" : "A previous account operation needs checking",
        zh ? "切换前需核查上次操作的恢复状态；请勿手动删除锁或账号文件。额度监控不受影响。"
          : "Check recovery of the previous operation before switching. Do not delete lock or account files manually. Quota monitoring is unaffected.");
    case "legacy_viewer_state_unavailable":
      return item("warning", zh ? "无法确认 QuotaViewer 是否已退出" : "Cannot confirm whether QuotaViewer has stopped",
        zh ? "切换暂不可用，稍后会重新检查；额度监控不受影响。" : "Switching is temporarily unavailable. The check will retry; quota monitoring is unaffected.");
    case "mutation_status_available":
      if (vault.status === "available") return item("success", zh ? "切换保护检查通过" : "Switch protection check passed",
        zh ? "实际切换账号前仍会再次检查，并由你确认。" : "Switching will still require a fresh check and your confirmation.");
  }
  return item("warning", zh ? "账户操作状态暂不可用" : "Account operation status unavailable",
    zh ? "无法完成切换前检查，请稍后重试。额度监控不受影响。" : "The pre-switch check could not finish. Try again later. Quota monitoring is unaffected.");
}
