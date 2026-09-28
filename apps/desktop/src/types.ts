export interface UsageWindow {
  usedPercent: number;
  remainingPercent: number;
  resetsAt?: number | null;
  windowMinutes?: number | null;
}

export interface UsageSummary {
  primary?: UsageWindow | null;
  secondary?: UsageWindow | null;
  apiExpiresAt?: string | null;
  plan?: string | null;
  fetchedAt?: string | null;
  error?: string | null;
}

export interface Account {
  id: string;
  email: string;
  note: string;
  expiresAt: string;
  privateDetails: AccountPrivateDetails;
  plan: string;
  accountId?: string | null;
  active: boolean;
  autoSwitchEnabled: boolean;
  autoSwitchPriority: number;
  autoSwitchThreshold: number;
  localProxyCompatible: boolean;
  directSwitchCompatible: boolean;
  agentIdentity: boolean;
  official: boolean;
  metadataEditable: boolean;
  usage: UsageSummary;
}

export interface AccountPrivateDetails {
  password: string;
  phoneNumber: string;
  totpSecret: string;
}

export interface AccountDetailsDraft {
  note: string;
  expiresAt: string;
  privateDetails: AccountPrivateDetails;
}

export type SafeAccountSwitchMode = "managerOnly" | "localProxy" | "direct";
export type SafeSwitchProtectedTarget =
  | "currentAuth"
  | "currentConfig"
  | "managerState"
  | "providerConfigBackup"
  | "providerModelCatalog"
  | "aggregateApiStore"
  | "activeProviderProfile"
  | "activeProviderFieldMetadata";

export interface SafeAccountSwitchPreview {
  confirmToken: string;
  expiresAt: string;
  mode: SafeAccountSwitchMode;
  protectedTargets: SafeSwitchProtectedTarget[];
  willRestartClient: boolean;
  createsRestorePoint: boolean;
  automaticRollback: boolean;
}

export type SafeAccountDeactivatePreview = SafeAccountSwitchPreview;

export type SafeProviderMutationKind = "enter" | "exit";

export interface SafeProviderMutationPreview extends SafeAccountSwitchPreview {
  kind: SafeProviderMutationKind;
  targetLabel: string;
}

export interface SafeProviderEditPreview extends SafeAccountSwitchPreview {
  targetLabel: string;
}

export interface SafeRollbackPreview {
  confirmToken: string;
  expiresAt: string;
  protectedTargets: SafeSwitchProtectedTarget[];
  willRestartClient: boolean;
  verifiesCurrentState: boolean;
}

export interface ResetCredit {
  issuedAt?: string | null;
  expiresAt?: string | null;
}

export interface ResetCreditsSummary {
  credits: ResetCredit[];
}

export type ResetCreditsLoadState =
  | { status: "loading"; fetchedAt?: string }
  | { status: "loaded"; data: ResetCreditsSummary; fetchedAt: string }
  | { status: "error"; error: string; fetchedAt?: string };

export interface AppInfo {
  codexHome: string;
  authPath: string;
  configPath: string;
  accountStore: string;
  providerStore: string;
  version: string;
}

export type ProviderApiFormat = "openaiResponses" | "openaiChat";
export type ModelApiFormats = Record<string, ProviderApiFormat>;
export type ProviderKind = "custom" | "openai";
export type ProviderBalancePlatform = "newApi" | "sub2Api" | "deepSeek";
export type ReasoningEffort = "none" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";
export type ModelReasoningEfforts = Record<string, ReasoningEffort[]>;
export type ModelContextWindows = Record<string, number>;
export type ModelTokenCosts = Record<string, number>;

export type ProviderConnectivityIssueCode =
  | "invalid_base_url"
  | "local_proxy_url"
  | "credential_required"
  | "credential_endpoint_mismatch"
  | "credential_unavailable"
  | "timeout"
  | "network"
  | "unauthorized"
  | "forbidden"
  | "not_found"
  | "rate_limited"
  | "upstream_server"
  | "redirect_rejected"
  | "response_too_large"
  | "invalid_response"
  | "empty_model_catalog"
  | "request_failed"
  | "provider_unavailable"
  | "http_error";

export interface ProviderConnectivityIssue {
  code: ProviderConnectivityIssueCode;
  retryable: boolean;
  httpStatus?: number | null;
}

export interface ProviderConnectivityReport {
  reachable: boolean;
  issue?: ProviderConnectivityIssue | null;
}

export type CpaPoolSource = "unavailable" | "cached" | "live";

export type CpaPoolIssueCode =
  | "bridge_disabled"
  | "invalid_configuration"
  | "unsupported_platform"
  | "bridge_unavailable"
  | "bridge_timeout"
  | "bridge_rejected"
  | "output_too_large"
  | "invalid_response"
  | "no_records";

export interface CpaPoolIssue {
  code: CpaPoolIssueCode;
  retryable: boolean;
}

export interface CpaQuotaWindow {
  usedPercent: number;
  remainingPercent: number;
  windowMinutes?: number | null;
  resetsAt?: number | null;
}

export interface CpaPoolMember {
  id: string;
  displayName: string;
  isCurrentRoute: boolean;
  isRoutePreferred: boolean;
  isLatestRequestRoute: boolean;
  primary?: CpaQuotaWindow | null;
  secondary?: CpaQuotaWindow | null;
  planType?: string | null;
  model?: string | null;
  reasoningEffort?: string | null;
  statusCode?: number | null;
  failed?: boolean | null;
  isStale: boolean;
  refreshSkipped: boolean;
  skipReason?: string | null;
  quotaGuardState?: string | null;
  quotaGuardReason?: string | null;
  statsSampleCount?: number | null;
  estimatedRemainingSuccesses?: number | null;
  observedAt?: string | null;
}

export interface CpaPoolStatus {
  source: CpaPoolSource;
  stale: boolean;
  fetchedAt?: string | null;
  checkedAt: string;
  parentMemberId?: string | null;
  members: CpaPoolMember[];
  issue?: CpaPoolIssue | null;
}

export interface CpaBridgeSettings {
  enabled: boolean;
  sshHost: string;
  refreshSeconds: number;
  timeoutSeconds: number;
}

export interface RelayModelDiscoveryResult {
  status: "ready" | "manual_fallback";
  models: string[];
  issue?: ProviderConnectivityIssue | null;
}

export interface Provider {
  id: string;
  kind: ProviderKind;
  name: string;
  group: string;
  baseUrl: string;
  model: string;
  models: string[];
  modelReasoningEfforts: ModelReasoningEfforts;
  modelContextWindows: ModelContextWindows;
  modelApiFormats: ModelApiFormats;
  modelTokenCosts?: ModelTokenCosts;
  imageInputModels: string[];
  imageInputModelsConfigured: boolean;
  contextWindow?: number | null;
  modelSelectionControlledByCodex: boolean;
  apiFormat: ProviderApiFormat;
  active: boolean;
  autoSwitchEnabled: boolean;
  hasApiKey: boolean;
  supportsDirectSwitch: boolean;
  balancePlatform?: ProviderBalancePlatform | null;
  balanceQueryUrl?: string | null;
  balanceQueryUsesApiKey: boolean;
  hasBalanceQueryToken: boolean;
  walletQueryUrl?: string | null;
  hasWalletQueryToken: boolean;
  walletUsername?: string | null;
  hasWalletLoginCredentials: boolean;
}

export interface ProviderInput {
  id?: string;
  kind: ProviderKind;
  name: string;
  group?: string;
  baseUrl: string;
  model: string;
  models: string[];
  modelReasoningEfforts: ModelReasoningEfforts;
  modelContextWindows: ModelContextWindows;
  modelApiFormats?: ModelApiFormats;
  modelTokenCosts?: ModelTokenCosts;
  imageInputModels: string[];
  imageInputModelsConfigured?: boolean;
  contextWindow?: number | null;
  modelSelectionControlledByCodex: boolean;
  apiKey?: string;
  apiFormat: ProviderApiFormat;
  balancePlatform?: ProviderBalancePlatform | null;
  balanceQueryUrl?: string | null;
  balanceQueryToken?: string;
  balanceQueryUsesApiKey?: boolean;
  walletQueryUrl?: string | null;
  walletQueryToken?: string;
  walletUsername?: string;
  walletPassword?: string;
}

export interface AggregateApi {
  id: string;
  name: string;
  model: string;
  memberProviderIds: string[];
  enabled: boolean;
  active: boolean;
  memberConversationCounts: Record<string, number>;
}

export interface AggregateApiInput {
  id?: string;
  name: string;
  model: string;
  memberProviderIds: string[];
  enabled: boolean;
}

export interface CcSwitchImportRequest {
  requestId: string;
  app: string;
  name: string;
  endpoint: string;
  models: string[];
  apiKeyProvided: boolean;
  balancePlatform?: ProviderBalancePlatform | null;
}

export interface ProviderBalance {
  apiAmount?: number | null;
  apiUnit: string;
  apiUnlimited: boolean;
  walletAmount?: number | null;
  walletUnit: string;
  walletError?: string | null;
  balanceItems?: ProviderBalanceItem[];
  queriedAt: number;
}

export interface ProviderBalanceItem {
  amount: number;
  unit: string;
}

export interface LocalProxyStatus {
  running: boolean;
  address: string;
  port: number;
  baseUrl: string;
  autoSwitchOnQuotaExhaustion: boolean;
  concurrentAccountRoutingEnabled: boolean;
  customAutoSwitchPriorityEnabled: boolean;
  customAutoSwitchThresholdEnabled: boolean;
  globalAutoSwitchThreshold: number;
  autoDisableUnreachableAccounts: boolean;
  systemPromptFilterEnabled: boolean;
  systemPromptFilterRules: SystemPromptRule[];
  systemPromptInjectionEnabled: boolean;
  systemPromptInjectionPrompts: SystemPromptRule[];
  listenOnAllInterfaces: boolean;
  hasLanApiKey: boolean;
  imageGenerationAccountId?: string | null;
  imageInputTarget?: ImageModelTarget | null;
  imageOutputTarget?: ImageModelTarget | null;
  openaiAuthAccountId?: string | null;
}

export type ImageModelTarget =
  | { kind: "official"; accountId: string }
  | { kind: "provider"; providerId: string; model: string };

export type ImageRouteKind = "input" | "output";

export type LocalProxyStopPhase =
  | "stoppingClient"
  | "restoringConversations"
  | "skippingConversations"
  | "restoringConfiguration"
  | "restartingClient"
  | "complete"
  | "failed";

export interface LocalProxyStopProgress {
  phase: LocalProxyStopPhase;
  percent: number;
  processedFiles?: number | null;
  totalFiles?: number | null;
}

export type LocalProxyStartPhase =
  | "preparingClient"
  | "startingProxy"
  | "syncingConversations"
  | "restartingClient"
  | "complete"
  | "failed";

export interface LocalProxyStartProgress {
  phase: LocalProxyStartPhase;
  percent: number;
  processedFiles?: number | null;
  totalFiles?: number | null;
}

export interface ProxySession {
  id: string;
  title?: string | null;
  client: string;
  remoteAddress?: string | null;
  connectedAt: number;
  lastSeenAt: number;
  activeRequests: number;
  requestCount: number;
  provider?: string | null;
  concurrentRouted?: boolean;
  accountId?: string | null;
  accountEmail?: string | null;
  model?: string | null;
  contextTokens?: number | null;
  modelContextWindow?: number | null;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  cachedTokens: number;
}

export interface ProxySessionRequest {
    id: number;
    startedAt: number;
    model?: string | null;
    reasoningEffort?: string | null;
    conversation?: string | null;
    firstResponseTimeMs?: number | null;
    responseTimeMs?: number | null;
    totalTokens?: number | null;
    inputTokens?: number | null;
    outputTokens?: number | null;
    reasoningTokens?: number | null;
    cachedTokens?: number | null;
}

export interface ProxySessionLatencySummary {
  totalFirstResponseTimeMs: number;
  requestCount: number;
}

export interface DirectConversationSyncResult {
  conversationsUpdated: number;
  rolloutFilesUpdated: number;
}

export type CodexThreadKind = "conversation" | "external" | "subagent";
export type CodexThreadStatus = "active" | "archived";

export interface CodexThreadEntry {
  sessionId: string;
  sessionKind: CodexThreadKind;
  status: CodexThreadStatus;
  title: string;
  cwd: string;
  updatedAt: number | null;
  sizeBytes: number;
  rolloutCount?: number;
  matchExcerpt: string | null;
  matchTimestamp?: string | null;
  accountId: string | null;
  accountEmail: string | null;
  accountActive: boolean;
}

export interface CodexThreadSearchCoverage {
  totalSessions: number;
  searchedSessions: number;
  incompleteSessions: number;
  skippedRecords: number;
  completedSessions: number;
  pendingSessions: number;
  decodedBytes: number;
}

export interface CodexThreadSearchResult {
  entries: CodexThreadEntry[];
  coverage: CodexThreadSearchCoverage | null;
  continuation: string | null;
}

export interface SystemPromptRule {
  name?: string;
  text: string;
  enabled: boolean;
}

export interface CodexThreadTokenTotals {
  sessionId: string;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
}

export interface CodexThreadDetailSummary {
  sessionId: string;
  title: string;
  cwd: string;
  startedAt: string | null;
  updatedAt: string | null;
  originator: string;
  source: string;
  cliVersion: string;
  modelProvider: string;
  sizeBytes: number;
  segmentCount?: number;
  skippedRecordCount?: number;
  lineCount: number;
  eventCount: number;
  toolCallCount: number;
  userPromptExcerpt: string;
  latestAgentMessageExcerpt: string;
  resumeCommand: string | null;
}

export type CodexThreadTimelineKind = "message:user" | "message:assistant" | "tool_call";

export interface CodexThreadTimelineItem {
  id: string;
  kind: CodexThreadTimelineKind;
  timestamp: string | null;
  text: string | null;
  toolName: string | null;
  toolInput: string | null;
  toolOutput: string | null;
  toolStatus: "pending" | "completed" | "errored" | null;
  truncated: boolean;
}

export interface CodexThreadDetailPage {
  summary: CodexThreadDetailSummary;
  items: CodexThreadTimelineItem[];
  total: number;
  nextOffset: number | null;
  previousOffset?: number | null;
  offset?: number;
  revision: string;
}

export interface CodexThreadResumeResult {
  sessionId: string;
  cwd: string;
  mode: "savedDirectory" | "temporaryDirectory";
  resumeCommand: string;
  launched: boolean;
  message: string;
}

export type CodexThreadRebindProtectedTarget =
  | "rolloutMetadata"
  | "canonicalState"
  | "legacyState"
  | "threadCatalog"
  | "sessionIndexGuard";

export interface CodexThreadRebindPreview {
  confirmToken: string;
  expiresAt: string;
  sessionId: string;
  title: string;
  oldCwd: string;
  newCwd: string;
  rolloutFileCount: number;
  stateDatabaseRowCount: number;
  catalogRowCount: number;
  protectedTargets: CodexThreadRebindProtectedTarget[];
  sessionIndexUnchanged: boolean;
  automaticRollback: boolean;
  crashRecovery: boolean;
}

export interface CodexThreadRebindReport {
  sessionId: string;
  oldCwd: string;
  newCwd: string;
  rolloutFileCount: number;
  stateDatabaseRowCount: number;
  catalogRowCount: number;
  message: string;
}

export interface CodexThreadBinEntry {
  sessionId: string;
  title: string;
  cwd: string;
  deletedAt: number | null;
  sizeBytes: number;
}

export type CodexThreadTrashProtectedTarget =
  | "rolloutFiles"
  | "sessionIndex"
  | "stateVisibility";

export interface CodexThreadTrashPreviewItem {
  sessionId: string;
  title: string;
  cwd: string;
  sizeBytes: number;
}

export interface CodexThreadTrashPreview {
  confirmToken: string;
  expiresAt: string;
  requestedCount: number;
  affectedCount: number;
  totalSizeBytes: number;
  items: CodexThreadTrashPreviewItem[];
  protectedTargets: CodexThreadTrashProtectedTarget[];
  createsRestorePoint: boolean;
  automaticRollback: boolean;
}

export type CodexThreadRestoreProtectedTarget =
  | "rolloutFiles"
  | "sessionIndex"
  | "stateVisibility";

export interface CodexThreadRestorePreviewItem {
  sessionId: string;
  title: string;
  cwd: string;
  sizeBytes: number;
}

export interface CodexThreadRestorePreview {
  confirmToken: string;
  expiresAt: string;
  requestedCount: number;
  affectedCount: number;
  totalSizeBytes: number;
  items: CodexThreadRestorePreviewItem[];
  protectedTargets: CodexThreadRestoreProtectedTarget[];
  conflictsChecked: boolean;
  automaticRollback: boolean;
}

export interface CodexThreadPurgePreviewItem {
  sessionId: string;
  title: string;
  cwd: string;
  sizeBytes: number;
}

export interface CodexThreadPurgePreview {
  confirmToken: string;
  expiresAt: string;
  emptyBin: boolean;
  requestedCount: number;
  affectedCount: number;
  totalSizeBytes: number;
  items: CodexThreadPurgePreviewItem[];
  createsTemporaryRestorePoint: boolean;
  automaticRollback: boolean;
  permanentlyDeletes: boolean;
}

export interface CodexThreadPurgeOutcome {
  sessionId: string;
  status: "purged";
  releasedBytes: number;
}

export interface CodexThreadPurgeReport {
  requestedCount: number;
  affectedCount: number;
  releasedBytes: number;
  outcomes: CodexThreadPurgeOutcome[];
  restorePointRemoved: boolean;
  message: string;
}

export type CodexThreadArchiveProtectedTarget =
  | "rolloutFiles"
  | "sessionIndex"
  | "stateVisibility";

export interface CodexThreadArchivePreviewItem {
  sessionId: string;
  title: string;
  cwd: string;
  sizeBytes: number;
}

export interface CodexThreadArchivePreview {
  confirmToken: string;
  expiresAt: string;
  requestedCount: number;
  affectedCount: number;
  totalSizeBytes: number;
  items: CodexThreadArchivePreviewItem[];
  protectedTargets: CodexThreadArchiveProtectedTarget[];
  conflictsChecked: boolean;
  preservesSessionIndex: boolean;
  automaticRollback: boolean;
  crashRecovery: boolean;
}

export interface CodexThreadMutationReport {
  requestedCount: number;
  affectedCount: number;
  releasedBytes: number;
  message: string;
}

export interface CodexThreadBundleItem {
  sessionId: string;
  title: string;
  cwd: string;
  updatedAt: number | null;
  sizeBytes: number;
  status: "ready" | "duplicate" | "conflict" | "invalid";
  reason: string | null;
}

export interface CodexThreadBundlePreview {
  packageVersion: number;
  exportedAt: string | null;
  totalCount: number;
  readyCount: number;
  totalSizeBytes: number;
  items: CodexThreadBundleItem[];
}

export interface CodexThreadBundleResult {
  requestedCount: number;
  completedCount: number;
  skippedCount: number;
  path: string;
  message: string;
}

export type CodexThreadVisibilityRepairProtectedTarget =
  | "rolloutSource"
  | "canonicalState"
  | "legacyState"
  | "threadCatalog"
  | "sessionIndex";

export interface CodexThreadVisibilityRepairPreview {
  confirmToken: string;
  expiresAt: string;
  mode: "quick" | "deep";
  scope: "all" | "selected";
  requestedCount: number;
  scannedCount: number;
  rolloutGuardCount: number;
  stateUpdateCount: number;
  stateInsertCount: number;
  catalogUpdateCount: number;
  indexAddCount: number;
  indexUpdateCount: number;
  protectedTargets: CodexThreadVisibilityRepairProtectedTarget[];
  rolloutSourceUnchanged: boolean;
  automaticRollback: boolean;
  crashRecovery: boolean;
}

export interface CodexThreadVisibilityRepairReport {
  mode: "quick" | "deep";
  repairedSessionCount: number;
  stateUpdateCount: number;
  stateInsertCount: number;
  catalogUpdateCount: number;
  indexAddCount: number;
  indexUpdateCount: number;
  message: string;
}

export type LegacyMigrationStatus = "ready" | "partial" | "unsupported" | "failed";
export type LegacyMigrationArtifactKind =
  | "settings"
  | "account_index"
  | "account_records"
  | "quota_cache"
  | "session_manager_settings"
  | "provider_mode"
  | "restore_points";
export type LegacyMigrationArtifactState =
  | "present"
  | "missing"
  | "invalid"
  | "unsafe"
  | "limit_exceeded";
export type LegacyMigrationTarget =
  | "monitor_settings"
  | "shell_preferences"
  | "work_plan"
  | "account_vault"
  | "quota_cache"
  | "session_preferences"
  | "provider_mode_state"
  | "restore_point_ledger";
export type LegacyMigrationDisposition = "import" | "skip" | "review" | "blocked";
export type LegacyMigrationConflictKind =
  | "duplicate_account_id"
  | "missing_account_record"
  | "orphan_account_record"
  | "invalid_account_record"
  | "invalid_restore_point"
  | "provider_restore_point_missing";

export interface LegacyMigrationArtifact {
  kind: LegacyMigrationArtifactKind;
  state: LegacyMigrationArtifactState;
  fileCount: number;
  recordCount: number;
  sourceBytes: number;
  reasonCodes: string[];
}

export interface LegacyMigrationAction {
  target: LegacyMigrationTarget;
  disposition: LegacyMigrationDisposition;
  sourceItemCount: number;
  expectedTargetCount: number;
  reasonCode: string;
}

export interface LegacyMigrationConflict {
  kind: LegacyMigrationConflictKind;
  count: number;
  reasonCode: string;
}

export interface LegacyMigrationDryRunReport {
  schemaVersion: "1.0";
  status: LegacyMigrationStatus;
  reasonCodes: string[];
  source: {
    product: "codex_quota_viewer";
    format: string | null;
    baselineCommit: string;
    readOnly: true;
  };
  inventory: LegacyMigrationArtifact[];
  plan: LegacyMigrationAction[];
  conflicts: LegacyMigrationConflict[];
  spaceEstimate: {
    sourceBytes: number;
    minimumFreeBytes: number;
    status: string;
  };
}

export interface LegacyMigrationImportSummary {
  autoRefreshEnabled: boolean | null;
  refreshIntervalSeconds: number | null;
  launchAtLogin: boolean | null;
  language: "en" | "zh" | null;
  legacyStatusItemStyle: "meter" | "text" | null;
  statusItemPolicy: "dynamic_capacity";
  workPlanEnabled: boolean | null;
  offPeriodCount: number;
  sessionLanguage: "en" | "zh" | null;
}

export interface LegacyMigrationApplyPreview {
  schemaVersion: "1.0";
  status: "confirmation_required" | "already_applied";
  confirmToken: string | null;
  expiresAt: string | null;
  typedConfirmation: "IMPORT";
  changedTargets: LegacyMigrationTarget[];
  reviewTargetsExcluded: LegacyMigrationTarget[];
  imported: LegacyMigrationImportSummary;
  createsBackup: boolean;
  automaticRollback: boolean;
}

export interface LegacyMigrationApplyResult {
  schemaVersion: "1.0";
  status: "applied" | "rolled_back";
  operationId: string | null;
  changedTargets: LegacyMigrationTarget[];
  sourceUnchanged: boolean;
  rollbackAvailable: boolean;
  language: "en" | "zh" | null;
}

export interface LegacyMigrationOperationView {
  schemaVersion: "1.0";
  status: "none" | "prepared" | "applying" | "committed" | "rolled_back" | "needs_review";
  operationId: string | null;
  changedTargets: LegacyMigrationTarget[];
  rollbackAvailable: boolean;
  recoveryRequired: boolean;
  typedRollback: "ROLLBACK";
}

export type LegacyAccountMigrationDisposition =
  | "import"
  | "already_present"
  | "keep_existing"
  | "unsupported";

export interface LegacyAccountMigrationPreviewRow {
  sourceOrdinal: number;
  label: string;
  authMode: "chatgpt" | "api_key" | "unknown";
  preferred: boolean;
  disposition: LegacyAccountMigrationDisposition;
  reasonCode: string;
}

export interface LegacyAccountMigrationPreview {
  schemaVersion: "1.0";
  status: "confirmation_required" | "already_applied" | "review_only";
  confirmToken: string | null;
  expiresAt: string | null;
  typedConfirmation: "IMPORT ACCOUNTS";
  accounts: LegacyAccountMigrationPreviewRow[];
  importCount: number;
  alreadyPresentCount: number;
  conflictCount: number;
  unsupportedCount: number;
  preservesCurrentLogin: true;
  importsQuotaCache: false;
  createsRestorePoint: boolean;
  automaticRollback: true;
}

export interface LegacyAccountMigrationResult {
  schemaVersion: "1.0";
  status: "applied" | "rolled_back";
  operationId: string | null;
  importedCount: number;
  alreadyPresentCount: number;
  conflictCount: number;
  unsupportedCount: number;
  sourceUnchanged: true;
  currentLoginPreserved: true;
  rollbackAvailable: boolean;
}

export interface LegacyAccountMigrationOperationView {
  schemaVersion: "1.0";
  status: "none" | "prepared" | "applying" | "committed" | "rolled_back" | "needs_review";
  operationId: string | null;
  importedCount: number;
  rollbackAvailable: boolean;
  recoveryRequired: boolean;
  typedRollback: "ROLLBACK ACCOUNTS";
}

export interface LegacyProviderImportRow {
  sourceOrdinal: number;
  label: string;
  endpoint: string | null;
  model: string | null;
  apiFormat: ProviderApiFormat | null;
  reasoningEffort: ReasoningEffort | null;
  contextWindow: number | null;
  disposition: "import" | "keep_existing" | "review";
  reasonCode: string | null;
  confirmToken: string | null;
  expiresAt: string | null;
}

export interface LegacyProviderImportPreview {
  providers: LegacyProviderImportRow[];
  typedConfirmation: "IMPORT API";
}

export interface LegacyProviderImportOperation {
  operationId: string | null;
  label: string | null;
  status: "none" | "unfinished" | "committed" | "rolled_back";
  rollbackAvailable: boolean;
  recoveryRequired: boolean;
  typedRollback: "ROLLBACK API";
}

export interface CodexThreadMigrationReport {
  requestedCount: number;
  migratedCount: number;
  skippedCount: number;
  message: string;
}

export interface TokenUsageEntry {
  id: string;
  ts: number;
  provider: string;
  providerId?: string | null;
  accountId?: string | null;
  accountEmail?: string | null;
  model: string;
  durationMs?: number | null;
  inputTokens?: number | null;
  outputTokens?: number | null;
  reasoningTokens?: number | null;
  cachedTokens?: number | null;
  totalTokens?: number | null;
  modelContextWindow?: number | null;
}

export interface AccountTokenUsageTotals {
  accountId?: string | null;
  accountEmail?: string | null;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  cachedTokens: number;
  estimatedCost: number;
}

export interface ProviderTokenUsageTotals {
  provider: string;
  providerId?: string | null;
  todayTokens: number;
  totalTokens: number;
  todayEstimatedCost: number;
  totalEstimatedCost: number;
}

export interface DailyTokenUsage {
  date: string;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  cachedTokens: number;
}

export interface UpdateInfo {
  currentVersion: string;
  latestVersion: string;
  releaseName: string;
  releaseNotes?: string | null;
  releaseUrl: string;
}

export interface AppSettings {
  codexHome?: string | null;
  launchAtStartup?: boolean;
  closeToTray?: boolean;
  floatingBubbleEnabled: boolean;
  privacyMode: boolean;
  hideAccountNotes: boolean;
  bubbleResetDisplay: BubbleResetDisplay;
  bubbleStyle: BubbleStyle;
  themeColor?: string | null;
  bubbleX?: number | null;
  bubbleY?: number | null;
  cloudBaseUrl?: string | null;
  showCustomCloudServer?: boolean;
  tokenUsageWeeks?: number;
  tokenUsageRefreshSeconds?: number;
  autoDisableStatusCodes?: number[];
  upstream429RetryTimeoutSeconds?: number;
  showUsageNetworkErrors?: boolean;
  gpt56SolContextWindow?: number;
  officialModelContextWindows?: Record<string, number>;
  webProxyPort?: number | null;
  webProxyListenOnAllInterfaces?: boolean;
  networkProxy?: NetworkProxySettings;
  cpaBridge?: CpaBridgeSettings;
  providerGroups?: string[];
  thirdPartyAppWrite?: ThirdPartyAppWriteSettings;
  claudeCodeWriteTarget?: ClaudeCodeWriteTarget;
}

export type ClaudeCodeWriteTarget = "all" | "codex" | "claudeCode";

export type ThirdPartyAppId =
  | "claudeCode"
  | "openCode"
  | "openClaw"
  | "hermesAgent"
  | "trae"
  | "workBuddy"
  | "zCode"
  | "deepSeekHarness"
  | "openViking";

export interface ThirdPartyAppWriteSettings {
  enabled: boolean;
  writeCodex: boolean;
  apps: Record<ThirdPartyAppId, boolean>;
  claudeSubagentModel: ClaudeSubagentModel;
}

/** Model identifier used for Claude Code background agents. */
export type ClaudeSubagentModel = string;

export interface NetworkProxySettings {
  enabled: boolean;
  proxyUrl: string;
  proxyPort: number | null;
}

export interface LoginStart {
  loginId: string;
  embedded: boolean;
}

export type LoginPhase = "waiting" | "exchanging" | "browserFallback" | "succeeded" | "cancelled" | "timedOut" | "failed";

export interface LoginStatus {
  ok: boolean;
  message: string;
  accountId?: string | null;
  loginId?: string | null;
  phase?: LoginPhase | null;
  reason?: string | null;
}

export interface CloudAuthState {
  enabled: boolean;
  baseUrl?: string | null;
  authenticated: boolean;
  userEmail?: string | null;
  userId?: string | null;
  lastSyncAt?: string | null;
  sessionExpired: boolean;
}

export interface SavedCloudLogin {
  email: string;
  password: string;
}

export interface CloudAuthenticationResult {
  state: CloudAuthState;
  passwordSaved: boolean;
  credentialStorageUpdated: boolean;
}

export interface CloudSyncResult {
  uploaded: number;
  downloaded: number;
}

export interface CloudAnnouncement {
  /** Legacy Chinese content returned for compatibility with older clients. */
  content: string;
  contentZh: string;
  contentEn: string;
  link: string;
  enabled: boolean;
  textColor: string;
  backgroundColor: string;
  scrollDurationSeconds: number;
  updatedAt?: string | null;
}

export interface CloudCurrencyRate {
  code: string;
  name: string;
  rate: number;
}

export interface CloudCurrencyRates {
  currencies: CloudCurrencyRate[];
  updatedAt: string | null;
}

export interface CloudNotification {
  id: string;
  titleZh: string;
  titleEn: string;
  contentZh: string;
  contentEn: string;
  link: string;
  linkLabelZh: string;
  linkLabelEn: string;
  enabled: boolean;
  publishedAt: string;
  updatedAt: string;
}

export interface CloudFaq {
  id: string;
  questionZh: string;
  questionEn: string;
  answerZh: string;
  answerEn: string;
  enabled: boolean;
  sortOrder: number;
  createdAt: string;
  updatedAt: string;
}

export interface SkillMarketItem {
  id: string;
  title: string;
  description: string;
  version: string;
  archiveSize: number;
  archiveSha256: string;
  hasPreview: boolean;
  uploaderId?: string | null;
  official: boolean;
  installCount: number;
  createdAt: string;
  updatedAt: string;
  installed: boolean;
  installedVersion?: string | null;
  enabled: boolean;
}

export interface OfficialPluginItem {
  id: string;
  name: string;
  title: string;
  description: string;
  version: string;
  category: string;
  developer: string;
  brandColor?: string | null;
  iconUrl?: string | null;
  installed: boolean;
  enabled: boolean;
}

export type PromptPluginType = "injection" | "filter";

export interface PromptPluginItem {
  id: string;
  name: string;
  version: string;
  type: PromptPluginType;
  text: string;
  uploaderId?: string | null;
  installCount: number;
  createdAt: string;
  updatedAt: string;
  installed: boolean;
  installedVersion?: string | null;
  enabled: boolean;
}

export interface PromptPluginPublishInput {
  pluginId?: string | null;
  name: string;
  version: string;
  type: PromptPluginType;
  text: string;
}

export type SkillPackageKind = "archive" | "folder";

export interface SkillPackageSelection {
  path: string;
  kind: SkillPackageKind;
  name: string;
}

export interface SkillPublishInput {
  skillId?: string | null;
  title: string;
  description: string;
  version: string;
  package: SkillPackageSelection;
  preview?: FeedbackImageInput | null;
}

export interface FeedbackImageInput {
  fileName: string;
  mimeType: string;
  dataBase64: string;
}

export interface AccountArchiveImportResult {
  imported: number;
  accountIds: string[];
  activeAccountId?: string | null;
  providersImported: number;
  providerIds: string[];
  activeProviderId?: string | null;
}

export type BubbleResetDisplay = "countdown" | "resetAt";
export type BubbleStyle = "classic" | "glass";

export interface DreamSkinThemeSummary {
  id: string;
  name: string;
}

export interface DeletedCloudAccount {
  id: string;
  email: string;
  note: string;
  expiresAt: string;
  plan: string;
  deletedAt: string;
}

export interface DeletedCloudProvider {
  id: string;
  name: string;
  baseUrl: string;
  model: string;
  deletedAt: string;
}

export interface DreamSkinMarketTheme {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string;
  license: string;
  sourceUrl: string;
  tags: string[];
  theme: string;
  image: string;
  preview: string;
  themeSha256: string;
  imageSha256: string;
  previewUrl: string;
  installed: boolean;
  installedVersion?: string | null;
  updateAvailable: boolean;
}

export interface DreamSkinMarketResult {
  schemaVersion: number;
  updatedAt: string;
  repositoryUrl: string;
  cached: boolean;
  warning?: string | null;
  themes: DreamSkinMarketTheme[];
}

export interface DreamSkinCommunityTheme {
  applyCompatible: boolean;
  authorDisplayName: string;
  authorUserId: string;
  displayMeta: Record<string, unknown>;
  downloadCount: number;
  id: string;
  license: string;
  name: string;
  packageBytes: number;
  packageSha256: string;
  reviewedAt: string;
  slug: string;
  submittedAt: string;
  themeId: string;
  version: string;
  previewUrl: string;
  installed: boolean;
  installedVersion?: string | null;
  updateAvailable: boolean;
}

export interface DreamSkinCommunityPage {
  items: DreamSkinCommunityTheme[];
  total: number;
  offset: number;
  limit: number;
  cached: boolean;
  warning?: string | null;
}

export type DreamSkinResourcesPhase = "idle" | "checking" | "downloading" | "ready" | "error" | "unsupported";

export interface DreamSkinResourcesStatus {
  phase: DreamSkinResourcesPhase;
  installed: boolean;
  installedVersion?: string | null;
  availableVersion?: string | null;
  downloadedBytes: number;
  totalBytes?: number | null;
  error?: string | null;
}

export type DreamSkinSession = "unsupported" | "notInstalled" | "ready" | "active" | "paused";
export type DreamSkinAppearance = "auto" | "light" | "dark";

export interface DreamSkinStatus {
  supported: boolean;
  platform: string;
  installed: boolean;
  runtimeInstalled: boolean;
  session: DreamSkinSession;
  activeThemeId?: string | null;
  activeThemeName?: string | null;
  activeThemeAppearance?: DreamSkinAppearance | null;
  activeThemeOverlayOpacity?: number | null;
  enginePath?: string | null;
  savedThemes: DreamSkinThemeSummary[];
}

export interface DreamSkinImportOptions {
  name: string;
  appearance: DreamSkinAppearance;
  safeArea: "auto" | "left" | "right" | "center" | "none";
  taskMode: "auto" | "ambient" | "banner" | "off";
  focusX?: number | null;
  focusY?: number | null;
}
