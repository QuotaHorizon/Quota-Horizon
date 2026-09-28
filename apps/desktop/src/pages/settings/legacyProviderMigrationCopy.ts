import type { Language } from "../../i18n";

export const providerMigrationCopy = {
  zh: {
    title: "API 账号与连接", review: "审阅 API 连接", reviewAgain: "重新审阅",
    description: "将 Viewer 保存的 API 账号逐项移入“连接”。只新增未启用的连接，不覆盖已有连接，不试连，不改变 Codex 的登录或网络。",
    safety: "密钥由本机系统凭据存储保护，不会显示在预览中。请先退出 QuotaViewer，再输入 IMPORT API 确认选中的连接；预览两分钟内有效。",
    import: "可导入", keep_existing: "保留现有连接", reviewOnly: "需要复核",
    select: "选择导入", confirm: "确认导入此连接", empty: "这个目录中没有需要迁移的 API 账号。",
    cancel: "取消选择", selected: "待导入连接", endpoint: "地址", model: "默认模型",
    protocol: "协议", reasoning: "推理强度", context: "上下文",
    connectionScope: "仅迁移连接与模型设置；不带入旧的代理运行状态、模型缓存或其他全局 Codex 设置。",
    busy: "正在处理连接迁移…", failed: "连接迁移未完成",
    committed: "连接导入已完成", rolledBack: "本次移入的连接已撤销",
    unfinished: "上次连接迁移尚未收尾，请先尝试撤销。若文件已被修改，将保留当前版本并提示复核。",
    rollback: "撤销最近一次连接导入", rollbackHint: "只撤销下方这个连接。若它正在使用或导入后已被修改，会拒绝撤销；旧 Viewer 数据与当前登录不会被改动。",
    rollbackAction: "确认撤销该连接", notReversible: "此连接当前不可撤销。请先确认它未启用且导入后未发生修改。",
    futureReason: "该连接包含当前无法完整迁移的设置，请在连接页面复核后手动添加。",
  },
  en: {
    title: "API accounts and connections", review: "Review API connections", reviewAgain: "Review again",
    description: "Move saved Viewer API accounts into Connections one at a time. Add inactive connections only; never replace existing connections, test endpoints, or change the Codex login or network.",
    safety: "Keys are protected by this device’s credential store and never shown in the preview. Quit QuotaViewer, then enter IMPORT API to confirm the selected connection. The preview expires in two minutes.",
    import: "Ready to import", keep_existing: "Keep existing", reviewOnly: "Needs review",
    select: "Select for import", confirm: "Import this connection", empty: "This directory has no API accounts to migrate.",
    cancel: "Cancel selection", selected: "Selected connection", endpoint: "Endpoint", model: "Default model",
    protocol: "Protocol", reasoning: "Reasoning", context: "Context",
    connectionScope: "Imports connection and model settings only, not the old running proxy state, model cache, or other global Codex settings.",
    busy: "Processing connection migration…", failed: "Connection migration did not finish",
    committed: "Connection import completed", rolledBack: "Imported connection removed",
    unfinished: "The previous connection import has not finished. Try undoing it first. Changed files are preserved for review.",
    rollback: "Undo the last connection import", rollbackHint: "Only removes the connection below. Undo is refused if it is active or has changed since import. Viewer data and the current login are untouched.",
    rollbackAction: "Undo this connection", notReversible: "Undo is currently unavailable. Check that the connection is inactive and has not changed since import.",
    futureReason: "Some settings cannot be migrated completely. Review and add this connection manually on Connections.",
  },
} as const;

const reasons: Record<string, [string, string]> = {
  legacy_provider_existing: ["该连接已导入，或已存在同名、同地址、同模型的连接。保留 Horizon 当前版本。", "Already imported, or a connection has the same name, endpoint, and model. Keep Horizon’s current version."],
  legacy_provider_auth_unsupported: ["无法识别这个账号的认证方式。", "This account’s authentication method is not recognized."],
  legacy_provider_auth_invalid: ["账号认证信息无法读取。", "The account’s authentication information is unreadable."],
  legacy_provider_mixed_auth: ["同时存在多种认证信息，需要先确认应使用哪一种。", "Multiple authentication methods need review before import."],
  legacy_provider_key_missing: ["没有可用的 API 密钥。", "No usable API key is available."],
  legacy_provider_config_missing: ["没有保存连接配置，无法确认地址和模型。", "No saved connection configuration; endpoint and model cannot be verified."],
  legacy_provider_config_invalid: ["保存的连接配置格式有误。", "The saved connection configuration is invalid."],
  legacy_provider_model_missing: ["没有明确的默认模型，不会代你猜测。", "No explicit default model; one will not be guessed."],
  legacy_provider_endpoint_missing: ["缺少所选连接的服务地址。", "The selected connection has no service endpoint."],
  legacy_provider_endpoint_invalid: ["地址无效或包含额外认证信息，需要手动复核。", "The endpoint is invalid or contains extra authentication information; review it manually."],
  legacy_provider_custom_auth_review: ["包含额外请求头、环境变量或认证规则，不能直接迁移。", "Extra headers, environment variables, or authentication rules require manual review."],
  legacy_provider_transport_review: ["包含无法对应的传输设置，已保留原配置，未导入。", "Unmapped transport settings; the original is preserved and not imported."],
  legacy_provider_reasoning_review: ["模型的推理强度设置需要复核。", "The model’s reasoning setting needs review."],
  legacy_provider_context_invalid: ["模型上下文大小设置无效。", "The model’s context window setting is invalid."],
  legacy_provider_name_invalid: ["连接名称无效，需要先修正。", "The connection name needs correction."],
  legacy_provider_profile_invalid: ["连接与 Horizon 当前支持的设置不兼容。", "This connection is incompatible with the supported settings."],
  legacy_provider_preview_limit: ["待确认的连接过多，请稍后重新审阅。", "Too many pending confirmations; review again shortly."],
};

export function providerImportReason(code: string | null, language: Language): string | null {
  if (!code) return null;
  return reasons[code]?.[language === "zh" ? 0 : 1] ?? providerMigrationCopy[language].futureReason;
}

export function providerImportReasoning(value: string, language: Language): string {
  const labels: Record<string, [string, string]> = {
    none: ["关闭", "None"], low: ["低", "Low"], medium: ["中", "Medium"], high: ["高", "High"],
    xhigh: ["极高", "Extra high"], max: ["最大", "Maximum"], ultra: ["超高", "Ultra"],
  };
  return labels[value]?.[language === "zh" ? 0 : 1] ?? (language === "zh" ? "需要复核" : "Needs review");
}
