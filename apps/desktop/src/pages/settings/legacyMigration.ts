import type {
  LegacyMigrationArtifactKind,
  LegacyMigrationArtifactState,
  LegacyAccountMigrationDisposition,
  LegacyMigrationConflictKind,
  LegacyMigrationDisposition,
  LegacyMigrationDryRunReport,
  LegacyMigrationStatus,
  LegacyMigrationTarget,
} from "../../types";
import type { Language } from "../../i18n";

export const legacyMigrationCopy = {
  zh: {
    title: "迁移 QuotaViewer",
    description: "检查当前 Viewer 数据或备份，列出可直接迁移、需复核和被阻止的内容。检查全程只读。",
    inspectDefault: "检查当前 Viewer",
    inspecting: "正在只读检查 Viewer 数据…",
    chooseBackup: "选择备份目录",
    modalTitle: "QuotaViewer 迁移预检",
    close: "关闭",
    inspectAgain: "重新检查",
    defaultSource: "系统默认 Viewer 目录",
    selectedSource: "所选备份：{name}",
    readOnly: "源数据只读",
    schema: "预检契约 {version}",
    format: "识别格式",
    inventory: "检测到的数据",
    plan: "迁移计划",
    conflicts: "需要处理的冲突",
    noConflicts: "没有检测到结构冲突",
    sourceSize: "源数据 {source}；正式导入前至少预留 {minimum}",
    ready: "可以进入安全导入",
    partial: "部分内容需要复核或修复",
    unsupported: "没有识别到受支持的 Viewer 数据",
    failed: "无法安全检查该目录",
    next: "预检本身只读。设置、ChatGPT 账号与 API 连接分别审阅并确认导入，不覆盖现有登录或连接。API 导入入口在下方；旧额度缓存、代理运行状态与旧恢复点仍保留在 Viewer，需另行复核。",
    prepareImport: "准备安全导入",
    preparingImport: "正在固定导入计划…",
    importing: "正在建立恢复点、导入并逐项校验…",
    rollingBack: "正在恢复导入前状态并逐项校验…",
    operationFailed: "Viewer 迁移操作未完成",
    confirmationTitle: "确认导入这些设置",
    confirmationHint: "输入 IMPORT 后执行。确认有效期 120 秒；旧版 Viewer 必须已完全退出。",
    confirmImport: "备份并导入",
    alreadyApplied: "这些设置已经与 QuotaViewer 一致，无需再次写入。",
    changedTargets: "将更新",
    reviewExcluded: "本次不会导入",
    importSummary: "导入结果预览",
    refreshMode: "后台刷新",
    launchMode: "登录启动",
    languageMode: "界面语言",
    workPlanMode: "Work Plan",
    statusPolicy: "状态栏",
    dynamicStatus: "采用 QuotaHorizon 动态额度页眉",
    enabled: "开启",
    disabled: "关闭",
    minutes: "每 {minutes} 分钟",
    manual: "仅手动",
    offPeriods: "{state} · {count} 个停用时段",
    importApplied: "Viewer 设置已安全导入并完成校验。",
    rollbackAvailable: "已保留目标侧恢复点，可撤销本次导入。",
    rollbackTitle: "撤销最近一次 Viewer 导入",
    rollbackHint: "输入 ROLLBACK 后恢复导入前状态；如果之后又修改过这些设置，系统会拒绝覆盖。",
    confirmRollback: "撤销导入",
    rolledBack: "已恢复导入前状态。",
    recoveryRequired: "检测到未收口的迁移状态，需要先完成安全恢复。",
    filesRecords: "{files} 个文件 · {records} 条记录 · {size}",
    sourceCount: "源 {source}",
    targetCount: "预计 {target}",
    accountTitle: "ChatGPT 账号迁移与去重",
    accountDescription: "逐个验证 Viewer 登录并与 QuotaHorizon 现有账号对照。当前登录不会被切换，旧额度缓存不会被复制；API 账号请使用下方的连接导入。",
    reviewAccounts: "复核账号",
    reviewAccountsAgain: "重新复核",
    preparingAccounts: "正在验证账号格式并对照现有账号…",
    importingAccounts: "正在建立恢复点、导入缺少账号并逐项校验…",
    rollingBackAccounts: "正在撤销账号导入并校验…",
    accountImportCount: "将导入",
    accountExistingCount: "已存在",
    accountConflictCount: "保留现有",
    accountUnsupportedCount: "需另行处理",
    accountPreferred: "Viewer 首选",
    accountsAlreadyApplied: "Viewer 中的兼容账号已存在，无需写入。",
    accountsReviewOnly: "没有可自动补入的账号；现有冲突或非 ChatGPT 账号保持不变。",
    accountSafetySummary: "只会补入缺少的账号",
    accountSafetyDetail: "同一身份已存在但凭据不同的账号一律保留 QuotaHorizon 当前副本；导入不切换 Codex 登录。完成后请到“账号”页执行 Refresh All 获取实时额度。",
    accountConfirmationHint: "输入 IMPORT ACCOUNTS 后执行。确认有效期 120 秒；Viewer 必须已完全退出。",
    confirmAccountImport: "备份并导入账号",
    accountsImported: "已安全导入 {count} 个缺少账号。",
    accountCurrentLoginPreserved: "Codex 当前登录保持不变。",
    accountRollbackAvailable: "已保留目标侧恢复点；刷新或编辑导入账号后，为防止丢失新数据，撤销会拒绝覆盖。",
    accountRollbackTitle: "撤销最近一次账号导入",
    accountRollbackHint: "输入 ROLLBACK ACCOUNTS 后，仅移除本次创建且从未改变的账号；Viewer 源数据始终保留。",
    confirmAccountRollback: "撤销账号导入",
    accountsRolledBack: "本次创建的账号已移除。",
    accountRecoveryRequired: "检测到未收口或已变化的账号迁移；请先保留当前数据并完成恢复复核。",
  },
  en: {
    title: "Migrate QuotaViewer",
    description: "Inspect current Viewer data or a backup and classify what can import, needs review, or is blocked. Inspection is read-only.",
    inspectDefault: "Inspect current Viewer",
    inspecting: "Inspecting Viewer data without modifying it…",
    chooseBackup: "Choose backup folder",
    modalTitle: "QuotaViewer migration preflight",
    close: "Close",
    inspectAgain: "Inspect again",
    defaultSource: "Default Viewer folder",
    selectedSource: "Selected backup: {name}",
    readOnly: "Source stays read-only",
    schema: "Preflight contract {version}",
    format: "Detected format",
    inventory: "Detected data",
    plan: "Migration plan",
    conflicts: "Conflicts to resolve",
    noConflicts: "No structural conflicts detected",
    sourceSize: "Source data {source}; reserve at least {minimum} before import",
    ready: "Ready for safe import",
    partial: "Some content needs review or repair",
    unsupported: "No supported Viewer data was detected",
    failed: "This folder could not be inspected safely",
    next: "Preflight is read-only. Review and confirm settings, ChatGPT accounts, and API connections separately; existing logins and connections are preserved. The API import panel is below. Old quota cache, running proxy state, and old restore points remain in Viewer for separate review.",
    prepareImport: "Prepare safe import",
    preparingImport: "Freezing the import plan…",
    importing: "Creating a restore point, importing, and verifying…",
    rollingBack: "Restoring and verifying the pre-import state…",
    operationFailed: "Viewer migration operation did not complete",
    confirmationTitle: "Confirm these settings",
    confirmationHint: "Type IMPORT to continue. Confirmation lasts 120 seconds and the legacy Viewer must be fully quit.",
    confirmImport: "Back up and import",
    alreadyApplied: "These settings already match QuotaViewer; nothing needs to be written.",
    changedTargets: "Will update",
    reviewExcluded: "Not imported now",
    importSummary: "Imported settings preview",
    refreshMode: "Background refresh",
    launchMode: "Launch at login",
    languageMode: "Interface language",
    workPlanMode: "Work Plan",
    statusPolicy: "Menu bar",
    dynamicStatus: "Use QuotaHorizon's dynamic capacity header",
    enabled: "On",
    disabled: "Off",
    minutes: "Every {minutes} minutes",
    manual: "Manual only",
    offPeriods: "{state} · {count} off periods",
    importApplied: "Viewer settings were imported and verified safely.",
    rollbackAvailable: "A target-side restore point is available for this import.",
    rollbackTitle: "Undo the latest Viewer import",
    rollbackHint: "Type ROLLBACK to restore the pre-import state. If these settings changed later, rollback refuses to overwrite them.",
    confirmRollback: "Undo import",
    rolledBack: "The pre-import state has been restored.",
    recoveryRequired: "An unfinished migration needs safe recovery before another import.",
    filesRecords: "{files} files · {records} records · {size}",
    sourceCount: "Source {source}",
    targetCount: "Expected {target}",
    accountTitle: "ChatGPT account migration and deduplication",
    accountDescription: "Validate each Viewer login against QuotaHorizon's existing accounts. The current login is never switched and old quota cache is not copied. Use the connection panel below for API accounts.",
    reviewAccounts: "Review accounts",
    reviewAccountsAgain: "Review again",
    preparingAccounts: "Validating account formats and comparing existing accounts…",
    importingAccounts: "Creating a restore point, importing missing accounts, and verifying…",
    rollingBackAccounts: "Undoing the account import and verifying…",
    accountImportCount: "Import",
    accountExistingCount: "Present",
    accountConflictCount: "Keep existing",
    accountUnsupportedCount: "Manual review",
    accountPreferred: "Viewer preferred",
    accountsAlreadyApplied: "Compatible Viewer accounts are already present; no write is needed.",
    accountsReviewOnly: "There are no accounts to add automatically; conflicts and non-ChatGPT accounts remain unchanged.",
    accountSafetySummary: "Only missing accounts will be added",
    accountSafetyDetail: "If the same identity exists with different credentials, QuotaHorizon's current copy always wins. The import does not switch the Codex login. Use Refresh All on Accounts afterward to fetch live quota.",
    accountConfirmationHint: "Type IMPORT ACCOUNTS to continue. Confirmation lasts 120 seconds and Viewer must be fully quit.",
    confirmAccountImport: "Back up and import accounts",
    accountsImported: "Safely imported {count} missing account(s).",
    accountCurrentLoginPreserved: "The current Codex login was preserved.",
    accountRollbackAvailable: "A target-side restore point is available. Undo refuses to remove an imported account after it was refreshed or edited, preventing loss of newer data.",
    accountRollbackTitle: "Undo the latest account import",
    accountRollbackHint: "Type ROLLBACK ACCOUNTS to remove only unchanged accounts created by this import. Viewer source data is always retained.",
    confirmAccountRollback: "Undo account import",
    accountsRolledBack: "Accounts created by this import were removed.",
    accountRecoveryRequired: "An unfinished or later-modified account migration needs recovery review before another import.",
  },
} as const;

export const legacyArtifactLabels: Record<Language, Record<LegacyMigrationArtifactKind, string>> = {
  zh: {
    settings: "设置与 Work Plan",
    account_index: "账号索引",
    account_records: "账号记录",
    quota_cache: "额度缓存",
    session_manager_settings: "Session Manager 偏好",
    provider_mode: "Provider 模式",
    restore_points: "切换恢复点",
  },
  en: {
    settings: "Settings and Work Plan",
    account_index: "Account index",
    account_records: "Account records",
    quota_cache: "Quota cache",
    session_manager_settings: "Session Manager preferences",
    provider_mode: "Provider mode",
    restore_points: "Switch restore points",
  },
};

export const legacyTargetLabels: Record<Language, Record<LegacyMigrationTarget, string>> = {
  zh: {
    monitor_settings: "监控刷新设置",
    shell_preferences: "菜单栏与语言偏好",
    work_plan: "Work Plan",
    account_vault: "账号 Vault",
    quota_cache: "额度缓存",
    session_preferences: "Session 偏好",
    provider_mode_state: "Provider 模式状态",
    restore_point_ledger: "恢复点账本",
  },
  en: {
    monitor_settings: "Monitor refresh settings",
    shell_preferences: "Menu bar and language preferences",
    work_plan: "Work Plan",
    account_vault: "Account vault",
    quota_cache: "Quota cache",
    session_preferences: "Session preferences",
    provider_mode_state: "Provider mode state",
    restore_point_ledger: "Restore-point ledger",
  },
};

export const legacyStateLabels: Record<Language, Record<LegacyMigrationArtifactState, string>> = {
  zh: { present: "可读取", missing: "未发现", invalid: "格式无效", unsafe: "路径不安全", limit_exceeded: "超过上限" },
  en: { present: "Readable", missing: "Not found", invalid: "Invalid", unsafe: "Unsafe path", limit_exceeded: "Over limit" },
};

export const legacyDispositionLabels: Record<Language, Record<LegacyMigrationDisposition, string>> = {
  zh: { import: "可导入", skip: "跳过", review: "需复核", blocked: "已阻止" },
  en: { import: "Import", skip: "Skip", review: "Review", blocked: "Blocked" },
};

export const legacyAccountDispositionLabels: Record<
  Language,
  Record<LegacyAccountMigrationDisposition, string>
> = {
  zh: {
    import: "补入账号",
    already_present: "已存在",
    keep_existing: "保留现有",
    unsupported: "另行处理",
  },
  en: {
    import: "Add account",
    already_present: "Already present",
    keep_existing: "Keep existing",
    unsupported: "Manual review",
  },
};

export const legacyAccountAuthModeLabels: Record<
  Language,
  Record<"chatgpt" | "api_key" | "unknown", string>
> = {
  zh: { chatgpt: "ChatGPT", api_key: "API 账号", unknown: "未知格式" },
  en: { chatgpt: "ChatGPT", api_key: "API account", unknown: "Unknown format" },
};

export const legacyConflictLabels: Record<Language, Record<LegacyMigrationConflictKind, string>> = {
  zh: {
    duplicate_account_id: "重复账号 ID",
    missing_account_record: "账号记录缺失",
    orphan_account_record: "孤立账号记录",
    invalid_account_record: "账号记录无效",
    invalid_restore_point: "恢复点无效",
    provider_restore_point_missing: "Provider 恢复点缺失",
  },
  en: {
    duplicate_account_id: "Duplicate account ID",
    missing_account_record: "Missing account record",
    orphan_account_record: "Orphan account record",
    invalid_account_record: "Invalid account record",
    invalid_restore_point: "Invalid restore point",
    provider_restore_point_missing: "Missing Provider restore point",
  },
};

export function formatMigrationBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB"];
  let amount = value;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || amount >= 100 ? 0 : amount >= 10 ? 1 : 2;
  return `${amount.toFixed(digits)} ${units[unit]}`;
}

export function migrationStatusColor(status: LegacyMigrationStatus): string {
  if (status === "ready") return "green";
  if (status === "partial") return "gold";
  if (status === "unsupported") return "default";
  return "red";
}

export function migrationDispositionColor(disposition: LegacyMigrationDisposition): string {
  if (disposition === "import") return "green";
  if (disposition === "review") return "gold";
  if (disposition === "blocked") return "red";
  return "default";
}

export function migrationAccountDispositionColor(
  disposition: LegacyAccountMigrationDisposition,
): string {
  if (disposition === "import") return "green";
  if (disposition === "keep_existing") return "gold";
  if (disposition === "unsupported") return "red";
  return "default";
}

export function migrationArtifactStateColor(state: LegacyMigrationArtifactState): string {
  if (state === "present") return "green";
  if (state === "missing") return "default";
  return "red";
}

export function migrationPlanCounts(report: LegacyMigrationDryRunReport) {
  return report.plan.reduce(
    (counts, action) => ({ ...counts, [action.disposition]: counts[action.disposition] + 1 }),
    { import: 0, review: 0, skip: 0, blocked: 0 } as Record<LegacyMigrationDisposition, number>,
  );
}

export function selectedMigrationFolderLabel(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) || "Viewer backup";
}

export function legacyOptionalStateLabel(
  value: boolean | null,
  enabled: string,
  disabled: string,
): string {
  return value === null ? "—" : value ? enabled : disabled;
}

export function legacyImportedLanguageLabel(value: "en" | "zh" | null): string {
  if (value === "zh") return "中文";
  if (value === "en") return "English";
  return "—";
}
