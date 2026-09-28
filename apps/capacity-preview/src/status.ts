export type Availability = "complete" | "partial" | "unsupported" | "failed";
export type Freshness = "live" | "stale" | "not_applicable";
export type Compatibility =
  | "tested"
  | "expected_compatible"
  | "not_tested"
  | "unsupported"
  | "known_broken"
  | "not_applicable";

export interface ImplementationStatus {
  phase: "wp4d_desktop_live" | string;
  desktopLiveReadsEnabled: boolean;
  storeBackendAvailable: boolean;
  mutationStatusEnabled: boolean;
}

export interface DesktopIssue {
  code: string;
  message: string;
  retryAfterMs: number | null;
}

export interface CodexCandidate {
  executableId: string;
  canonicalPath: string | null;
  version: string | null;
  sources: string[];
  verification: "verified" | "confirmation_required" | "version_timeout" | "version_failed";
  requiresConfirmation: boolean;
}

export interface DesktopDataStatus {
  availability: Availability;
  freshness: Freshness;
  compatibility: Compatibility;
  reasonCodes: string[];
}

export interface DesktopQuotaWindow {
  limitId: string;
  label: string | null;
  windowMinutes: number | null;
  usedPercent: number;
  remainingPercent: number;
  resetsAt: string | null;
}

export interface DesktopStatus {
  schemaVersion: "1.0" | string;
  capturedAt: string;
  codexVersion: string | null;
  account: {
    authMode: string | null;
    planType: string | null;
    bindingStatus: "stable" | "ephemeral" | "unavailable";
  } | null;
  dataStatus: DesktopDataStatus;
  quotaWindows: DesktopQuotaWindow[];
  quotaObservedAt?: string;
  quotaFreshness?: "live" | "stale" | "not_applicable";
  quotaSource?: "managed_account_cache";
  resetCredits: {
    summaryStatus: "available" | "partial" | "unavailable";
    availableCount: number | null;
    detailsStatus: "complete" | "partial" | "unavailable";
  };
  usage: {
    availability: Availability;
    hasSummary: boolean;
    reasonCodes: string[];
  };
  diagnosticCodes: string[];
}

export interface DesktopStatusEnvelope {
  schemaVersion: "1.0";
  sequence: number;
  /** Opaque process-local boundary; changes on account/source changes, not refreshes. */
  historyContextId?: string;
  lifecycle: "idle" | "ready" | "stale" | "selection_required" | "error";
  status: DesktopStatus | null;
  candidates: CodexCandidate[];
  selectedExecutableId: string | null;
  /** Source category only; normal status does not expose executable paths. */
  selectedExecutableSource?: "desktop_app" | "cli";
  issue: DesktopIssue | null;
  /** Current cross-session account history, not merely SQLite availability. */
  persistenceEnabled: boolean;
}

export interface DesktopHistoryPoint {
  snapshotId: string;
  capturedAt: string;
  limitId: string;
  label: string | null;
  windowMinutes: number | null;
  usedPercent: number;
  remainingPercent: number;
  resetsAt: string | null;
  availability: "complete" | "partial";
  compatibility: Compatibility;
}

export interface DesktopHistoryEnvelope {
  schemaVersion: "1.0";
  historyContextId?: string;
  status: "available" | "binding_required" | "unavailable" | "failed" | "invalid_request";
  reasonCode: string;
  points: DesktopHistoryPoint[];
  /** Last 30 days relative to the latest capture; hourly first/last/extrema points. */
  overview?: {
    sampleCount: number;
    /** Computed from raw captures, before chart reduction; OS-local dates. */
    dailyActivity: DesktopHistoryDayActivity[];
    /** Raw, account-scoped observations; never proof of a global reset. */
    quotaChanges?: DesktopQuotaChangeSummary;
  };
}

export interface DesktopQuotaRiseObservation {
  beforeSnapshotId: string;
  firstAfterSnapshotId: string;
  confirmationSnapshotId: string | null;
  beforeAt: string;
  firstAfterAt: string;
  confirmedAt: string | null;
  beforeRemainingPercent: number;
  afterRemainingPercent: number;
  confirmedRemainingPercent: number | null;
  beforeResetsAt: string | null;
  afterResetsAt: string | null;
  classification: "awaiting_followup" | "around_scheduled_boundary" | "before_scheduled_boundary" | "unverified";
  limitation: "awaiting_followup" | "observation_gap" | "window_unavailable" | "window_not_reanchored"
    | "window_changed_again" | "not_sustained" | "ambiguous_timing" | "source_changed" | "invalid_sample" | null;
}

export interface DesktopQuotaChangeSummary {
  algorithmVersion: string;
  validSamples: number;
  comparableIntervals: number;
  excludedIntervals: number;
  dateOnlyChanges: number;
  totalRises: number;
  observations: DesktopQuotaRiseObservation[];
}

export interface DesktopHistoryDayActivity {
  date: string;
  sampleCount: number;
  comparableIntervals: number;
  consumedPercent: number;
}

export interface DesktopWorkSchedulePeriod {
  startMinuteOfDay: number;
  endMinuteOfDay: number;
}

export interface DesktopWorkSchedule {
  revision: number;
  enabled: boolean;
  offPeriods: DesktopWorkSchedulePeriod[];
  updatedAt: string;
}

export interface DesktopSchedulePaceComparison {
  enabled: boolean;
  segment: "working" | "off";
  targetAt: string | null;
  expectedRemainingPercent: number;
  actualRemainingPercent: number;
  overspendPercent: number;
  state: "on_pace" | "within_guard" | "over_guard";
  dailyBudgetPercent: number;
  usableMinutesPerDay: number;
  offMinutesPerDay: number;
  windowExpired: boolean;
}

export interface DesktopWorkPlanEnvelope {
  schemaVersion: "1.0";
  status: "available" | "updated" | "revision_conflict" | "invalid_request" | "failed";
  reasonCode: string;
  schedule: DesktopWorkSchedule | null;
  baseline: {
    kind: "schedule_baseline";
    limitId: string;
    source: "app_server_status" | "managed_account_cache";
    observedAt: string;
    freshness: Freshness;
    comparison: DesktopSchedulePaceComparison;
  } | null;
}

export interface DesktopWorkScheduleUpdateRequest {
  expectedRevision: number;
  enabled: boolean;
  offPeriods: DesktopWorkSchedulePeriod[];
}

export interface CapacityDemandInput {
  horizonEnd: string;
  demandKind: "active_hours" | "maintain_recent_pace";
  plannedCodexActiveHours: number | null;
}

export interface DesktopDemandPlan {
  workPlanId: string;
  revision: number;
  demandId: string;
  demandRevision: number;
  enabled: boolean;
  demand: CapacityDemandInput;
  createdAt: string;
}

export interface DesktopDemandPlanUpdate {
  historyContextId: string;
  expectedRevision: number;
  enabled: boolean;
  demand: CapacityDemandInput;
}

export interface PaceRange {status: "available" | "unavailable"; lower: number | null; upper: number | null; reasonCode: string}
export interface PaceEstimate {
  paceEstimateId: string;
  algorithmVersion: string;
  state: "pace_only" | "abstained";
  reasonCode: string;
  limitId: string;
  windowMinutes: number | null;
  generatedAt: string;
  observedAt: string | null;
  forecastHorizon: string | null;
  currentRemainingPercent: number | null;
  inputSnapshotIds: string[];
  historyCoverage: {sampleCount: number; coveredSeconds: number; requiredSeconds: number; maximumGapSeconds: number};
  observedBlockRates: number[];
  rateRange: PaceRange;
  balanceAtHorizon: PaceRange;
  estimatedCodexActiveHoursRange: PaceRange;
  depletionTimeRange: {status: "available" | "unavailable"; earliestAt: string | null; latestAt: string | null; reasonCode: string};
  compatibilityUnverified: boolean;
  cachedSource: boolean;
  dataStability: string;
  assumptions: string[];
  invalidationFactors: string[];
  decision: "not_assessed";
  backtestStatus: "not_started" | "insufficient_samples";
}
export interface DesktopPaceEnvelope {
  schemaVersion: "1.0";
  historyContextId: string;
  status: "available" | "unavailable" | "failed";
  reasonCode: string;
  estimates: PaceEstimate[];
  generatedAt: string;
}

export interface PaceOutcomeSummary {
  verifierVersion: string;
  assessedAt: string;
  classification: string;
  reasonCode: string;
  observedRange: [number, number] | null;
  observedFrom: string | null;
  observedUntil: string | null;
  sampleCount: number;
  maximumGapSeconds: number;
  compatibilityUnverified: boolean;
}
export interface PaceTrialView {
  trialId: string;
  algorithmVersion: string;
  issuedAt: string;
  observedAt: string;
  forecastHorizon: string;
  limitId: string;
  windowMinutes: number | null;
  balanceRange: [number, number];
  compatibilityUnverified: boolean;
  outcome: PaceOutcomeSummary | null;
}
export interface DesktopPaceTrialsEnvelope {
  historyContextId: string;
  status: "available" | "unavailable" | "failed";
  reasonCode: string;
  offset: number;
  hasMore: boolean;
  recordingFailed: boolean;
  trials: PaceTrialView[];
  summary: PaceEvidenceSummary | null;
}
export interface PaceEvidenceCounts {
  total: number;
  pending: number;
  within: number;
  below: number;
  above: number;
  unscorable: number;
}
export interface PaceEvidenceSummary {
  generatedAt: string;
  oldestIssuedAt: string | null;
  newestIssuedAt: string | null;
  limited: boolean;
  counts: PaceEvidenceCounts;
  groups: Array<{
    algorithmVersion: string;
    verifierVersion: string;
    windowMinutes: number | null;
    compatibilityUnverified: boolean;
    counts: PaceEvidenceCounts;
    trainingMaximumGapSeconds: [number, number] | null;
    trainingRateRatio: [number, number] | null;
  }>;
  unscorableReasons: Array<{reasonCode: string; count: number}>;
}

export type PlanningArchiveRef = {kind: "plan" | "record" | "timer" | "trial"; id: string};
export type PlanningArchiveQuery = {kind: "list"; offset: number}
  | {kind: "details"; source: PlanningArchiveRef; offset: number}
  | {kind: "pace"; source: PlanningArchiveRef; offset: number}
  | {kind: "quota"; source: PlanningArchiveRef; observationId: string};
export interface PlanningArchiveEntry {
  source: PlanningArchiveRef;
  platform: string;
  architecture: string;
  boundary: string;
  codexVersion: string | null;
  lastRecordedAt: string;
  planRevisions: number;
  recordCount: number;
  trialCount: number;
}
export type PlanningArchiveResult = {kind: "list"; entries: PlanningArchiveEntry[]; hasMore: boolean}
  | {kind: "pace"; trials: PaceTrialView[]; hasMore: boolean}
  | {kind: "details"; plans: DesktopDemandPlan[]; records: ActiveTimeObservation[];
      draft: {startedAt: string; lastCheckpoint: string; suggestedSeconds: number} | null; hasMore: boolean}
  | {kind: "quota"; record: ActiveTimeObservation; comparison: NonNullable<DesktopActiveTimeQuotaEnvelope["comparison"]>};
export interface DesktopPlanningArchiveEnvelope {
  historyContextId: string;
  query: PlanningArchiveQuery;
  status: "available" | "unavailable" | "failed" | "missing" | "invalid_request";
  reasonCode: string;
  result: PlanningArchiveResult | null;
  generatedAt: string;
}

export interface DesktopDemandPlanEnvelope {
  schemaVersion: "1.0";
  historyContextId: string;
  status: "available" | "updated" | "unavailable" | "revision_conflict" | "invalid_request" | "failed";
  reasonCode: string;
  plans: DesktopDemandPlan[];
  hasOtherEnvironmentPlans: boolean;
  allowances: Array<{
    limitId: string;
    windowMinutes: number;
    remainingPercent: number;
    resetsAt: string | null;
    allocation: { method: "equal_allocation_v1"; reasonCode: string; dailyPercent: number | null; perPlannedHourPercent: number | null };
  }>;
  generatedAt: string;
  observedAt: string | null;
  freshness: Freshness;
  decision: "not_assessed";
}

export type DesktopSettingsLanguage = "system" | "en" | "zh-CN";

export interface DesktopMonitorSettings {
  revision: number;
  autoRefreshEnabled: boolean;
  refreshIntervalSeconds: 60 | 300 | 900;
  notificationThresholdBasisPoints: number | null;
  resetCreditNoticeHours: number;
  quietHoursEnabled: boolean;
  quietHoursStartMinute: number | null;
  quietHoursEndMinute: number | null;
  language: DesktopSettingsLanguage;
  lockScreenPrivacy: boolean;
  launchAtLogin: boolean;
  historyRetentionDays: number;
  updatedAt: string;
}

export interface DesktopSettingsUpdateRequest {
  expectedRevision: number;
  autoRefreshEnabled: boolean;
  refreshIntervalSeconds: 60 | 300 | 900;
  notificationThresholdBasisPoints: number | null;
  resetCreditNoticeHours: number;
  quietHoursEnabled: boolean;
  quietHoursStartMinute: number | null;
  quietHoursEndMinute: number | null;
  language: DesktopSettingsLanguage;
  lockScreenPrivacy: boolean;
  launchAtLogin: boolean;
  historyRetentionDays: number;
}

export interface DesktopSettingsEnvelope {
  schemaVersion: "1.0";
  status: "available" | "updated" | "revision_conflict" | "invalid_request" | "failed";
  reasonCode: string;
  settings: DesktopMonitorSettings | null;
}

export interface DesktopDeleteAllRequest {
  expectedSettingsRevision: number;
  confirmed: boolean;
}

export interface DesktopDeleteAllEnvelope {
  schemaVersion: "1.0";
  status: "deleted" | "confirmation_required" | "revision_conflict" | "invalid_request" | "failed";
  reasonCode: string;
  deletedRecordCount: number | null;
  settingsRevision: number | null;
}

export interface DesktopVaultMutationStatusEnvelope {
  schemaVersion: "1.0";
  status: "available" | "recovery_required" | "blocked" | "failed";
  reasonCode: string;
  coordination: {
    lockState: "unlocked" | "active" | "stale" | "unverifiable" | "unavailable";
    legacyViewerState: "not_running" | "running" | "unavailable";
  };
  catalog: {
    managedAccountCount: number;
    pendingAccountOperationCount: number;
    pendingKeyRotationCount: number;
    needsReviewCount: number;
  } | null;
  /** Inspection only: no user-triggered mutation command is exposed yet. */
  mutationCommandsEnabled: false;
  identifiersRedacted: true;
  pathsRedacted: true;
}

export interface DesktopDiagnosticsPreview {
  generatedAt: string;
  productVersion: string;
  platform: "macos" | "windows" | "linux" | "unknown";
  architecture: string;
  discovery: {
    outcome:
      | "selected"
      | "confirmation_required"
      | "ambiguous"
      | "not_found"
      | "verification_failed";
    candidateCount: number;
    selectedSourceCategories: string[];
    codexVersion: string | null;
    identityStrength: "os_file_id" | "metadata_fingerprint" | null;
  };
  monitor: {
    storeBackendAvailable: boolean;
    settingsAvailable: boolean;
    autoRefreshEnabled: boolean | null;
    refreshIntervalSeconds: 60 | 300 | 900 | null;
    notificationPrivacy: "generic" | "detailed" | null;
  };
  lastRead: {
    lifecycle: DesktopStatusEnvelope["lifecycle"];
    capturedAt: string;
    availability: Availability;
    freshness: Freshness;
    compatibility: Compatibility;
    quotaWindowCount: number;
    resetCreditSummary: "available" | "partial" | "unavailable";
    resetCreditDetails: "complete" | "partial" | "unavailable";
    usageAvailability: Availability;
    diagnosticCodes: string[];
  } | null;
  pathRedacted: true;
  accountRedacted: true;
  quotaValuesRedacted: true;
}

export interface DesktopDiagnosticsEnvelope {
  schemaVersion: "1.0";
  status: "available" | "partial" | "failed";
  reasonCode: string;
  preview: DesktopDiagnosticsPreview | null;
}

export const browserEnvelope: DesktopStatusEnvelope = {
  schemaVersion: "1.0",
  sequence: 0,
  lifecycle: "error",
  status: null,
  candidates: [],
  selectedExecutableId: null,
  issue: {
    code: "desktop_runtime_required",
    message: "Open the Tauri desktop app to read local Codex capacity.",
    retryAfterMs: null,
  },
  persistenceEnabled: false,
};

export const desktopPendingEnvelope: DesktopStatusEnvelope = {
  schemaVersion: "1.0",
  sequence: 0,
  lifecycle: "idle",
  status: null,
  candidates: [],
  selectedExecutableId: null,
  issue: null,
  persistenceEnabled: false,
};

export const desktopFailureEnvelope: DesktopStatusEnvelope = {
  ...desktopPendingEnvelope,
  lifecycle: "error",
  issue: {
    code: "desktop_runtime_failed",
    message: "The local desktop status service could not complete the request.",
    retryAfterMs: null,
  },
};

export function formatAxis(value: string): string {
  return value.replace(/_/g, " ");
}

export interface ActiveTimeInput {
  startedAt: string;
  endedAt: string;
  durationSeconds: number;
}
export interface ActiveTimer {
  timerId: string;
  revision: number;
  state: "running" | "paused" | "review" | "saved" | "discarded";
  startedAt: string;
  endedAt: string | null;
  suggestedSeconds: number;
  updatedAt: string;
  interrupted: boolean;
}
export interface ActiveTimeObservation {
  observationId: string;
  source: "user_timer" | "user_reported";
  quality: "user_confirmed";
  observation: ActiveTimeInput;
  included: boolean;
  revision: number;
  createdAt: string;
}
export type ActiveTimeAction =
  | { kind: "start"; timerId: string }
  | { kind: "pause" | "resume" | "finish" | "discard"; timerId: string; expectedRevision: number }
  | { kind: "confirm"; timerId: string; expectedRevision: number; observation: ActiveTimeInput }
  | { kind: "record"; observationId: string; observation: ActiveTimeInput }
  | { kind: "set_included"; observationId: string; expectedRevision: number; included: boolean };
export interface DesktopActiveTimeUpdate { historyContextId: string; action: ActiveTimeAction }
export interface DesktopActiveTimeEnvelope {
  schemaVersion: "1.0";
  historyContextId: string;
  status: "available" | "updated" | "unavailable" | "failed" | "revision_conflict" | "invalid_request";
  reasonCode: string;
  revision: number;
  timer: ActiveTimer | null;
  observations: ActiveTimeObservation[];
  includedCount30Days: number;
  includedSeconds30Days: number;
  hasOtherEnvironmentRecords: boolean;
  generatedAt: string;
}

export interface ActivityQuotaWindow {
  limitId: string;
  windowMinutes: number | null;
  reasonCode: string;
  sampleCount: number;
  firstSnapshotId: string;
  lastSnapshotId: string;
  observedFrom: string;
  observedUntil: string;
  firstRemainingPercent: number | null;
  lastRemainingPercent: number | null;
  observedDecreasePercent: number | null;
  coveragePercent: number;
  leadingGapSeconds: number;
  trailingGapSeconds: number;
  maximumGapSeconds: number;
  compatibilityUnverified: boolean;
}
export interface DesktopActiveTimeQuotaEnvelope {
  schemaVersion: "1.0";
  historyContextId: string;
  observationId: string;
  status: "available" | "missing" | "unavailable" | "invalid_request" | "failed";
  reasonCode: string;
  record: ActiveTimeObservation | null;
  comparison: {algorithmVersion: string; queryTruncated: boolean; windows: ActivityQuotaWindow[]} | null;
  generatedAt: string;
}
