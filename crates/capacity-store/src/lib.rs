//! SQLite persistence for stable-account quota history.
//!
//! The store deliberately has no representation for ephemeral account
//! bindings. A caller may pass one to [`CapacityStore::record_snapshot`], but
//! the result is an explicit skip and no row is written.

mod active_time;
mod activity_quota;
mod demand;
#[cfg(test)]
mod migration_tests;
mod pace;
mod pace_trial;
mod planning_archive;
pub use active_time::{ActiveTimeSnapshot, ActiveTimeWriteOutcome};
pub use activity_quota::ActivityQuotaEvidence;
pub use demand::{CapacityWorkPlan, CapacityWorkPlanUpdate, CapacityWorkPlanUpdateOutcome};
pub use planning_archive::{PlanningArchiveQuery, PlanningArchiveResult};

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

use capacity_domain::quota_change::{QuotaChangeObserver, QuotaChangeSample, QuotaChangeSummary};
use capacity_domain::{
    AccountBinding, AccountBindingStatus, AccountFingerprint, Availability, Compatibility,
    EnvironmentSnapshot, Freshness, InstallationKeyId, PlannerError, ResetCreditDetailsStatus,
    StatusSnapshot, SummaryStatus, UtcTimestamp, ValidationError, VaultAccount,
    VaultAccountAuthMode, VaultAccountId, VaultAccountLifecycle, VaultAccountRegistration,
    VaultAccountSource, VaultCatalogSnapshot, VaultEnvironmentState, VaultKeyRotation,
    VaultKeyRotationCheckpoint, VaultKeyRotationId, VaultKeyRotationStatus,
    VaultKeyRotationTransition, VaultOperation, VaultOperationCheckpoint, VaultOperationId,
    VaultOperationKind, VaultOperationStatus, VaultOperationTransition, VaultRecordRef,
    VaultValidationError, WorkSchedule, WorkSchedulePeriod,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use thiserror::Error;
use uuid::Uuid;

pub const STORE_SCHEMA_VERSION: u32 = 9;
pub const DEFAULT_SETTINGS_REVISION: u32 = 1;
pub const SUPPORTED_REFRESH_INTERVAL_SECONDS: [u32; 3] = [60, 300, 900];
const MIGRATION_V1: &str = include_str!("../../../schemas/store/v1/schema.sql");
const MIGRATION_V2: &str = include_str!("../../../schemas/store/v2/schema.sql");
const MIGRATION_V3: &str = include_str!("../../../schemas/store/v3/schema.sql");
const MIGRATION_V4: &str = include_str!("../../../schemas/store/v4/schema.sql");
const MIGRATION_V5: &str = include_str!("../../../schemas/store/v5/schema.sql");
const MIGRATION_V6: &str = include_str!("../../../schemas/store/v6/schema.sql");
const MIGRATION_V7: &str = include_str!("../../../schemas/store/v7/schema.sql");
const MIGRATION_V8: &str = include_str!("../../../schemas/store/v8/schema.sql");
const MIGRATION_V9: &str = include_str!("../../../schemas/store/v9/schema.sql");
const MAX_HISTORY_QUERY: u32 = 10_000;
const MAX_VAULT_ACCOUNTS: u32 = 1_024;
const MAX_ACTIVE_VAULT_OPERATIONS: u32 = 1_024;
const MAX_TOTAL_VAULT_OPERATIONS: u32 = 10_000;
const MAX_VAULT_RECOVERY_QUERY: u32 = 256;
const MAX_TOTAL_VAULT_KEY_ROTATIONS: u32 = 10_000;
const SCHEMA_V1_TABLE_COUNT: usize = 9;
const SCHEMA_V2_TABLE_COUNT: usize = 11;
const SCHEMA_V3_TABLE_COUNT: usize = 12;
const SCHEMA_V4_TABLE_COUNT: usize = 13;
const SCHEMA_V5_TABLE_COUNT: usize = 15;
const SCHEMA_V6_TABLE_COUNT: usize = 17;
const SCHEMA_V7_TABLE_COUNT: usize = 19;
const SCHEMA_V8_TABLE_COUNT: usize = 20;
const SCHEMA_COLUMNS: &[(&str, &[&str])] = &[
    ("schema_migrations", &["version", "applied_at"]),
    (
        "environments",
        &[
            "environment_id",
            "platform",
            "architecture",
            "boundary",
            "created_at",
            "last_seen_at",
        ],
    ),
    (
        "account_bindings",
        &[
            "environment_id",
            "account_fingerprint",
            "created_at",
            "last_seen_at",
        ],
    ),
    (
        "compatibility_observations",
        &[
            "observation_id",
            "environment_id",
            "executable_id",
            "codex_version",
            "protocol_schema_fingerprint",
            "compatibility",
            "observed_at",
        ],
    ),
    (
        "quota_snapshots",
        &[
            "snapshot_id",
            "environment_id",
            "account_fingerprint",
            "compatibility_observation_id",
            "status_schema_version",
            "captured_at",
            "availability",
            "freshness",
        ],
    ),
    (
        "quota_windows",
        &[
            "snapshot_id",
            "limit_id",
            "label",
            "window_minutes",
            "used_basis_points",
            "remaining_basis_points",
            "resets_at",
        ],
    ),
    (
        "reset_credit_summaries",
        &[
            "snapshot_id",
            "summary_status",
            "available_count",
            "details_status",
        ],
    ),
    (
        "reset_credits",
        &[
            "snapshot_id",
            "ordinal",
            "opaque_id",
            "reset_type",
            "status",
            "granted_at",
            "expires_at",
            "title",
            "description",
        ],
    ),
    (
        "settings",
        &[
            "singleton_id",
            "revision",
            "auto_refresh_enabled",
            "refresh_interval_seconds",
            "notification_threshold_basis_points",
            "reset_credit_notice_hours",
            "quiet_hours_enabled",
            "quiet_hours_start_minute",
            "quiet_hours_end_minute",
            "language",
            "lock_screen_privacy",
            "launch_at_login",
            "history_retention_days",
            "updated_at",
        ],
    ),
    (
        "vault_accounts",
        &[
            "account_id",
            "account_fingerprint",
            "protected_record_ref",
            "display_name",
            "auth_mode",
            "source",
            "lifecycle",
            "provider_id",
            "model",
            "revision",
            "display_order",
            "created_at",
            "updated_at",
            "last_used_at",
        ],
    ),
    (
        "vault_environment_states",
        &[
            "environment_id",
            "revision",
            "selected_account_id",
            "observed_account_id",
            "updated_at",
        ],
    ),
    (
        "vault_operations",
        &[
            "operation_id",
            "kind",
            "status",
            "checkpoint",
            "revision",
            "account_id",
            "account_fingerprint",
            "protected_record_ref",
            "display_name",
            "auth_mode",
            "source",
            "lifecycle",
            "provider_id",
            "model",
            "expected_account_revision",
            "last_error_code",
            "created_at",
            "updated_at",
        ],
    ),
    (
        "vault_key_rotations",
        &[
            "rotation_id",
            "status",
            "checkpoint",
            "revision",
            "source_key_id",
            "target_key_id",
            "expected_key_ring_revision",
            "last_error_code",
            "created_at",
            "updated_at",
        ],
    ),
    (
        "work_schedule_settings",
        &["singleton_id", "revision", "enabled", "updated_at"],
    ),
    (
        "work_schedule_periods",
        &[
            "singleton_id",
            "ordinal",
            "start_minute_of_day",
            "end_minute_of_day",
        ],
    ),
    (
        "capacity_demands",
        &[
            "environment_id",
            "account_fingerprint",
            "demand_id",
            "revision",
            "horizon_end",
            "demand_kind",
            "planned_codex_active_hours",
            "created_at",
        ],
    ),
    (
        "capacity_work_plans",
        &[
            "environment_id",
            "account_fingerprint",
            "work_plan_id",
            "revision",
            "demand_id",
            "demand_revision",
            "enabled",
            "created_at",
        ],
    ),
    (
        "active_time_timers",
        &[
            "timer_id",
            "environment_id",
            "account_fingerprint",
            "owner_context_id",
            "revision",
            "state",
            "started_at",
            "ended_at",
            "suggested_seconds",
            "updated_at",
            "interrupted",
        ],
    ),
    (
        "active_time_observations",
        &[
            "observation_id",
            "environment_id",
            "account_fingerprint",
            "source",
            "quality",
            "started_at",
            "ended_at",
            "duration_seconds",
            "included",
            "revision",
            "created_at",
        ],
    ),
    (
        "quota_snapshot_contexts",
        &["snapshot_id", "account_plan_type"],
    ),
    (
        "pace_trials",
        &[
            "trial_id",
            "environment_id",
            "account_fingerprint",
            "limit_id",
            "algorithm_version",
            "issued_at",
            "forecast_horizon",
            "inputs_json",
        ],
    ),
    (
        "pace_trial_outcomes",
        &["trial_id", "assessed_at", "outcome_json"],
    ),
];

#[derive(Debug)]
pub struct CapacityStore {
    connection: Connection,
    file_backed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityObservationInput {
    pub environment_id: String,
    pub executable_id: String,
    pub codex_version: Option<String>,
    pub protocol_schema_fingerprint: Option<String>,
    pub compatibility: Compatibility,
    pub observed_at: UtcTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotPersistenceOutcome {
    Persisted { snapshot_id: String },
    Skipped(SnapshotPersistenceSkip),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPersistenceSkip {
    EphemeralBinding,
    UnavailableBinding,
    NotLive,
    NonPersistableAvailability,
    NoQuotaWindows,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryPoint {
    pub snapshot_id: String,
    pub captured_at: UtcTimestamp,
    pub limit_id: String,
    pub label: Option<String>,
    pub window_minutes: Option<u64>,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at: Option<UtcTimestamp>,
    pub availability: Availability,
    pub compatibility: Compatibility,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistoryOverview {
    pub points: Vec<HistoryPoint>,
    pub sample_count: u64,
    pub daily_activity: Vec<HistoryDayActivity>,
    /// Computed from ordered raw captures, never the hourly chart reduction.
    pub quota_changes: QuotaChangeSummary,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistoryDayActivity {
    /// Calendar date in the operating system's timezone at query time.
    pub date: String,
    pub sample_count: u64,
    pub comparable_intervals: u64,
    /// Sum of observed decreases, not an estimate of unobserved usage.
    pub consumed_percent: f64,
}

#[derive(Default)]
struct HistoryHour {
    first: Option<(u64, HistoryPoint)>,
    lowest: Option<(u64, HistoryPoint)>,
    highest: Option<(u64, HistoryPoint)>,
    last: Option<(u64, HistoryPoint)>,
}

impl HistoryHour {
    fn observe(&mut self, index: u64, point: HistoryPoint) {
        if self.first.is_none() {
            self.first = Some((index, point.clone()));
        }
        if self
            .lowest
            .as_ref()
            .is_none_or(|(_, low)| point.remaining_percent < low.remaining_percent)
        {
            self.lowest = Some((index, point.clone()));
        }
        if self
            .highest
            .as_ref()
            .is_none_or(|(_, high)| point.remaining_percent > high.remaining_percent)
        {
            self.highest = Some((index, point.clone()));
        }
        self.last = Some((index, point));
    }

    fn finish(self, output: &mut Vec<HistoryPoint>) {
        let mut samples: Vec<_> = [self.first, self.lowest, self.highest, self.last]
            .into_iter()
            .flatten()
            .collect();
        samples.sort_by_key(|(index, _)| *index);
        samples.dedup_by_key(|(index, _)| *index);
        output.extend(samples.into_iter().map(|(_, point)| point));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsLanguage {
    System,
    English,
    SimplifiedChinese,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSettings {
    pub revision: u32,
    pub auto_refresh_enabled: bool,
    pub refresh_interval_seconds: u32,
    pub notification_threshold_basis_points: Option<u16>,
    pub reset_credit_notice_hours: u16,
    pub quiet_hours_enabled: bool,
    pub quiet_hours_start_minute: Option<u16>,
    pub quiet_hours_end_minute: Option<u16>,
    pub language: SettingsLanguage,
    pub lock_screen_privacy: bool,
    pub launch_at_login: bool,
    pub history_retention_days: u16,
    pub updated_at: UtcTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSettingsUpdate {
    pub expected_revision: u32,
    pub auto_refresh_enabled: bool,
    pub refresh_interval_seconds: u32,
    pub notification_threshold_basis_points: Option<u16>,
    pub reset_credit_notice_hours: u16,
    pub quiet_hours_enabled: bool,
    pub quiet_hours_start_minute: Option<u16>,
    pub quiet_hours_end_minute: Option<u16>,
    pub language: SettingsLanguage,
    pub lock_screen_privacy: bool,
    pub launch_at_login: bool,
    pub history_retention_days: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsUpdateOutcome {
    Updated(MonitorSettings),
    RevisionConflict(MonitorSettings),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScheduleSettings {
    pub revision: u32,
    pub schedule: WorkSchedule,
    pub updated_at: UtcTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScheduleSettingsUpdate {
    pub expected_revision: u32,
    pub enabled: bool,
    pub off_periods: Vec<WorkSchedulePeriod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkScheduleSettingsUpdateOutcome {
    Updated(WorkScheduleSettings),
    RevisionConflict(WorkScheduleSettings),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultAccountRegistrationOutcome {
    Created(VaultAccount),
    Existing(VaultAccount),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultOperationBeginOutcome {
    Created(VaultOperation),
    Existing(VaultOperation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultOperationAdvanceOutcome {
    Updated(VaultOperation),
    RevisionConflict(VaultOperation),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultKeyRotationBeginOutcome {
    Created(VaultKeyRotation),
    Existing(VaultKeyRotation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultKeyRotationAdvanceOutcome {
    Updated(VaultKeyRotation),
    RevisionConflict(VaultKeyRotation),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultAccountMutationOutcome {
    Updated(VaultAccount),
    RevisionConflict(VaultAccount),
    Missing,
}

/// Identifier-free inventory used by the desktop mutation host to decide
/// whether startup recovery or explicit review is required. Counts are read
/// from one SQLite snapshot; no account, operation, rotation, key, or record
/// identifier crosses this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaultMutationRecoverySummary {
    pub managed_accounts: u32,
    pub pending_account_operations: u32,
    pub account_operations_needing_review: u32,
    pub pending_key_rotations: u32,
    pub key_rotations_needing_review: u32,
}

impl VaultMutationRecoverySummary {
    pub fn needs_review_count(self) -> u32 {
        self.account_operations_needing_review
            .saturating_add(self.key_rotations_needing_review)
    }

    pub fn has_pending_work(self) -> bool {
        self.pending_account_operations > 0 || self.pending_key_rotations > 0
    }
}

/// Bounded SQLite-only dependency inventory for one installation-key ID.
/// Protected-record tags are audited by the vault layer and are intentionally
/// not represented here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultKeyDependencyAudit {
    pub vault_accounts: u32,
    pub account_bindings: u32,
    pub unmanaged_account_bindings: u32,
    pub quota_snapshots: u64,
    pub active_vault_operations: u32,
    pub active_key_rotations: u32,
}

impl VaultKeyDependencyAudit {
    pub fn has_dependencies(&self) -> bool {
        self.vault_accounts > 0
            || self.account_bindings > 0
            || self.quota_snapshots > 0
            || self.active_vault_operations > 0
            || self.active_key_rotations > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultFingerprintCascadeReport {
    pub account: VaultAccount,
    pub account_bindings: u32,
    pub quota_snapshots: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultFingerprintCascadeOutcome {
    Updated(VaultFingerprintCascadeReport),
    AlreadyApplied(VaultAccount),
    RevisionConflict(VaultAccount),
    BindingConflict,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultAccountRemoval {
    pub account: VaultAccount,
    pub cleared_environment_states: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultAccountRemovalOutcome {
    Removed(VaultAccountRemoval),
    RevisionConflict(VaultAccount),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultEnvironmentStateUpdateOutcome {
    Updated(VaultEnvironmentState),
    RevisionConflict(Option<VaultEnvironmentState>),
    MissingAccount(VaultAccountId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultReorderOutcome {
    Updated(Vec<VaultAccount>),
    CatalogConflict(Vec<VaultAccount>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteDatabaseReport {
    pub environments: u64,
    pub account_bindings: u64,
    pub compatibility_observations: u64,
    pub quota_snapshots: u64,
    pub quota_windows: u64,
    pub reset_credit_summaries: u64,
    pub reset_credits: u64,
    pub settings: u64,
    pub vault_accounts: u64,
    pub vault_environment_states: u64,
    pub vault_operations: u64,
    pub vault_key_rotations: u64,
    pub work_schedule_settings: u64,
    pub work_schedule_periods: u64,
    pub capacity_demands: u64,
    pub capacity_work_plans: u64,
    pub active_time_timers: u64,
    pub active_time_observations: u64,
    pub quota_snapshot_contexts: u64,
    pub pace_trials: u64,
    pub pace_trial_outcomes: u64,
}

impl DeleteDatabaseReport {
    pub fn total_rows(&self) -> u64 {
        self.environments
            + self.account_bindings
            + self.compatibility_observations
            + self.quota_snapshots
            + self.quota_windows
            + self.reset_credit_summaries
            + self.reset_credits
            + self.settings
            + self.vault_accounts
            + self.vault_environment_states
            + self.vault_operations
            + self.vault_key_rotations
            + self.work_schedule_settings
            + self.work_schedule_periods
            + self.capacity_demands
            + self.capacity_work_plans
            + self.active_time_timers
            + self.active_time_observations
            + self.quota_snapshot_contexts
            + self.pace_trials
            + self.pace_trial_outcomes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteDatabaseOutcome {
    Deleted(DeleteDatabaseReport),
    RevisionConflict(MonitorSettings),
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite operation failed")]
    Database(#[from] rusqlite::Error),
    #[error("database file operation failed")]
    Io(#[from] std::io::Error),
    #[error("status snapshot failed domain validation")]
    DomainValidation(#[from] ValidationError),
    #[error("account-vault input failed domain validation")]
    VaultValidation(#[from] VaultValidationError),
    #[error("work schedule failed domain validation")]
    PlannerValidation(#[from] PlannerError),
    #[error("explicit work demand failed domain validation")]
    DemandValidation(#[from] capacity_domain::demand::DemandError),
    #[error("explicit activity failed domain validation")]
    ActiveTimeValidation(#[from] capacity_domain::active_time::ActiveTimeError),
    #[error("database schema {found} is newer than supported schema {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("database file is not a regular file")]
    UnsafeDatabaseFileType,
    #[error("database file permissions are broader than the current OS user")]
    InsecureDatabasePermissions,
    #[error("stable binding does not match snapshot binding status")]
    BindingStatusMismatch,
    #[error("environment metadata changed for an existing environment_id")]
    EnvironmentConflict,
    #[error("compatibility observation does not match the snapshot boundary")]
    CompatibilityObservationMismatch,
    #[error("snapshot has no verified executable identity")]
    MissingExecutableIdentity,
    #[error("invalid bounded metadata field: {0}")]
    InvalidMetadata(&'static str),
    #[error("invalid monitor setting: {0}")]
    InvalidSettings(&'static str),
    #[error("invalid account-vault operation: {0}")]
    InvalidVaultOperation(&'static str),
    #[error("protected vault record reference is already assigned to another account")]
    VaultRecordConflict,
    #[error("account-vault catalog reached its bounded account limit")]
    VaultCatalogFull,
    #[error("an active vault operation conflicts with the requested mutation")]
    VaultOperationConflict,
    #[error("the bounded vault operation journal is full")]
    VaultOperationJournalFull,
    #[error("an active vault key rotation conflicts with the requested mutation")]
    VaultKeyRotationConflict,
    #[error("the bounded vault key rotation journal is full")]
    VaultKeyRotationJournalFull,
    #[error("numeric value exceeds SQLite integer range")]
    NumericOverflow,
    #[error("stored enum value is not recognized: {0}")]
    CorruptEnum(&'static str),
    #[error("SQLite schema or integrity invariant failed")]
    SchemaInvariant,
}

impl CapacityStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        prepare_database_file(path)?;
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )?;
        configure_connection(&connection, true)?;
        let mut store = Self {
            connection,
            file_backed: true,
        };
        store.migrate()?;
        store.verify_schema()?;
        verify_database_file(path)?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        configure_connection(&connection, false)?;
        let mut store = Self {
            connection,
            file_backed: false,
        };
        store.migrate()?;
        store.verify_schema()?;
        Ok(store)
    }

    pub fn schema_version(&self) -> Result<u32, StoreError> {
        let version = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
        Ok(version)
    }

    pub fn vault_catalog(&self, environment_id: &str) -> Result<VaultCatalogSnapshot, StoreError> {
        if !bounded_metadata(environment_id, 256) {
            return Err(StoreError::InvalidMetadata("environment_id"));
        }
        Ok(VaultCatalogSnapshot {
            accounts: read_vault_accounts(&self.connection)?,
            environment_state: read_vault_environment_state(&self.connection, environment_id)?,
        })
    }

    /// Returns the complete bounded managed-account catalog without requiring
    /// an environment selector. Key rotation is installation-scoped and must
    /// converge every account before an old key can be retired.
    pub fn vault_accounts(&self) -> Result<Vec<VaultAccount>, StoreError> {
        read_vault_accounts(&self.connection)
    }

    /// Returns an aggregate recovery inventory without materializing any
    /// sensitive catalog or journal row in the desktop host.
    pub fn vault_mutation_recovery_summary(
        &self,
    ) -> Result<VaultMutationRecoverySummary, StoreError> {
        let counts = self.connection.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM vault_accounts),
                 (SELECT COUNT(*) FROM vault_operations
                  WHERE status IN ('in_progress', 'needs_review')),
                 (SELECT COUNT(*) FROM vault_operations
                  WHERE status = 'needs_review'),
                 (SELECT COUNT(*) FROM vault_key_rotations
                  WHERE status IN ('in_progress', 'rolling_back', 'needs_review')),
                 (SELECT COUNT(*) FROM vault_key_rotations
                  WHERE status = 'needs_review')",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        Ok(VaultMutationRecoverySummary {
            managed_accounts: u32::try_from(counts.0).map_err(|_| StoreError::NumericOverflow)?,
            pending_account_operations: u32::try_from(counts.1)
                .map_err(|_| StoreError::NumericOverflow)?,
            account_operations_needing_review: u32::try_from(counts.2)
                .map_err(|_| StoreError::NumericOverflow)?,
            pending_key_rotations: u32::try_from(counts.3)
                .map_err(|_| StoreError::NumericOverflow)?,
            key_rotations_needing_review: u32::try_from(counts.4)
                .map_err(|_| StoreError::NumericOverflow)?,
        })
    }

    pub fn vault_account(
        &self,
        account_id: &VaultAccountId,
    ) -> Result<Option<VaultAccount>, StoreError> {
        read_vault_account(&self.connection, account_id)
    }

    pub fn vault_account_by_fingerprint(
        &self,
        account_fingerprint: &AccountFingerprint,
    ) -> Result<Option<VaultAccount>, StoreError> {
        read_vault_account_by_fingerprint(&self.connection, account_fingerprint)
    }

    pub fn vault_account_by_record_ref(
        &self,
        protected_record_ref: &VaultRecordRef,
    ) -> Result<Option<VaultAccount>, StoreError> {
        read_vault_account_where(
            &self.connection,
            "protected_record_ref",
            protected_record_ref.as_str(),
        )
    }

    /// Counts every SQLite reference whose HMAC fingerprint was produced by
    /// `key_id`. This is one half of the old-key retirement gate; callers must
    /// combine it with a protected-record tag inventory before removing a key.
    pub fn vault_key_dependency_audit(
        &self,
        key_id: &str,
    ) -> Result<VaultKeyDependencyAudit, StoreError> {
        let key_id = InstallationKeyId::parse(key_id.to_owned())
            .map_err(|_| StoreError::InvalidVaultOperation("installation_key_id"))?;
        self.vault_key_dependency_audit_excluding_rotation(&key_id, None)
    }

    /// Performs the old-key retirement audit while excluding the rotation
    /// that owns the retirement attempt. The excluded row must exist and must
    /// name `key_id`; callers cannot suppress an unrelated active rotation.
    pub fn vault_key_dependency_audit_for_rotation(
        &self,
        key_id: &InstallationKeyId,
        rotation_id: &VaultKeyRotationId,
    ) -> Result<VaultKeyDependencyAudit, StoreError> {
        let rotation = read_vault_key_rotation(&self.connection, rotation_id)?.ok_or(
            StoreError::InvalidVaultOperation("key_rotation_dependency_audit"),
        )?;
        if rotation.source_key_id != *key_id && rotation.target_key_id != *key_id {
            return Err(StoreError::InvalidVaultOperation(
                "key_rotation_dependency_audit",
            ));
        }
        self.vault_key_dependency_audit_excluding_rotation(key_id, Some(rotation_id))
    }

    fn vault_key_dependency_audit_excluding_rotation(
        &self,
        key_id: &InstallationKeyId,
        excluded_rotation_id: Option<&VaultKeyRotationId>,
    ) -> Result<VaultKeyDependencyAudit, StoreError> {
        let vault_accounts = count_fingerprint_key_dependencies(
            &self.connection,
            "SELECT COUNT(*) FROM vault_accounts
             WHERE substr(account_fingerprint, 16, 36) = ?1",
            key_id.as_str(),
        )?;
        let account_bindings = count_fingerprint_key_dependencies(
            &self.connection,
            "SELECT COUNT(*) FROM account_bindings
             WHERE substr(account_fingerprint, 16, 36) = ?1",
            key_id.as_str(),
        )?;
        let unmanaged_account_bindings = count_fingerprint_key_dependencies(
            &self.connection,
            "SELECT COUNT(*)
             FROM account_bindings AS bindings
             WHERE substr(bindings.account_fingerprint, 16, 36) = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM vault_accounts AS accounts
                   WHERE accounts.account_fingerprint = bindings.account_fingerprint
               )",
            key_id.as_str(),
        )?;
        let quota_snapshots = count_fingerprint_key_dependencies(
            &self.connection,
            "SELECT COUNT(*) FROM quota_snapshots
             WHERE substr(account_fingerprint, 16, 36) = ?1",
            key_id.as_str(),
        )?;
        let active_vault_operations = count_fingerprint_key_dependencies(
            &self.connection,
            "SELECT COUNT(*) FROM vault_operations
             WHERE status IN ('in_progress', 'needs_review')
               AND substr(account_fingerprint, 16, 36) = ?1",
            key_id.as_str(),
        )?;
        let active_key_rotations: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM vault_key_rotations
             WHERE status IN ('in_progress', 'rolling_back', 'needs_review')
               AND (source_key_id = ?1 OR target_key_id = ?1)
               AND (?2 IS NULL OR rotation_id != ?2)",
            params![
                key_id.as_str(),
                excluded_rotation_id.map(VaultKeyRotationId::as_str),
            ],
            |row| row.get(0),
        )?;
        Ok(VaultKeyDependencyAudit {
            vault_accounts: u32::try_from(vault_accounts)
                .map_err(|_| StoreError::NumericOverflow)?,
            account_bindings: u32::try_from(account_bindings)
                .map_err(|_| StoreError::NumericOverflow)?,
            unmanaged_account_bindings: u32::try_from(unmanaged_account_bindings)
                .map_err(|_| StoreError::NumericOverflow)?,
            quota_snapshots,
            active_vault_operations: u32::try_from(active_vault_operations)
                .map_err(|_| StoreError::NumericOverflow)?,
            active_key_rotations: u32::try_from(active_key_rotations)
                .map_err(|_| StoreError::NumericOverflow)?,
        })
    }

    pub fn vault_key_rotation(
        &self,
        rotation_id: &VaultKeyRotationId,
    ) -> Result<Option<VaultKeyRotation>, StoreError> {
        read_vault_key_rotation(&self.connection, rotation_id)
    }

    /// Persists the immutable rotation intent before either key material or
    /// fingerprints are changed. Only one active rotation may exist, and it
    /// is mutually exclusive with account registration/forget journals.
    pub fn begin_vault_key_rotation(
        &mut self,
        source_key_id: &InstallationKeyId,
        target_key_id: &InstallationKeyId,
        expected_key_ring_revision: u64,
        created_at: &UtcTimestamp,
    ) -> Result<VaultKeyRotationBeginOutcome, StoreError> {
        if expected_key_ring_revision == 0 {
            return Err(StoreError::InvalidVaultOperation(
                "expected_key_ring_revision",
            ));
        }
        let rotation = VaultKeyRotation {
            rotation_id: new_vault_key_rotation_id()?,
            status: VaultKeyRotationStatus::InProgress,
            checkpoint: VaultKeyRotationCheckpoint::Prepared,
            revision: 1,
            source_key_id: source_key_id.clone(),
            target_key_id: target_key_id.clone(),
            expected_key_ring_revision,
            last_error_code: None,
            created_at: created_at.clone(),
            updated_at: created_at.clone(),
        };
        rotation.validate()?;
        let expected_key_ring_revision =
            i64::try_from(expected_key_ring_revision).map_err(|_| StoreError::NumericOverflow)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        let active_operation_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM vault_operations
             WHERE status IN ('in_progress', 'needs_review')",
            [],
            |row| row.get(0),
        )?;
        if active_operation_count != 0 {
            return Err(StoreError::VaultKeyRotationConflict);
        }

        let active = read_active_vault_key_rotations(&transaction)?;
        if active.len() > 1 {
            return Err(StoreError::SchemaInvariant);
        }
        if let Some(existing) = active.into_iter().next() {
            return if same_vault_key_rotation_request(&existing, &rotation) {
                Ok(VaultKeyRotationBeginOutcome::Existing(existing))
            } else {
                Err(StoreError::VaultKeyRotationConflict)
            };
        }
        if table_count(&transaction, "vault_key_rotations")?
            >= u64::from(MAX_TOTAL_VAULT_KEY_ROTATIONS)
        {
            return Err(StoreError::VaultKeyRotationJournalFull);
        }

        insert_vault_key_rotation(&transaction, &rotation, expected_key_ring_revision)?;
        transaction.commit()?;
        Ok(VaultKeyRotationBeginOutcome::Created(rotation))
    }

    /// Returns active rotations in deterministic creation order. The schema
    /// currently permits one, while a bounded API keeps recovery stable if a
    /// future migration raises that limit.
    pub fn recoverable_vault_key_rotations(
        &self,
        limit: u32,
    ) -> Result<Vec<VaultKeyRotation>, StoreError> {
        if limit == 0 || limit > MAX_VAULT_RECOVERY_QUERY {
            return Err(StoreError::InvalidVaultOperation("recovery_limit"));
        }
        read_recoverable_vault_key_rotations(&self.connection, limit)
    }

    pub fn advance_vault_key_rotation(
        &mut self,
        rotation_id: &VaultKeyRotationId,
        expected_revision: u32,
        transition: VaultKeyRotationTransition,
        updated_at: &UtcTimestamp,
    ) -> Result<VaultKeyRotationAdvanceOutcome, StoreError> {
        validate_expected_vault_revision(expected_revision)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(current) = read_vault_key_rotation(&transaction, rotation_id)? else {
            return Ok(VaultKeyRotationAdvanceOutcome::Missing);
        };
        if current.revision != expected_revision {
            return Ok(VaultKeyRotationAdvanceOutcome::RevisionConflict(current));
        }
        let next = current.apply_transition(transition, updated_at.clone())?;
        let changed = transaction.execute(
            "UPDATE vault_key_rotations
             SET status = ?1,
                 checkpoint = ?2,
                 revision = ?3,
                 last_error_code = ?4,
                 updated_at = ?5
             WHERE rotation_id = ?6 AND revision = ?7",
            params![
                vault_key_rotation_status_as_str(next.status),
                vault_key_rotation_checkpoint_as_str(next.checkpoint),
                i64::from(next.revision),
                next.last_error_code,
                next.updated_at.as_str(),
                rotation_id.as_str(),
                i64::from(expected_revision),
            ],
        )?;
        if changed != 1 {
            let current = read_vault_key_rotation(&transaction, rotation_id)?
                .ok_or(StoreError::SchemaInvariant)?;
            return Ok(VaultKeyRotationAdvanceOutcome::RevisionConflict(current));
        }
        transaction.commit()?;
        Ok(VaultKeyRotationAdvanceOutcome::Updated(next))
    }

    pub fn prune_terminal_vault_key_rotations_before(
        &mut self,
        before: &UtcTimestamp,
    ) -> Result<u64, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = transaction.execute(
            "DELETE FROM vault_key_rotations
             WHERE status IN ('succeeded', 'compensated')
               AND julianday(updated_at) < julianday(?1)",
            [before.as_str()],
        )?;
        transaction.commit()?;
        u64::try_from(deleted).map_err(|_| StoreError::NumericOverflow)
    }

    pub fn vault_operation(
        &self,
        operation_id: &VaultOperationId,
    ) -> Result<Option<VaultOperation>, StoreError> {
        read_vault_operation(&self.connection, operation_id)
    }

    /// Resolves the one active journal owner for a protected-record reference.
    /// Multiple rows indicate schema/coordination drift and fail closed.
    pub fn active_vault_operation_by_record_ref(
        &self,
        protected_record_ref: &VaultRecordRef,
    ) -> Result<Option<VaultOperation>, StoreError> {
        let operations =
            read_active_vault_operations_by_record_ref(&self.connection, protected_record_ref)?;
        if operations.len() > 1 {
            return Err(StoreError::SchemaInvariant);
        }
        Ok(operations.into_iter().next())
    }

    /// Starts or resumes the durable coordination record for protected-record
    /// creation followed by metadata registration. Only an exact active
    /// request is idempotent; aliasing any immutable identity fails closed.
    pub fn begin_vault_registration_operation(
        &mut self,
        registration: &VaultAccountRegistration,
        created_at: &UtcTimestamp,
    ) -> Result<VaultOperationBeginOutcome, StoreError> {
        registration.validate()?;
        let operation = VaultOperation {
            operation_id: new_vault_operation_id()?,
            kind: VaultOperationKind::RegisterAccount,
            status: VaultOperationStatus::InProgress,
            checkpoint: VaultOperationCheckpoint::Prepared,
            revision: 1,
            account_id: None,
            account_fingerprint: registration.account_fingerprint.clone(),
            protected_record_ref: registration.protected_record_ref.clone(),
            display_name: registration.display_name.clone(),
            auth_mode: registration.auth_mode,
            source: registration.source,
            lifecycle: registration.lifecycle,
            provider_id: registration.provider_id.clone(),
            model: registration.model.clone(),
            expected_account_revision: None,
            last_error_code: None,
            created_at: created_at.clone(),
            updated_at: created_at.clone(),
        };
        self.begin_vault_operation(operation)
    }

    /// Starts or resumes a quarantine-first metadata removal operation from a
    /// caller-held account snapshot. The account revision is journaled so a
    /// recovery pass cannot remove metadata that changed after intent capture.
    pub fn begin_vault_forget_operation(
        &mut self,
        account: &VaultAccount,
        created_at: &UtcTimestamp,
    ) -> Result<VaultOperationBeginOutcome, StoreError> {
        let operation = VaultOperation {
            operation_id: new_vault_operation_id()?,
            kind: VaultOperationKind::ForgetAccount,
            status: VaultOperationStatus::InProgress,
            checkpoint: VaultOperationCheckpoint::Prepared,
            revision: 1,
            account_id: Some(account.account_id.clone()),
            account_fingerprint: account.account_fingerprint.clone(),
            protected_record_ref: account.protected_record_ref.clone(),
            display_name: account.display_name.clone(),
            auth_mode: account.auth_mode,
            source: account.source,
            lifecycle: account.lifecycle,
            provider_id: account.provider_id.clone(),
            model: account.model.clone(),
            expected_account_revision: Some(account.revision),
            last_error_code: None,
            created_at: created_at.clone(),
            updated_at: created_at.clone(),
        };
        self.begin_vault_operation(operation)
    }

    fn begin_vault_operation(
        &mut self,
        operation: VaultOperation,
    ) -> Result<VaultOperationBeginOutcome, StoreError> {
        operation.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active_rotation_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM vault_key_rotations
             WHERE status IN ('in_progress', 'rolling_back', 'needs_review')",
            [],
            |row| row.get(0),
        )?;
        if active_rotation_count != 0 {
            return Err(StoreError::VaultOperationConflict);
        }
        let matches = read_active_vault_operations_matching(&transaction, &operation)?;
        if matches.len() > 1 {
            return Err(StoreError::SchemaInvariant);
        }
        if let Some(existing) = matches.into_iter().next() {
            return if same_vault_operation_request(&existing, &operation) {
                Ok(VaultOperationBeginOutcome::Existing(existing))
            } else {
                Err(StoreError::VaultOperationConflict)
            };
        }

        let active_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM vault_operations
             WHERE status IN ('in_progress', 'needs_review')",
            [],
            |row| row.get(0),
        )?;
        let total_count = table_count(&transaction, "vault_operations")?;
        if active_count >= i64::from(MAX_ACTIVE_VAULT_OPERATIONS)
            || total_count >= u64::from(MAX_TOTAL_VAULT_OPERATIONS)
        {
            return Err(StoreError::VaultOperationJournalFull);
        }

        insert_vault_operation(&transaction, &operation)?;
        transaction.commit()?;
        Ok(VaultOperationBeginOutcome::Created(operation))
    }

    /// Returns a bounded deterministic recovery queue. Terminal operations are
    /// never replayed; `needs_review` remains visible beside resumable work.
    pub fn recoverable_vault_operations(
        &self,
        limit: u32,
    ) -> Result<Vec<VaultOperation>, StoreError> {
        if limit == 0 || limit > MAX_VAULT_RECOVERY_QUERY {
            return Err(StoreError::InvalidVaultOperation("recovery_limit"));
        }
        read_recoverable_vault_operations(&self.connection, limit)
    }

    /// Applies a pure domain transition under an optimistic revision guard.
    /// A stale worker receives the current journal row and performs no write.
    pub fn advance_vault_operation(
        &mut self,
        operation_id: &VaultOperationId,
        expected_revision: u32,
        transition: VaultOperationTransition,
        updated_at: &UtcTimestamp,
    ) -> Result<VaultOperationAdvanceOutcome, StoreError> {
        validate_expected_vault_revision(expected_revision)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(current) = read_vault_operation(&transaction, operation_id)? else {
            return Ok(VaultOperationAdvanceOutcome::Missing);
        };
        if current.revision != expected_revision {
            return Ok(VaultOperationAdvanceOutcome::RevisionConflict(current));
        }
        let next = current.apply_transition(transition, updated_at.clone())?;
        let changed = transaction.execute(
            "UPDATE vault_operations
             SET status = ?1,
                 checkpoint = ?2,
                 revision = ?3,
                 account_id = ?4,
                 last_error_code = ?5,
                 updated_at = ?6
             WHERE operation_id = ?7 AND revision = ?8",
            params![
                vault_operation_status_as_str(next.status),
                vault_operation_checkpoint_as_str(next.checkpoint),
                i64::from(next.revision),
                next.account_id.as_ref().map(VaultAccountId::as_str),
                next.last_error_code,
                next.updated_at.as_str(),
                operation_id.as_str(),
                i64::from(expected_revision),
            ],
        )?;
        if changed != 1 {
            let current = read_vault_operation(&transaction, operation_id)?
                .ok_or(StoreError::SchemaInvariant)?;
            return Ok(VaultOperationAdvanceOutcome::RevisionConflict(current));
        }
        transaction.commit()?;
        Ok(VaultOperationAdvanceOutcome::Updated(next))
    }

    /// Removes only old terminal journal rows. Active or review-required work
    /// survives retention and must be explicitly recovered or compensated.
    pub fn prune_terminal_vault_operations_before(
        &mut self,
        before: &UtcTimestamp,
    ) -> Result<u64, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = transaction.execute(
            "DELETE FROM vault_operations
             WHERE status IN ('succeeded', 'compensated')
               AND julianday(updated_at) < julianday(?1)",
            [before.as_str()],
        )?;
        transaction.commit()?;
        u64::try_from(deleted).map_err(|_| StoreError::NumericOverflow)
    }

    /// Registers metadata for a protected record that has already been
    /// allocated by a vault backend. A matching fingerprint is an idempotent
    /// no-op; a protected-record reference assigned elsewhere fails closed.
    pub fn register_vault_account(
        &mut self,
        registration: &VaultAccountRegistration,
        created_at: &UtcTimestamp,
    ) -> Result<VaultAccountRegistrationOutcome, StoreError> {
        registration.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing) =
            read_vault_account_by_fingerprint(&transaction, &registration.account_fingerprint)?
        {
            return Ok(VaultAccountRegistrationOutcome::Existing(existing));
        }
        let record_owner: Option<String> = transaction
            .query_row(
                "SELECT account_id FROM vault_accounts WHERE protected_record_ref = ?1",
                [registration.protected_record_ref.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        if record_owner.is_some() {
            return Err(StoreError::VaultRecordConflict);
        }

        let account_count = table_count(&transaction, "vault_accounts")?;
        if account_count >= u64::from(MAX_VAULT_ACCOUNTS) {
            return Err(StoreError::VaultCatalogFull);
        }
        let display_order: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(display_order) + 1, 0) FROM vault_accounts",
            [],
            |row| row.get(0),
        )?;
        let account_id = VaultAccountId::parse(format!("account:v1:{}", Uuid::new_v4()))?;
        transaction.execute(
            "INSERT INTO vault_accounts(
                 account_id,
                 account_fingerprint,
                 protected_record_ref,
                 display_name,
                 auth_mode,
                 source,
                 lifecycle,
                 provider_id,
                 model,
                 revision,
                 display_order,
                 created_at,
                 updated_at,
                 last_used_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11, ?11, NULL)",
            params![
                account_id.as_str(),
                registration.account_fingerprint.as_str(),
                registration.protected_record_ref.as_str(),
                registration.display_name,
                vault_auth_mode_as_str(registration.auth_mode),
                vault_source_as_str(registration.source),
                vault_lifecycle_as_str(registration.lifecycle),
                registration.provider_id,
                registration.model,
                display_order,
                created_at.as_str(),
            ],
        )?;
        let account =
            read_vault_account(&transaction, &account_id)?.ok_or(StoreError::SchemaInvariant)?;
        transaction.commit()?;
        Ok(VaultAccountRegistrationOutcome::Created(account))
    }

    pub fn rename_vault_account(
        &mut self,
        account_id: &VaultAccountId,
        expected_revision: u32,
        display_name: &str,
        updated_at: &UtcTimestamp,
    ) -> Result<VaultAccountMutationOutcome, StoreError> {
        validate_vault_mutation(expected_revision, display_name, 128, "display_name")?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE vault_accounts
             SET display_name = ?1, revision = revision + 1, updated_at = ?2
             WHERE account_id = ?3 AND revision = ?4",
            params![
                display_name,
                updated_at.as_str(),
                account_id.as_str(),
                i64::from(expected_revision),
            ],
        )?;
        vault_account_mutation_outcome(transaction, account_id, changed)
    }

    pub fn note_vault_account_used(
        &mut self,
        account_id: &VaultAccountId,
        expected_revision: u32,
        used_at: &UtcTimestamp,
    ) -> Result<VaultAccountMutationOutcome, StoreError> {
        validate_expected_vault_revision(expected_revision)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE vault_accounts
             SET last_used_at = ?1, revision = revision + 1, updated_at = ?1
             WHERE account_id = ?2 AND revision = ?3",
            params![
                used_at.as_str(),
                account_id.as_str(),
                i64::from(expected_revision),
            ],
        )?;
        vault_account_mutation_outcome(transaction, account_id, changed)
    }

    pub fn set_vault_account_lifecycle(
        &mut self,
        account_id: &VaultAccountId,
        expected_revision: u32,
        lifecycle: VaultAccountLifecycle,
        updated_at: &UtcTimestamp,
    ) -> Result<VaultAccountMutationOutcome, StoreError> {
        validate_expected_vault_revision(expected_revision)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE vault_accounts
             SET lifecycle = ?1, revision = revision + 1, updated_at = ?2
             WHERE account_id = ?3 AND revision = ?4",
            params![
                vault_lifecycle_as_str(lifecycle),
                updated_at.as_str(),
                account_id.as_str(),
                i64::from(expected_revision),
            ],
        )?;
        vault_account_mutation_outcome(transaction, account_id, changed)
    }

    /// Atomically changes one managed account's HMAC fingerprint and every
    /// matching persistent history binding. `quota_snapshots` follows through
    /// its `ON UPDATE CASCADE` foreign key; exact pre/post counts make schema
    /// drift or a partial cascade a transaction failure.
    ///
    /// The protected record must already be verifiable by both rotation keys.
    /// Cross-store ordering and recovery are owned by the higher journaled
    /// rotation service, which must hold the product mutation lease.
    pub fn cascade_vault_account_fingerprint(
        &mut self,
        expected_account: &VaultAccount,
        replacement: &AccountFingerprint,
        updated_at: &UtcTimestamp,
    ) -> Result<VaultFingerprintCascadeOutcome, StoreError> {
        if expected_account.revision == 0
            || expected_account.account_fingerprint == *replacement
            || expected_account.account_fingerprint.key_id() == replacement.key_id()
        {
            return Err(StoreError::InvalidVaultOperation("rotation_fingerprint"));
        }
        if !updated_at.is_not_before(&expected_account.updated_at) {
            return Err(StoreError::InvalidVaultOperation("updated_at"));
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(current) = read_vault_account(&transaction, &expected_account.account_id)? else {
            return Ok(VaultFingerprintCascadeOutcome::Missing);
        };
        if current.account_fingerprint == *replacement {
            let mut exact_retry = expected_account.clone();
            exact_retry.account_fingerprint = replacement.clone();
            exact_retry.revision = exact_retry
                .revision
                .checked_add(1)
                .ok_or(StoreError::NumericOverflow)?;
            exact_retry.updated_at = updated_at.clone();
            return Ok(if current == exact_retry {
                VaultFingerprintCascadeOutcome::AlreadyApplied(current)
            } else {
                VaultFingerprintCascadeOutcome::RevisionConflict(current)
            });
        }
        if current != *expected_account {
            return Ok(VaultFingerprintCascadeOutcome::RevisionConflict(current));
        }

        let active_operation_count = count_fingerprint_key_dependencies(
            &transaction,
            "SELECT COUNT(*) FROM vault_operations
             WHERE status IN ('in_progress', 'needs_review')
               AND account_fingerprint = ?1",
            expected_account.account_fingerprint.as_str(),
        )?;
        if active_operation_count != 0 {
            return Err(StoreError::VaultOperationConflict);
        }
        if read_vault_account_by_fingerprint(&transaction, replacement)?.is_some() {
            return Ok(VaultFingerprintCascadeOutcome::BindingConflict);
        }
        let binding_collision_count = transaction.query_row(
            "SELECT COUNT(*)
             FROM account_bindings AS old_binding
             JOIN account_bindings AS new_binding
               ON new_binding.environment_id = old_binding.environment_id
              AND new_binding.account_fingerprint = ?2
             WHERE old_binding.account_fingerprint = ?1",
            params![
                expected_account.account_fingerprint.as_str(),
                replacement.as_str(),
            ],
            |row| row.get::<_, i64>(0),
        )?;
        if binding_collision_count != 0 {
            return Ok(VaultFingerprintCascadeOutcome::BindingConflict);
        }

        let old_bindings = count_exact_fingerprint(
            &transaction,
            "account_bindings",
            &expected_account.account_fingerprint,
        )?;
        let old_snapshots = count_exact_fingerprint(
            &transaction,
            "quota_snapshots",
            &expected_account.account_fingerprint,
        )?;
        let new_bindings_before =
            count_exact_fingerprint(&transaction, "account_bindings", replacement)?;
        let new_snapshots_before =
            count_exact_fingerprint(&transaction, "quota_snapshots", replacement)?;

        let changed_bindings = transaction.execute(
            "UPDATE account_bindings
             SET account_fingerprint = ?1
             WHERE account_fingerprint = ?2",
            params![
                replacement.as_str(),
                expected_account.account_fingerprint.as_str(),
            ],
        )?;
        if u64::try_from(changed_bindings).map_err(|_| StoreError::NumericOverflow)? != old_bindings
        {
            return Err(StoreError::SchemaInvariant);
        }
        let changed_account = transaction.execute(
            "UPDATE vault_accounts
             SET account_fingerprint = ?1,
                 revision = revision + 1,
                 updated_at = ?2
             WHERE account_id = ?3
               AND revision = ?4
               AND account_fingerprint = ?5
               AND protected_record_ref = ?6",
            params![
                replacement.as_str(),
                updated_at.as_str(),
                expected_account.account_id.as_str(),
                i64::from(expected_account.revision),
                expected_account.account_fingerprint.as_str(),
                expected_account.protected_record_ref.as_str(),
            ],
        )?;
        if changed_account != 1 {
            return Err(StoreError::SchemaInvariant);
        }

        let expected_new_bindings = new_bindings_before
            .checked_add(old_bindings)
            .ok_or(StoreError::NumericOverflow)?;
        let expected_new_snapshots = new_snapshots_before
            .checked_add(old_snapshots)
            .ok_or(StoreError::NumericOverflow)?;
        if count_exact_fingerprint(
            &transaction,
            "account_bindings",
            &expected_account.account_fingerprint,
        )? != 0
            || count_exact_fingerprint(
                &transaction,
                "quota_snapshots",
                &expected_account.account_fingerprint,
            )? != 0
            || count_exact_fingerprint(&transaction, "account_bindings", replacement)?
                != expected_new_bindings
            || count_exact_fingerprint(&transaction, "quota_snapshots", replacement)?
                != expected_new_snapshots
        {
            return Err(StoreError::SchemaInvariant);
        }
        let mut foreign_key_check = transaction.prepare("PRAGMA foreign_key_check")?;
        if foreign_key_check.query([])?.next()?.is_some() {
            return Err(StoreError::SchemaInvariant);
        }
        drop(foreign_key_check);

        let account = read_vault_account(&transaction, &expected_account.account_id)?
            .ok_or(StoreError::SchemaInvariant)?;
        let mut expected_updated = expected_account.clone();
        expected_updated.account_fingerprint = replacement.clone();
        expected_updated.revision = expected_updated
            .revision
            .checked_add(1)
            .ok_or(StoreError::NumericOverflow)?;
        expected_updated.updated_at = updated_at.clone();
        if account != expected_updated {
            return Err(StoreError::SchemaInvariant);
        }
        transaction.commit()?;
        Ok(VaultFingerprintCascadeOutcome::Updated(
            VaultFingerprintCascadeReport {
                account,
                account_bindings: u32::try_from(old_bindings)
                    .map_err(|_| StoreError::NumericOverflow)?,
                quota_snapshots: old_snapshots,
            },
        ))
    }

    /// Removes ordinary SQLite metadata only and returns the protected record
    /// reference inside `account`. A higher account-vault service must wrap
    /// this with its protected-backend delete/restore transaction; this method
    /// never claims that credential material was deleted.
    pub fn remove_vault_account_metadata(
        &mut self,
        account_id: &VaultAccountId,
        expected_revision: u32,
        removed_at: &UtcTimestamp,
    ) -> Result<VaultAccountRemovalOutcome, StoreError> {
        validate_expected_vault_revision(expected_revision)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(account) = read_vault_account(&transaction, account_id)? else {
            return Ok(VaultAccountRemovalOutcome::Missing);
        };
        if account.revision != expected_revision {
            return Ok(VaultAccountRemovalOutcome::RevisionConflict(account));
        }
        let cleared = transaction.execute(
            "UPDATE vault_environment_states
             SET selected_account_id = CASE
                     WHEN selected_account_id = ?1 THEN NULL ELSE selected_account_id END,
                 observed_account_id = CASE
                     WHEN observed_account_id = ?1 THEN NULL ELSE observed_account_id END,
                 revision = revision + 1,
                 updated_at = ?2
             WHERE selected_account_id = ?1 OR observed_account_id = ?1",
            params![account_id.as_str(), removed_at.as_str()],
        )?;
        let deleted = transaction.execute(
            "DELETE FROM vault_accounts WHERE account_id = ?1 AND revision = ?2",
            params![account_id.as_str(), i64::from(expected_revision)],
        )?;
        if deleted != 1 {
            return Err(StoreError::SchemaInvariant);
        }
        transaction.commit()?;
        Ok(VaultAccountRemovalOutcome::Removed(VaultAccountRemoval {
            account,
            cleared_environment_states: u32::try_from(cleared)
                .map_err(|_| StoreError::NumericOverflow)?,
        }))
    }

    pub fn reorder_vault_accounts(
        &mut self,
        ordered_account_ids: &[VaultAccountId],
        updated_at: &UtcTimestamp,
    ) -> Result<VaultReorderOutcome, StoreError> {
        if ordered_account_ids.len() > MAX_VAULT_ACCOUNTS as usize {
            return Err(StoreError::VaultCatalogFull);
        }
        let requested: BTreeSet<&str> = ordered_account_ids
            .iter()
            .map(VaultAccountId::as_str)
            .collect();
        if requested.len() != ordered_account_ids.len() {
            return Err(StoreError::InvalidVaultOperation("duplicate_account_id"));
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_vault_accounts(&transaction)?;
        let current_ids: BTreeSet<&str> = current
            .iter()
            .map(|account| account.account_id.as_str())
            .collect();
        if current_ids != requested {
            return Ok(VaultReorderOutcome::CatalogConflict(current));
        }
        if current
            .iter()
            .map(|account| &account.account_id)
            .eq(ordered_account_ids.iter())
        {
            return Ok(VaultReorderOutcome::Updated(current));
        }
        let maximum_order: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(display_order), -1) FROM vault_accounts",
            [],
            |row| row.get(0),
        )?;
        let requested_len =
            i64::try_from(ordered_account_ids.len()).map_err(|_| StoreError::NumericOverflow)?;
        let offset = maximum_order
            .checked_add(requested_len)
            .and_then(|value| value.checked_add(1))
            .ok_or(StoreError::NumericOverflow)?;
        transaction.execute(
            "UPDATE vault_accounts SET display_order = display_order + ?1",
            [offset],
        )?;
        for (display_order, account_id) in ordered_account_ids.iter().enumerate() {
            let display_order =
                i64::try_from(display_order).map_err(|_| StoreError::NumericOverflow)?;
            let changed = transaction.execute(
                "UPDATE vault_accounts
                 SET display_order = ?1, revision = revision + 1, updated_at = ?2
                 WHERE account_id = ?3",
                params![display_order, updated_at.as_str(), account_id.as_str(),],
            )?;
            if changed != 1 {
                return Err(StoreError::SchemaInvariant);
            }
        }
        let reordered = read_vault_accounts(&transaction)?;
        transaction.commit()?;
        Ok(VaultReorderOutcome::Updated(reordered))
    }

    pub fn set_vault_environment_state(
        &mut self,
        environment: &EnvironmentSnapshot,
        expected_revision: u32,
        selected_account_id: Option<&VaultAccountId>,
        observed_account_id: Option<&VaultAccountId>,
        updated_at: &UtcTimestamp,
    ) -> Result<VaultEnvironmentStateUpdateOutcome, StoreError> {
        validate_environment_metadata(environment)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for account_id in [selected_account_id, observed_account_id]
            .into_iter()
            .flatten()
        {
            if read_vault_account(&transaction, account_id)?.is_none() {
                return Ok(VaultEnvironmentStateUpdateOutcome::MissingAccount(
                    account_id.clone(),
                ));
            }
        }
        persist_environment_metadata(&transaction, environment, updated_at)?;

        let current = read_vault_environment_state(&transaction, &environment.environment_id)?;
        match current {
            None if expected_revision == 0 => {
                transaction.execute(
                    "INSERT INTO vault_environment_states(
                         environment_id, revision, selected_account_id, observed_account_id, updated_at
                     ) VALUES (?1, 1, ?2, ?3, ?4)",
                    params![
                        environment.environment_id,
                        selected_account_id.map(VaultAccountId::as_str),
                        observed_account_id.map(VaultAccountId::as_str),
                        updated_at.as_str(),
                    ],
                )?;
            }
            None => {
                return Ok(VaultEnvironmentStateUpdateOutcome::RevisionConflict(None));
            }
            Some(current) if current.revision != expected_revision => {
                return Ok(VaultEnvironmentStateUpdateOutcome::RevisionConflict(Some(
                    current,
                )));
            }
            Some(_) => {
                let changed = transaction.execute(
                    "UPDATE vault_environment_states
                     SET revision = revision + 1,
                         selected_account_id = ?1,
                         observed_account_id = ?2,
                         updated_at = ?3
                     WHERE environment_id = ?4 AND revision = ?5",
                    params![
                        selected_account_id.map(VaultAccountId::as_str),
                        observed_account_id.map(VaultAccountId::as_str),
                        updated_at.as_str(),
                        environment.environment_id,
                        i64::from(expected_revision),
                    ],
                )?;
                if changed != 1 {
                    return Err(StoreError::SchemaInvariant);
                }
            }
        }
        let state = read_vault_environment_state(&transaction, &environment.environment_id)?
            .ok_or(StoreError::SchemaInvariant)?;
        transaction.commit()?;
        Ok(VaultEnvironmentStateUpdateOutcome::Updated(state))
    }

    pub fn record_snapshot(
        &mut self,
        binding: &AccountBinding,
        snapshot: &StatusSnapshot,
        observation: &CompatibilityObservationInput,
    ) -> Result<SnapshotPersistenceOutcome, StoreError> {
        snapshot.validate()?;

        let fingerprint = match binding {
            AccountBinding::Stable(fingerprint) => fingerprint,
            AccountBinding::Ephemeral(_) => {
                return Ok(SnapshotPersistenceOutcome::Skipped(
                    SnapshotPersistenceSkip::EphemeralBinding,
                ));
            }
            AccountBinding::Unavailable => {
                return Ok(SnapshotPersistenceOutcome::Skipped(
                    SnapshotPersistenceSkip::UnavailableBinding,
                ));
            }
        };

        if snapshot
            .account
            .as_ref()
            .map(|account| account.binding_status)
            != Some(AccountBindingStatus::Stable)
        {
            return Err(StoreError::BindingStatusMismatch);
        }
        if snapshot.data_status.freshness != Freshness::Live {
            return Ok(SnapshotPersistenceOutcome::Skipped(
                SnapshotPersistenceSkip::NotLive,
            ));
        }
        if !matches!(
            snapshot.data_status.availability,
            Availability::Complete | Availability::Partial
        ) {
            return Ok(SnapshotPersistenceOutcome::Skipped(
                SnapshotPersistenceSkip::NonPersistableAvailability,
            ));
        }
        if snapshot.quota.windows.is_empty() {
            return Ok(SnapshotPersistenceOutcome::Skipped(
                SnapshotPersistenceSkip::NoQuotaWindows,
            ));
        }

        validate_snapshot_metadata(snapshot, observation)?;

        let observation_id = Uuid::new_v4().to_string();
        let snapshot_id = Uuid::new_v4().to_string();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        persist_environment(&transaction, snapshot)?;
        persist_account_binding(&transaction, snapshot, fingerprint)?;
        persist_compatibility_observation(&transaction, observation, &observation_id)?;
        persist_quota_snapshot(
            &transaction,
            snapshot,
            fingerprint,
            &observation_id,
            &snapshot_id,
        )?;
        persist_quota_context(&transaction, snapshot, &snapshot_id)?;
        persist_quota_windows(&transaction, snapshot, &snapshot_id)?;
        persist_reset_credits(&transaction, snapshot, &snapshot_id)?;

        transaction.commit()?;
        Ok(SnapshotPersistenceOutcome::Persisted { snapshot_id })
    }

    pub fn history(
        &self,
        environment_id: &str,
        account_fingerprint: &AccountFingerprint,
        limit_id: &str,
        maximum_points: u32,
    ) -> Result<Vec<HistoryPoint>, StoreError> {
        if !bounded_metadata(environment_id, 256) {
            return Err(StoreError::InvalidMetadata("environment_id"));
        }
        if !bounded_metadata(limit_id, 128) {
            return Err(StoreError::InvalidMetadata("limit_id"));
        }
        let maximum_points = maximum_points.clamp(1, MAX_HISTORY_QUERY);
        let mut statement = self.connection.prepare(
            "SELECT
                 snapshots.snapshot_id,
                 snapshots.captured_at,
                 windows.limit_id,
                 windows.label,
                 windows.window_minutes,
                 windows.used_basis_points,
                 windows.remaining_basis_points,
                 windows.resets_at,
                 snapshots.availability,
                 observations.compatibility
             FROM quota_snapshots AS snapshots
             JOIN quota_windows AS windows
               ON windows.snapshot_id = snapshots.snapshot_id
             JOIN compatibility_observations AS observations
               ON observations.observation_id = snapshots.compatibility_observation_id
              AND observations.environment_id = snapshots.environment_id
             WHERE snapshots.environment_id = ?1
               AND snapshots.account_fingerprint = ?2
               AND windows.limit_id = ?3
             ORDER BY julianday(snapshots.captured_at) DESC, snapshots.snapshot_id DESC
             LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![
                environment_id,
                account_fingerprint.as_str(),
                limit_id,
                i64::from(maximum_points)
            ],
            |row| {
                Ok(HistoryRow {
                    snapshot_id: row.get(0)?,
                    captured_at: row.get(1)?,
                    limit_id: row.get(2)?,
                    label: row.get(3)?,
                    window_minutes: row.get(4)?,
                    used_basis_points: row.get(5)?,
                    remaining_basis_points: row.get(6)?,
                    resets_at: row.get(7)?,
                    availability: row.get(8)?,
                    compatibility: row.get(9)?,
                })
            },
        )?;

        rows.map(|row| history_point_from_row(row?)).collect()
    }

    /// A time-bounded view, independent of capture frequency. Keep original
    /// first/last/min/max observations per UTC hour (at most 2,884 points),
    /// while computing activity from every raw observation before reduction.
    /// Existing raw history and the stored observations are never modified.
    pub fn history_overview(
        &self,
        environment_id: &str,
        account_fingerprint: &AccountFingerprint,
        limit_id: &str,
    ) -> Result<HistoryOverview, StoreError> {
        self.history_overview_scoped(environment_id, account_fingerprint, limit_id, None)
    }

    /// Native desktop history follows the same account across local Codex binary
    /// updates. Per-executable IDs remain stored for provenance and strict APIs.
    /// Only runtime-generated IDs with identical platform/architecture/native
    /// metadata can share this read scope; all other environments stay exact.
    pub fn local_codex_history_overview(
        &self,
        environment_id: &str,
        account_fingerprint: &AccountFingerprint,
        limit_id: &str,
    ) -> Result<HistoryOverview, StoreError> {
        if !bounded_metadata(environment_id, 256) {
            return Err(StoreError::InvalidMetadata("environment_id"));
        }
        let native_family = self.connection.query_row(
            "SELECT platform, architecture, boundary FROM environments WHERE environment_id = ?1",
            [environment_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        ).optional()?.filter(|(platform, _, boundary)| {
            boundary == "native" && native_codex_environment_id(environment_id, platform)
        });
        self.history_overview_scoped(
            environment_id,
            account_fingerprint,
            limit_id,
            native_family
                .as_ref()
                .map(|(platform, architecture, _)| (platform.as_str(), architecture.as_str())),
        )
    }

    fn history_overview_scoped(
        &self,
        environment_id: &str,
        account_fingerprint: &AccountFingerprint,
        limit_id: &str,
        native_family: Option<(&str, &str)>,
    ) -> Result<HistoryOverview, StoreError> {
        if !bounded_metadata(environment_id, 256) {
            return Err(StoreError::InvalidMetadata("environment_id"));
        }
        if !bounded_metadata(limit_id, 128) {
            return Err(StoreError::InvalidMetadata("limit_id"));
        }
        let mut statement = self.connection.prepare(
            "WITH scoped AS (
                SELECT snapshots.snapshot_id, snapshots.captured_at,
                    windows.limit_id, windows.label, windows.window_minutes,
                    windows.used_basis_points, windows.remaining_basis_points,
                    windows.resets_at, snapshots.availability, observations.compatibility
                FROM quota_snapshots AS snapshots
                JOIN quota_windows AS windows ON windows.snapshot_id = snapshots.snapshot_id
                JOIN compatibility_observations AS observations
                    ON observations.observation_id = snapshots.compatibility_observation_id
                    AND observations.environment_id = snapshots.environment_id
                WHERE (snapshots.environment_id = ?1 OR (
                    ?4 IS NOT NULL AND EXISTS (
                        SELECT 1 FROM environments AS environment
                        WHERE environment.environment_id = snapshots.environment_id
                            AND environment.platform = ?4 AND environment.architecture = ?5
                            AND environment.boundary = 'native'
                            AND environment.environment_id GLOB (?4 || ':local:codex-executable-*')
                            AND length(environment.environment_id) = length(?4 || ':local:codex-executable-') + 16
                            AND substr(environment.environment_id, -16) NOT GLOB '*[^0-9a-f]*'
                    )
                )) AND snapshots.account_fingerprint = ?2
                    AND windows.limit_id = ?3
            )
            SELECT *, strftime('%Y-%m-%dT%H', captured_at),
                date(captured_at, 'localtime'), unixepoch(captured_at), unixepoch(resets_at)
            FROM scoped
            WHERE julianday(captured_at) >= (SELECT MAX(julianday(captured_at)) - 30 FROM scoped)
            ORDER BY julianday(captured_at), snapshot_id",
        )?;
        let mut rows = statement.query(params![
            environment_id,
            account_fingerprint.as_str(),
            limit_id,
            native_family.map(|(platform, _)| platform),
            native_family.map(|(_, architecture)| architecture),
        ])?;
        let mut overview = HistoryOverview::default();
        let mut changes = QuotaChangeObserver::default();
        let mut daily: BTreeMap<String, (HistoryDayActivity, i64)> = BTreeMap::new();
        let mut hour_key = String::new();
        let mut hour = HistoryHour::default();
        // time, local day, boundary, window duration, remaining basis points
        let mut previous: Option<(i64, String, Option<i64>, Option<i64>, i64)> = None;
        while let Some(row) = rows.next()? {
            let raw = HistoryRow {
                snapshot_id: row.get(0)?,
                captured_at: row.get(1)?,
                limit_id: row.get(2)?,
                label: row.get(3)?,
                window_minutes: row.get(4)?,
                used_basis_points: row.get(5)?,
                remaining_basis_points: row.get(6)?,
                resets_at: row.get(7)?,
                availability: row.get(8)?,
                compatibility: row.get(9)?,
            };
            let next_hour: String = row.get(10)?;
            let day: String = row.get(11)?;
            let epoch: i64 = row.get(12)?;
            let boundary: Option<i64> = row.get(13)?;
            let remaining = raw.remaining_basis_points;
            let window = raw.window_minutes;
            let point = history_point_from_row(raw)?;
            changes.observe(QuotaChangeSample {
                snapshot_id: point.snapshot_id.clone(),
                captured_at: point.captured_at.clone(),
                limit_id: point.limit_id.clone(),
                window_minutes: point.window_minutes,
                remaining_percent: point.remaining_percent,
                resets_at: point.resets_at.clone(),
            });
            let (activity, consumed_basis_points) = daily.entry(day.clone()).or_insert_with(|| {
                (
                    HistoryDayActivity {
                        date: day.clone(),
                        ..Default::default()
                    },
                    0,
                )
            });
            activity.sample_count += 1;
            if let Some((time, previous_day, reset, previous_window, previous_remaining)) =
                &previous
            {
                // No allocation across midnight, missing/changed windows, duplicate
                // instants or an observation gap longer than one hour.
                if day == *previous_day
                    && boundary.is_some()
                    && boundary == *reset
                    && window == *previous_window
                    && epoch > *time
                    && epoch - *time <= 3600
                {
                    activity.comparable_intervals += 1;
                    *consumed_basis_points += (*previous_remaining - remaining).max(0);
                }
            }
            previous = Some((epoch, day, boundary, window, remaining));
            if next_hour != hour_key {
                std::mem::take(&mut hour).finish(&mut overview.points);
                hour_key = next_hour;
            }
            hour.observe(overview.sample_count, point);
            overview.sample_count += 1;
        }
        hour.finish(&mut overview.points);
        overview.quota_changes = changes.finish();
        overview.daily_activity = daily
            .into_values()
            .map(|(mut activity, basis_points)| {
                activity.consumed_percent = basis_points as f64 / 100.0;
                activity
            })
            .collect();
        Ok(overview)
    }

    pub fn settings(&self) -> Result<MonitorSettings, StoreError> {
        read_monitor_settings(&self.connection)
    }

    pub fn update_settings(
        &mut self,
        update: &MonitorSettingsUpdate,
        updated_at: &UtcTimestamp,
    ) -> Result<SettingsUpdateOutcome, StoreError> {
        validate_settings_update(update)?;
        let expected_revision = i64::from(update.expected_revision);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE settings SET
                 revision = revision + 1,
                 auto_refresh_enabled = ?1,
                 refresh_interval_seconds = ?2,
                 notification_threshold_basis_points = ?3,
                 reset_credit_notice_hours = ?4,
                 quiet_hours_enabled = ?5,
                 quiet_hours_start_minute = ?6,
                 quiet_hours_end_minute = ?7,
                 language = ?8,
                 lock_screen_privacy = ?9,
                 launch_at_login = ?10,
                 history_retention_days = ?11,
                 updated_at = ?12
             WHERE singleton_id = 1 AND revision = ?13",
            params![
                bool_to_integer(update.auto_refresh_enabled),
                i64::from(update.refresh_interval_seconds),
                update.notification_threshold_basis_points.map(i64::from),
                i64::from(update.reset_credit_notice_hours),
                bool_to_integer(update.quiet_hours_enabled),
                update.quiet_hours_start_minute.map(i64::from),
                update.quiet_hours_end_minute.map(i64::from),
                settings_language_as_str(update.language),
                bool_to_integer(update.lock_screen_privacy),
                bool_to_integer(update.launch_at_login),
                i64::from(update.history_retention_days),
                updated_at.as_str(),
                expected_revision,
            ],
        )?;
        let current = read_monitor_settings(&transaction)?;
        if changed == 0 {
            return Ok(SettingsUpdateOutcome::RevisionConflict(current));
        }
        transaction.commit()?;
        Ok(SettingsUpdateOutcome::Updated(current))
    }

    pub fn work_schedule(&self) -> Result<WorkScheduleSettings, StoreError> {
        read_work_schedule(&self.connection)
    }

    pub fn update_work_schedule(
        &mut self,
        update: &WorkScheduleSettingsUpdate,
        updated_at: &UtcTimestamp,
    ) -> Result<WorkScheduleSettingsUpdateOutcome, StoreError> {
        if update.expected_revision == 0 {
            return Err(StoreError::InvalidSettings(
                "work_schedule_expected_revision",
            ));
        }
        let schedule = WorkSchedule::new(update.enabled, update.off_periods.clone())?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE work_schedule_settings
             SET revision = revision + 1, enabled = ?1, updated_at = ?2
             WHERE singleton_id = 1 AND revision = ?3",
            params![
                bool_to_integer(schedule.enabled),
                updated_at.as_str(),
                i64::from(update.expected_revision),
            ],
        )?;
        if changed == 0 {
            return Ok(WorkScheduleSettingsUpdateOutcome::RevisionConflict(
                read_work_schedule(&transaction)?,
            ));
        }
        transaction.execute(
            "DELETE FROM work_schedule_periods WHERE singleton_id = 1",
            [],
        )?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO work_schedule_periods(
                     singleton_id, ordinal, start_minute_of_day, end_minute_of_day
                 ) VALUES (1, ?1, ?2, ?3)",
            )?;
            for (ordinal, period) in schedule.off_periods.iter().enumerate() {
                statement.execute(params![
                    i64::try_from(ordinal).map_err(|_| StoreError::NumericOverflow)?,
                    i64::from(period.start_minute_of_day),
                    i64::from(period.end_minute_of_day),
                ])?;
            }
        }
        let current = read_work_schedule(&transaction)?;
        transaction.commit()?;
        Ok(WorkScheduleSettingsUpdateOutcome::Updated(current))
    }

    pub fn prune_history_before(
        &mut self,
        environment_id: &str,
        account_fingerprint: &AccountFingerprint,
        before: &UtcTimestamp,
    ) -> Result<u64, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = transaction.execute(
            "DELETE FROM quota_snapshots
             WHERE environment_id = ?1
               AND account_fingerprint = ?2
               AND julianday(captured_at) < julianday(?3)",
            params![
                environment_id,
                account_fingerprint.as_str(),
                before.as_str()
            ],
        )?;
        transaction.execute("DELETE FROM pace_trials WHERE environment_id=?1 AND account_fingerprint=?2 AND julianday(issued_at)<julianday(?3)", params![environment_id, account_fingerprint.as_str(), before.as_str()])?;
        transaction.execute(
            "DELETE FROM compatibility_observations
             WHERE NOT EXISTS (
                 SELECT 1 FROM quota_snapshots
                 WHERE quota_snapshots.compatibility_observation_id = compatibility_observations.observation_id
             )",
            [],
        )?;
        transaction.commit()?;
        u64::try_from(deleted).map_err(|_| StoreError::NumericOverflow)
    }

    /// Deletes every user-owned row in the database and restores only default
    /// settings plus migration metadata. The application layer remains
    /// responsible for diagnostics, feed caches, and app-owned backups.
    pub fn delete_all_database_data(&mut self) -> Result<DeleteDatabaseReport, StoreError> {
        match self.delete_database_data(None)? {
            DeleteDatabaseOutcome::Deleted(report) => Ok(report),
            DeleteDatabaseOutcome::RevisionConflict(_) => Err(StoreError::SchemaInvariant),
        }
    }

    pub fn delete_all_database_data_if_revision(
        &mut self,
        expected_settings_revision: u32,
    ) -> Result<DeleteDatabaseOutcome, StoreError> {
        self.delete_database_data(Some(expected_settings_revision))
    }

    fn delete_database_data(
        &mut self,
        expected_settings_revision: Option<u32>,
    ) -> Result<DeleteDatabaseOutcome, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_settings = read_monitor_settings(&transaction)?;
        if expected_settings_revision.is_some_and(|expected| expected != current_settings.revision)
        {
            return Ok(DeleteDatabaseOutcome::RevisionConflict(current_settings));
        }
        let report = DeleteDatabaseReport {
            environments: table_count(&transaction, "environments")?,
            account_bindings: table_count(&transaction, "account_bindings")?,
            compatibility_observations: table_count(&transaction, "compatibility_observations")?,
            quota_snapshots: table_count(&transaction, "quota_snapshots")?,
            quota_windows: table_count(&transaction, "quota_windows")?,
            reset_credit_summaries: table_count(&transaction, "reset_credit_summaries")?,
            reset_credits: table_count(&transaction, "reset_credits")?,
            settings: table_count(&transaction, "settings")?,
            vault_accounts: table_count(&transaction, "vault_accounts")?,
            vault_environment_states: table_count(&transaction, "vault_environment_states")?,
            vault_operations: table_count(&transaction, "vault_operations")?,
            vault_key_rotations: table_count(&transaction, "vault_key_rotations")?,
            work_schedule_settings: table_count(&transaction, "work_schedule_settings")?,
            work_schedule_periods: table_count(&transaction, "work_schedule_periods")?,
            capacity_demands: table_count(&transaction, "capacity_demands")?,
            capacity_work_plans: table_count(&transaction, "capacity_work_plans")?,
            active_time_timers: table_count(&transaction, "active_time_timers")?,
            active_time_observations: table_count(&transaction, "active_time_observations")?,
            quota_snapshot_contexts: table_count(&transaction, "quota_snapshot_contexts")?,
            pace_trials: table_count(&transaction, "pace_trials")?,
            pace_trial_outcomes: table_count(&transaction, "pace_trial_outcomes")?,
        };

        transaction.execute("DELETE FROM work_schedule_settings", [])?;
        transaction.execute("DELETE FROM environments", [])?;
        transaction.execute("DELETE FROM vault_key_rotations", [])?;
        transaction.execute("DELETE FROM vault_operations", [])?;
        transaction.execute("DELETE FROM vault_accounts", [])?;
        transaction.execute("DELETE FROM settings", [])?;
        insert_default_settings(&transaction)?;
        insert_default_work_schedule(&transaction)?;
        transaction.commit()?;

        self.connection.execute_batch("VACUUM;")?;
        if self.file_backed {
            self.connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        self.verify_schema()?;
        Ok(DeleteDatabaseOutcome::Deleted(report))
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let current = self.schema_version()?;
        if current > STORE_SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema {
                found: current,
                supported: STORE_SCHEMA_VERSION,
            });
        }
        if current == STORE_SCHEMA_VERSION {
            return Ok(());
        }
        if current == 1 {
            self.verify_schema_version(1, &SCHEMA_COLUMNS[..SCHEMA_V1_TABLE_COUNT])?;
        }
        if current == 2 {
            self.verify_schema_version(2, &SCHEMA_COLUMNS[..SCHEMA_V2_TABLE_COUNT])?;
        }
        if current == 3 {
            self.verify_schema_version(3, &SCHEMA_COLUMNS[..SCHEMA_V3_TABLE_COUNT])?;
        }
        if current == 4 {
            self.verify_schema_version(4, &SCHEMA_COLUMNS[..SCHEMA_V4_TABLE_COUNT])?;
        }
        if current == 5 {
            self.verify_schema_version(5, &SCHEMA_COLUMNS[..SCHEMA_V5_TABLE_COUNT])?;
        }
        if current == 6 {
            self.verify_schema_version(6, &SCHEMA_COLUMNS[..SCHEMA_V6_TABLE_COUNT])?;
        }
        if current == 7 {
            self.verify_schema_version(7, &SCHEMA_COLUMNS[..SCHEMA_V7_TABLE_COUNT])?;
        }
        if current == 8 {
            self.verify_schema_version(8, &SCHEMA_COLUMNS[..SCHEMA_V8_TABLE_COUNT])?;
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if current < 1 {
            transaction.execute_batch(MIGRATION_V1)?;
            record_schema_migration(&transaction, 1)?;
        }
        if current < 2 {
            transaction.execute_batch(MIGRATION_V2)?;
            record_schema_migration(&transaction, 2)?;
        }
        if current < 3 {
            transaction.execute_batch(MIGRATION_V3)?;
            record_schema_migration(&transaction, 3)?;
        }
        if current < 4 {
            transaction.execute_batch(MIGRATION_V4)?;
            record_schema_migration(&transaction, 4)?;
        }
        if current < 5 {
            transaction.execute_batch(MIGRATION_V5)?;
            record_schema_migration(&transaction, 5)?;
        }
        if current < 6 {
            transaction.execute_batch(MIGRATION_V6)?;
            record_schema_migration(&transaction, 6)?;
        }
        if current < 7 {
            transaction.execute_batch(MIGRATION_V7)?;
            record_schema_migration(&transaction, 7)?;
        }
        if current < 8 {
            transaction.execute_batch(MIGRATION_V8)?;
            record_schema_migration(&transaction, 8)?;
        }
        if current < 9 {
            transaction.execute_batch(MIGRATION_V9)?;
            record_schema_migration(&transaction, 9)?;
        }
        transaction.pragma_update(None, "user_version", STORE_SCHEMA_VERSION)?;
        // SQL success is not the migration postcondition. Validate while all
        // schema, ledger and data changes can still be rolled back together.
        Self::verify_connection_schema_version(&transaction, STORE_SCHEMA_VERSION, SCHEMA_COLUMNS)?;
        transaction.commit()?;
        Ok(())
    }

    fn verify_schema(&self) -> Result<(), StoreError> {
        self.verify_schema_version(STORE_SCHEMA_VERSION, SCHEMA_COLUMNS)
    }

    fn verify_schema_version(
        &self,
        expected_version: u32,
        schema_columns: &[(&str, &[&str])],
    ) -> Result<(), StoreError> {
        Self::verify_connection_schema_version(&self.connection, expected_version, schema_columns)
    }

    fn verify_connection_schema_version(
        connection: &Connection,
        expected_version: u32,
        schema_columns: &[(&str, &[&str])],
    ) -> Result<(), StoreError> {
        let actual_version: u32 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if actual_version != expected_version {
            return Err(StoreError::SchemaInvariant);
        }
        let expected_tables: BTreeSet<&str> =
            schema_columns.iter().map(|(table, _)| *table).collect();
        let mut table_statement = connection.prepare(
            "SELECT name, strict
             FROM pragma_table_list
             WHERE schema = 'main'
               AND type = 'table'
               AND name NOT LIKE 'sqlite_%'",
        )?;
        let table_rows = table_statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut actual_tables = BTreeSet::new();
        for row in table_rows {
            let (table, strict) = row?;
            if strict != 1 {
                return Err(StoreError::SchemaInvariant);
            }
            actual_tables.insert(table);
        }
        if actual_tables
            != expected_tables
                .iter()
                .map(|table| (*table).to_owned())
                .collect()
        {
            return Err(StoreError::SchemaInvariant);
        }

        for (table, expected_columns) in schema_columns {
            let sql = format!("PRAGMA table_info({table})");
            let mut statement = connection.prepare(&sql)?;
            let columns: Vec<String> = statement
                .query_map([], |row| row.get(1))?
                .collect::<Result<_, _>>()?;
            if columns != *expected_columns {
                return Err(StoreError::SchemaInvariant);
            }
        }

        let (migration_rows, minimum_migration, maximum_migration): (i64, i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(MIN(version), 0), COALESCE(MAX(version), 0)
                 FROM schema_migrations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        let settings_rows: i64 =
            connection.query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))?;
        let integrity: String =
            connection.pragma_query_value(None, "quick_check", |row| row.get(0))?;
        let mut foreign_key_check = connection.prepare("PRAGMA foreign_key_check")?;
        let has_foreign_key_violation = foreign_key_check.query([])?.next()?.is_some();
        if migration_rows != i64::from(expected_version)
            || minimum_migration != 1
            || maximum_migration != i64::from(expected_version)
            || settings_rows != 1
            || integrity != "ok"
            || has_foreign_key_violation
        {
            return Err(StoreError::SchemaInvariant);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct HistoryRow {
    snapshot_id: String,
    captured_at: String,
    limit_id: String,
    label: Option<String>,
    window_minutes: Option<i64>,
    used_basis_points: i64,
    remaining_basis_points: i64,
    resets_at: Option<String>,
    availability: String,
    compatibility: String,
}

#[derive(Debug)]
struct SettingsRow {
    revision: i64,
    auto_refresh_enabled: i64,
    refresh_interval_seconds: i64,
    notification_threshold_basis_points: Option<i64>,
    reset_credit_notice_hours: i64,
    quiet_hours_enabled: i64,
    quiet_hours_start_minute: Option<i64>,
    quiet_hours_end_minute: Option<i64>,
    language: String,
    lock_screen_privacy: i64,
    launch_at_login: i64,
    history_retention_days: i64,
    updated_at: String,
}

#[derive(Debug)]
struct VaultAccountRow {
    account_id: String,
    account_fingerprint: String,
    protected_record_ref: String,
    display_name: String,
    auth_mode: String,
    source: String,
    lifecycle: String,
    provider_id: Option<String>,
    model: Option<String>,
    revision: i64,
    display_order: i64,
    created_at: String,
    updated_at: String,
    last_used_at: Option<String>,
}

#[derive(Debug)]
struct VaultEnvironmentStateRow {
    environment_id: String,
    revision: i64,
    selected_account_id: Option<String>,
    observed_account_id: Option<String>,
    updated_at: String,
}

#[derive(Debug)]
struct VaultOperationRow {
    operation_id: String,
    kind: String,
    status: String,
    checkpoint: String,
    revision: i64,
    account_id: Option<String>,
    account_fingerprint: String,
    protected_record_ref: String,
    display_name: String,
    auth_mode: String,
    source: String,
    lifecycle: String,
    provider_id: Option<String>,
    model: Option<String>,
    expected_account_revision: Option<i64>,
    last_error_code: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct VaultKeyRotationRow {
    rotation_id: String,
    status: String,
    checkpoint: String,
    revision: i64,
    source_key_id: String,
    target_key_id: String,
    expected_key_ring_revision: i64,
    last_error_code: Option<String>,
    created_at: String,
    updated_at: String,
}

fn configure_connection(connection: &Connection, file_backed: bool) -> Result<(), StoreError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA trusted_schema = OFF;
         PRAGMA secure_delete = ON;
         PRAGMA synchronous = FULL;",
    )?;
    if file_backed {
        connection.execute_batch("PRAGMA journal_mode = WAL;")?;
    }
    Ok(())
}

fn record_schema_migration(transaction: &Transaction<'_>, version: u32) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO schema_migrations(version, applied_at)
         VALUES (?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        [version],
    )?;
    Ok(())
}

const VAULT_OPERATION_SELECT: &str = "SELECT
         operation_id,
         kind,
         status,
         checkpoint,
         revision,
         account_id,
         account_fingerprint,
         protected_record_ref,
         display_name,
         auth_mode,
         source,
         lifecycle,
         provider_id,
         model,
         expected_account_revision,
         last_error_code,
         created_at,
         updated_at
     FROM vault_operations";

fn new_vault_operation_id() -> Result<VaultOperationId, StoreError> {
    VaultOperationId::parse(format!("vault-operation:v1:{}", Uuid::new_v4())).map_err(Into::into)
}

fn insert_vault_operation(
    transaction: &Transaction<'_>,
    operation: &VaultOperation,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "INSERT INTO vault_operations(
             operation_id,
             kind,
             status,
             checkpoint,
             revision,
             account_id,
             account_fingerprint,
             protected_record_ref,
             display_name,
             auth_mode,
             source,
             lifecycle,
             provider_id,
             model,
             expected_account_revision,
             last_error_code,
             created_at,
             updated_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
             ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
         )",
        params![
            operation.operation_id.as_str(),
            vault_operation_kind_as_str(operation.kind),
            vault_operation_status_as_str(operation.status),
            vault_operation_checkpoint_as_str(operation.checkpoint),
            i64::from(operation.revision),
            operation.account_id.as_ref().map(VaultAccountId::as_str),
            operation.account_fingerprint.as_str(),
            operation.protected_record_ref.as_str(),
            operation.display_name,
            vault_auth_mode_as_str(operation.auth_mode),
            vault_source_as_str(operation.source),
            vault_lifecycle_as_str(operation.lifecycle),
            operation.provider_id,
            operation.model,
            operation.expected_account_revision.map(i64::from),
            operation.last_error_code,
            operation.created_at.as_str(),
            operation.updated_at.as_str(),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::SchemaInvariant);
    }
    Ok(())
}

fn read_vault_operation(
    connection: &Connection,
    operation_id: &VaultOperationId,
) -> Result<Option<VaultOperation>, StoreError> {
    let sql = format!("{VAULT_OPERATION_SELECT} WHERE operation_id = ?1");
    let row = connection
        .query_row(&sql, [operation_id.as_str()], vault_operation_row)
        .optional()?;
    row.map(vault_operation_from_row).transpose()
}

fn read_active_vault_operations_matching(
    connection: &Connection,
    operation: &VaultOperation,
) -> Result<Vec<VaultOperation>, StoreError> {
    let sql = format!(
        "{VAULT_OPERATION_SELECT}
         WHERE status IN ('in_progress', 'needs_review')
           AND (
               account_fingerprint = ?1
               OR protected_record_ref = ?2
               OR (?3 IS NOT NULL AND account_id = ?3)
           )
         ORDER BY operation_id
         LIMIT 2"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        params![
            operation.account_fingerprint.as_str(),
            operation.protected_record_ref.as_str(),
            operation.account_id.as_ref().map(VaultAccountId::as_str),
        ],
        vault_operation_row,
    )?;
    rows.map(|row| vault_operation_from_row(row?)).collect()
}

fn read_active_vault_operations_by_record_ref(
    connection: &Connection,
    protected_record_ref: &VaultRecordRef,
) -> Result<Vec<VaultOperation>, StoreError> {
    let sql = format!(
        "{VAULT_OPERATION_SELECT}
         WHERE status IN ('in_progress', 'needs_review')
           AND protected_record_ref = ?1
         ORDER BY operation_id
         LIMIT 2"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([protected_record_ref.as_str()], vault_operation_row)?;
    rows.map(|row| vault_operation_from_row(row?)).collect()
}

fn read_recoverable_vault_operations(
    connection: &Connection,
    limit: u32,
) -> Result<Vec<VaultOperation>, StoreError> {
    let sql = format!(
        "{VAULT_OPERATION_SELECT}
         WHERE status IN ('in_progress', 'needs_review')
         ORDER BY created_at, operation_id
         LIMIT ?1"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([i64::from(limit)], vault_operation_row)?;
    rows.map(|row| vault_operation_from_row(row?)).collect()
}

fn vault_operation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VaultOperationRow> {
    Ok(VaultOperationRow {
        operation_id: row.get(0)?,
        kind: row.get(1)?,
        status: row.get(2)?,
        checkpoint: row.get(3)?,
        revision: row.get(4)?,
        account_id: row.get(5)?,
        account_fingerprint: row.get(6)?,
        protected_record_ref: row.get(7)?,
        display_name: row.get(8)?,
        auth_mode: row.get(9)?,
        source: row.get(10)?,
        lifecycle: row.get(11)?,
        provider_id: row.get(12)?,
        model: row.get(13)?,
        expected_account_revision: row.get(14)?,
        last_error_code: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

fn vault_operation_from_row(row: VaultOperationRow) -> Result<VaultOperation, StoreError> {
    let operation = VaultOperation {
        operation_id: VaultOperationId::parse(row.operation_id)?,
        kind: vault_operation_kind_from_str(&row.kind)?,
        status: vault_operation_status_from_str(&row.status)?,
        checkpoint: vault_operation_checkpoint_from_str(&row.checkpoint)?,
        revision: u32::try_from(row.revision).map_err(|_| StoreError::NumericOverflow)?,
        account_id: row.account_id.map(VaultAccountId::parse).transpose()?,
        account_fingerprint: AccountFingerprint::parse(row.account_fingerprint)?,
        protected_record_ref: VaultRecordRef::parse(row.protected_record_ref)?,
        display_name: row.display_name,
        auth_mode: vault_auth_mode_from_str(&row.auth_mode)?,
        source: vault_source_from_str(&row.source)?,
        lifecycle: vault_lifecycle_from_str(&row.lifecycle)?,
        provider_id: row.provider_id,
        model: row.model,
        expected_account_revision: row
            .expected_account_revision
            .map(u32::try_from)
            .transpose()
            .map_err(|_| StoreError::NumericOverflow)?,
        last_error_code: row.last_error_code,
        created_at: UtcTimestamp::parse(row.created_at)?,
        updated_at: UtcTimestamp::parse(row.updated_at)?,
    };
    operation.validate()?;
    Ok(operation)
}

fn same_vault_operation_request(left: &VaultOperation, right: &VaultOperation) -> bool {
    left.kind == right.kind
        && (left.kind == VaultOperationKind::RegisterAccount || left.account_id == right.account_id)
        && left.account_fingerprint == right.account_fingerprint
        && left.protected_record_ref == right.protected_record_ref
        && left.display_name == right.display_name
        && left.auth_mode == right.auth_mode
        && left.source == right.source
        && left.lifecycle == right.lifecycle
        && left.provider_id == right.provider_id
        && left.model == right.model
        && left.expected_account_revision == right.expected_account_revision
}

const VAULT_KEY_ROTATION_SELECT: &str = "SELECT
         rotation_id,
         status,
         checkpoint,
         revision,
         source_key_id,
         target_key_id,
         expected_key_ring_revision,
         last_error_code,
         created_at,
         updated_at
     FROM vault_key_rotations";

fn new_vault_key_rotation_id() -> Result<VaultKeyRotationId, StoreError> {
    VaultKeyRotationId::parse(format!("vault-key-rotation:v1:{}", Uuid::new_v4()))
        .map_err(Into::into)
}

fn insert_vault_key_rotation(
    transaction: &Transaction<'_>,
    rotation: &VaultKeyRotation,
    expected_key_ring_revision: i64,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "INSERT INTO vault_key_rotations(
             rotation_id,
             status,
             checkpoint,
             revision,
             source_key_id,
             target_key_id,
             expected_key_ring_revision,
             last_error_code,
             created_at,
             updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            rotation.rotation_id.as_str(),
            vault_key_rotation_status_as_str(rotation.status),
            vault_key_rotation_checkpoint_as_str(rotation.checkpoint),
            i64::from(rotation.revision),
            rotation.source_key_id.as_str(),
            rotation.target_key_id.as_str(),
            expected_key_ring_revision,
            rotation.last_error_code,
            rotation.created_at.as_str(),
            rotation.updated_at.as_str(),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::SchemaInvariant);
    }
    Ok(())
}

fn read_vault_key_rotation(
    connection: &Connection,
    rotation_id: &VaultKeyRotationId,
) -> Result<Option<VaultKeyRotation>, StoreError> {
    let sql = format!("{VAULT_KEY_ROTATION_SELECT} WHERE rotation_id = ?1");
    let row = connection
        .query_row(&sql, [rotation_id.as_str()], vault_key_rotation_row)
        .optional()?;
    row.map(vault_key_rotation_from_row).transpose()
}

fn read_active_vault_key_rotations(
    connection: &Connection,
) -> Result<Vec<VaultKeyRotation>, StoreError> {
    let sql = format!(
        "{VAULT_KEY_ROTATION_SELECT}
         WHERE status IN ('in_progress', 'rolling_back', 'needs_review')
         ORDER BY created_at, rotation_id
         LIMIT 2"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], vault_key_rotation_row)?;
    rows.map(|row| vault_key_rotation_from_row(row?)).collect()
}

fn read_recoverable_vault_key_rotations(
    connection: &Connection,
    limit: u32,
) -> Result<Vec<VaultKeyRotation>, StoreError> {
    let sql = format!(
        "{VAULT_KEY_ROTATION_SELECT}
         WHERE status IN ('in_progress', 'rolling_back', 'needs_review')
         ORDER BY created_at, rotation_id
         LIMIT ?1"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([i64::from(limit)], vault_key_rotation_row)?;
    rows.map(|row| vault_key_rotation_from_row(row?)).collect()
}

fn vault_key_rotation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VaultKeyRotationRow> {
    Ok(VaultKeyRotationRow {
        rotation_id: row.get(0)?,
        status: row.get(1)?,
        checkpoint: row.get(2)?,
        revision: row.get(3)?,
        source_key_id: row.get(4)?,
        target_key_id: row.get(5)?,
        expected_key_ring_revision: row.get(6)?,
        last_error_code: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn vault_key_rotation_from_row(row: VaultKeyRotationRow) -> Result<VaultKeyRotation, StoreError> {
    let rotation = VaultKeyRotation {
        rotation_id: VaultKeyRotationId::parse(row.rotation_id)?,
        status: vault_key_rotation_status_from_str(&row.status)?,
        checkpoint: vault_key_rotation_checkpoint_from_str(&row.checkpoint)?,
        revision: u32::try_from(row.revision).map_err(|_| StoreError::NumericOverflow)?,
        source_key_id: InstallationKeyId::parse(row.source_key_id)?,
        target_key_id: InstallationKeyId::parse(row.target_key_id)?,
        expected_key_ring_revision: u64::try_from(row.expected_key_ring_revision)
            .map_err(|_| StoreError::NumericOverflow)?,
        last_error_code: row.last_error_code,
        created_at: UtcTimestamp::parse(row.created_at)?,
        updated_at: UtcTimestamp::parse(row.updated_at)?,
    };
    rotation.validate()?;
    Ok(rotation)
}

fn same_vault_key_rotation_request(left: &VaultKeyRotation, right: &VaultKeyRotation) -> bool {
    left.source_key_id == right.source_key_id
        && left.target_key_id == right.target_key_id
        && left.expected_key_ring_revision == right.expected_key_ring_revision
}

fn read_vault_accounts(connection: &Connection) -> Result<Vec<VaultAccount>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT
             account_id,
             account_fingerprint,
             protected_record_ref,
             display_name,
             auth_mode,
             source,
             lifecycle,
             provider_id,
             model,
             revision,
             display_order,
             created_at,
             updated_at,
             last_used_at
         FROM vault_accounts
         ORDER BY display_order, account_id",
    )?;
    let rows = statement.query_map([], vault_account_row)?;
    rows.map(|row| vault_account_from_row(row?)).collect()
}

fn read_vault_account(
    connection: &Connection,
    account_id: &VaultAccountId,
) -> Result<Option<VaultAccount>, StoreError> {
    read_vault_account_where(connection, "account_id", account_id.as_str())
}

fn read_vault_account_by_fingerprint(
    connection: &Connection,
    account_fingerprint: &AccountFingerprint,
) -> Result<Option<VaultAccount>, StoreError> {
    read_vault_account_where(
        connection,
        "account_fingerprint",
        account_fingerprint.as_str(),
    )
}

fn read_vault_account_where(
    connection: &Connection,
    column: &'static str,
    value: &str,
) -> Result<Option<VaultAccount>, StoreError> {
    let sql = format!(
        "SELECT
             account_id,
             account_fingerprint,
             protected_record_ref,
             display_name,
             auth_mode,
             source,
             lifecycle,
             provider_id,
             model,
             revision,
             display_order,
             created_at,
             updated_at,
             last_used_at
         FROM vault_accounts
         WHERE {column} = ?1"
    );
    let row = connection
        .query_row(&sql, [value], vault_account_row)
        .optional()?;
    row.map(vault_account_from_row).transpose()
}

fn vault_account_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VaultAccountRow> {
    Ok(VaultAccountRow {
        account_id: row.get(0)?,
        account_fingerprint: row.get(1)?,
        protected_record_ref: row.get(2)?,
        display_name: row.get(3)?,
        auth_mode: row.get(4)?,
        source: row.get(5)?,
        lifecycle: row.get(6)?,
        provider_id: row.get(7)?,
        model: row.get(8)?,
        revision: row.get(9)?,
        display_order: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        last_used_at: row.get(13)?,
    })
}

fn vault_account_from_row(row: VaultAccountRow) -> Result<VaultAccount, StoreError> {
    let account_fingerprint = AccountFingerprint::parse(row.account_fingerprint)?;
    let protected_record_ref = VaultRecordRef::parse(row.protected_record_ref)?;
    let auth_mode = vault_auth_mode_from_str(&row.auth_mode)?;
    let source = vault_source_from_str(&row.source)?;
    let lifecycle = vault_lifecycle_from_str(&row.lifecycle)?;
    let account = VaultAccount {
        account_id: VaultAccountId::parse(row.account_id)?,
        account_fingerprint: account_fingerprint.clone(),
        protected_record_ref: protected_record_ref.clone(),
        display_name: row.display_name,
        auth_mode,
        source,
        lifecycle,
        provider_id: row.provider_id,
        model: row.model,
        revision: u32::try_from(row.revision).map_err(|_| StoreError::NumericOverflow)?,
        display_order: u32::try_from(row.display_order).map_err(|_| StoreError::NumericOverflow)?,
        created_at: UtcTimestamp::parse(row.created_at)?,
        updated_at: UtcTimestamp::parse(row.updated_at)?,
        last_used_at: row.last_used_at.map(UtcTimestamp::parse).transpose()?,
    };
    VaultAccountRegistration {
        account_fingerprint,
        protected_record_ref,
        display_name: account.display_name.clone(),
        auth_mode,
        source,
        lifecycle,
        provider_id: account.provider_id.clone(),
        model: account.model.clone(),
    }
    .validate()?;
    if account.revision == 0 {
        return Err(StoreError::SchemaInvariant);
    }
    Ok(account)
}

fn read_vault_environment_state(
    connection: &Connection,
    environment_id: &str,
) -> Result<Option<VaultEnvironmentState>, StoreError> {
    let row = connection
        .query_row(
            "SELECT
                 environment_id,
                 revision,
                 selected_account_id,
                 observed_account_id,
                 updated_at
             FROM vault_environment_states
             WHERE environment_id = ?1",
            [environment_id],
            |row| {
                Ok(VaultEnvironmentStateRow {
                    environment_id: row.get(0)?,
                    revision: row.get(1)?,
                    selected_account_id: row.get(2)?,
                    observed_account_id: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()?;
    row.map(vault_environment_state_from_row).transpose()
}

fn vault_environment_state_from_row(
    row: VaultEnvironmentStateRow,
) -> Result<VaultEnvironmentState, StoreError> {
    if !bounded_metadata(&row.environment_id, 256) {
        return Err(StoreError::SchemaInvariant);
    }
    Ok(VaultEnvironmentState {
        environment_id: row.environment_id,
        revision: u32::try_from(row.revision).map_err(|_| StoreError::NumericOverflow)?,
        selected_account_id: row
            .selected_account_id
            .map(VaultAccountId::parse)
            .transpose()?,
        observed_account_id: row
            .observed_account_id
            .map(VaultAccountId::parse)
            .transpose()?,
        updated_at: UtcTimestamp::parse(row.updated_at)?,
    })
}

fn vault_account_mutation_outcome(
    transaction: Transaction<'_>,
    account_id: &VaultAccountId,
    changed: usize,
) -> Result<VaultAccountMutationOutcome, StoreError> {
    if changed == 0 {
        return Ok(match read_vault_account(&transaction, account_id)? {
            Some(current) => VaultAccountMutationOutcome::RevisionConflict(current),
            None => VaultAccountMutationOutcome::Missing,
        });
    }
    if changed != 1 {
        return Err(StoreError::SchemaInvariant);
    }
    let account =
        read_vault_account(&transaction, account_id)?.ok_or(StoreError::SchemaInvariant)?;
    transaction.commit()?;
    Ok(VaultAccountMutationOutcome::Updated(account))
}

fn validate_expected_vault_revision(expected_revision: u32) -> Result<(), StoreError> {
    if expected_revision == 0 {
        return Err(StoreError::InvalidVaultOperation("expected_revision"));
    }
    Ok(())
}

fn validate_vault_mutation(
    expected_revision: u32,
    value: &str,
    maximum_length: usize,
    field: &'static str,
) -> Result<(), StoreError> {
    validate_expected_vault_revision(expected_revision)?;
    if !bounded_metadata(value, maximum_length) || value != value.trim() {
        return Err(StoreError::InvalidVaultOperation(field));
    }
    Ok(())
}

fn validate_environment_metadata(environment: &EnvironmentSnapshot) -> Result<(), StoreError> {
    if !bounded_metadata(&environment.environment_id, 256) {
        return Err(StoreError::InvalidMetadata("environment_id"));
    }
    if !bounded_metadata(&environment.platform, 64) {
        return Err(StoreError::InvalidMetadata("platform"));
    }
    if !bounded_metadata(&environment.architecture, 64) {
        return Err(StoreError::InvalidMetadata("architecture"));
    }
    if !bounded_metadata(&environment.boundary, 256) {
        return Err(StoreError::InvalidMetadata("boundary"));
    }
    Ok(())
}

fn persist_environment_metadata(
    transaction: &Transaction<'_>,
    environment: &EnvironmentSnapshot,
    observed_at: &UtcTimestamp,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "INSERT INTO environments(
             environment_id, platform, architecture, boundary, created_at, last_seen_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(environment_id) DO UPDATE SET
             last_seen_at = CASE
                 WHEN julianday(excluded.last_seen_at) > julianday(environments.last_seen_at)
                 THEN excluded.last_seen_at
                 ELSE environments.last_seen_at
             END
         WHERE environments.platform = excluded.platform
           AND environments.architecture = excluded.architecture
           AND environments.boundary = excluded.boundary",
        params![
            environment.environment_id,
            environment.platform,
            environment.architecture,
            environment.boundary,
            observed_at.as_str(),
        ],
    )?;
    if changed == 0 {
        return Err(StoreError::EnvironmentConflict);
    }
    Ok(())
}

fn vault_auth_mode_as_str(value: VaultAccountAuthMode) -> &'static str {
    match value {
        VaultAccountAuthMode::ChatGpt => "chatgpt",
        VaultAccountAuthMode::ApiKey => "api_key",
        VaultAccountAuthMode::Unknown => "unknown",
    }
}

fn vault_auth_mode_from_str(value: &str) -> Result<VaultAccountAuthMode, StoreError> {
    match value {
        "chatgpt" => Ok(VaultAccountAuthMode::ChatGpt),
        "api_key" => Ok(VaultAccountAuthMode::ApiKey),
        "unknown" => Ok(VaultAccountAuthMode::Unknown),
        _ => Err(StoreError::CorruptEnum("vault_auth_mode")),
    }
}

fn vault_source_as_str(value: VaultAccountSource) -> &'static str {
    match value {
        VaultAccountSource::CurrentRuntime => "current_runtime",
        VaultAccountSource::ManualChatGpt => "manual_chatgpt",
        VaultAccountSource::ManualApi => "manual_api",
    }
}

fn vault_source_from_str(value: &str) -> Result<VaultAccountSource, StoreError> {
    match value {
        "current_runtime" => Ok(VaultAccountSource::CurrentRuntime),
        "manual_chatgpt" => Ok(VaultAccountSource::ManualChatGpt),
        "manual_api" => Ok(VaultAccountSource::ManualApi),
        _ => Err(StoreError::CorruptEnum("vault_source")),
    }
}

fn vault_lifecycle_as_str(value: VaultAccountLifecycle) -> &'static str {
    match value {
        VaultAccountLifecycle::Ready => "ready",
        VaultAccountLifecycle::RecordUnavailable => "record_unavailable",
        VaultAccountLifecycle::HistoricalOnly => "historical_only",
        VaultAccountLifecycle::NeedsReview => "needs_review",
    }
}

fn vault_lifecycle_from_str(value: &str) -> Result<VaultAccountLifecycle, StoreError> {
    match value {
        "ready" => Ok(VaultAccountLifecycle::Ready),
        "record_unavailable" => Ok(VaultAccountLifecycle::RecordUnavailable),
        "historical_only" => Ok(VaultAccountLifecycle::HistoricalOnly),
        "needs_review" => Ok(VaultAccountLifecycle::NeedsReview),
        _ => Err(StoreError::CorruptEnum("vault_lifecycle")),
    }
}

fn vault_operation_kind_as_str(value: VaultOperationKind) -> &'static str {
    match value {
        VaultOperationKind::RegisterAccount => "register_account",
        VaultOperationKind::ForgetAccount => "forget_account",
    }
}

fn vault_operation_kind_from_str(value: &str) -> Result<VaultOperationKind, StoreError> {
    match value {
        "register_account" => Ok(VaultOperationKind::RegisterAccount),
        "forget_account" => Ok(VaultOperationKind::ForgetAccount),
        _ => Err(StoreError::CorruptEnum("vault_operation_kind")),
    }
}

fn vault_operation_status_as_str(value: VaultOperationStatus) -> &'static str {
    match value {
        VaultOperationStatus::InProgress => "in_progress",
        VaultOperationStatus::Succeeded => "succeeded",
        VaultOperationStatus::Compensated => "compensated",
        VaultOperationStatus::NeedsReview => "needs_review",
    }
}

fn vault_operation_status_from_str(value: &str) -> Result<VaultOperationStatus, StoreError> {
    match value {
        "in_progress" => Ok(VaultOperationStatus::InProgress),
        "succeeded" => Ok(VaultOperationStatus::Succeeded),
        "compensated" => Ok(VaultOperationStatus::Compensated),
        "needs_review" => Ok(VaultOperationStatus::NeedsReview),
        _ => Err(StoreError::CorruptEnum("vault_operation_status")),
    }
}

fn vault_operation_checkpoint_as_str(value: VaultOperationCheckpoint) -> &'static str {
    match value {
        VaultOperationCheckpoint::Prepared => "prepared",
        VaultOperationCheckpoint::RecordReady => "record_ready",
        VaultOperationCheckpoint::RecordQuarantined => "record_quarantined",
        VaultOperationCheckpoint::MetadataCommitted => "metadata_committed",
        VaultOperationCheckpoint::MetadataRemoved => "metadata_removed",
        VaultOperationCheckpoint::RecordRestored => "record_restored",
    }
}

fn vault_operation_checkpoint_from_str(
    value: &str,
) -> Result<VaultOperationCheckpoint, StoreError> {
    match value {
        "prepared" => Ok(VaultOperationCheckpoint::Prepared),
        "record_ready" => Ok(VaultOperationCheckpoint::RecordReady),
        "record_quarantined" => Ok(VaultOperationCheckpoint::RecordQuarantined),
        "metadata_committed" => Ok(VaultOperationCheckpoint::MetadataCommitted),
        "metadata_removed" => Ok(VaultOperationCheckpoint::MetadataRemoved),
        "record_restored" => Ok(VaultOperationCheckpoint::RecordRestored),
        _ => Err(StoreError::CorruptEnum("vault_operation_checkpoint")),
    }
}

fn vault_key_rotation_status_as_str(value: VaultKeyRotationStatus) -> &'static str {
    match value {
        VaultKeyRotationStatus::InProgress => "in_progress",
        VaultKeyRotationStatus::RollingBack => "rolling_back",
        VaultKeyRotationStatus::Succeeded => "succeeded",
        VaultKeyRotationStatus::Compensated => "compensated",
        VaultKeyRotationStatus::NeedsReview => "needs_review",
    }
}

fn vault_key_rotation_status_from_str(value: &str) -> Result<VaultKeyRotationStatus, StoreError> {
    match value {
        "in_progress" => Ok(VaultKeyRotationStatus::InProgress),
        "rolling_back" => Ok(VaultKeyRotationStatus::RollingBack),
        "succeeded" => Ok(VaultKeyRotationStatus::Succeeded),
        "compensated" => Ok(VaultKeyRotationStatus::Compensated),
        "needs_review" => Ok(VaultKeyRotationStatus::NeedsReview),
        _ => Err(StoreError::CorruptEnum("vault_key_rotation_status")),
    }
}

fn vault_key_rotation_checkpoint_as_str(value: VaultKeyRotationCheckpoint) -> &'static str {
    match value {
        VaultKeyRotationCheckpoint::Prepared => "prepared",
        VaultKeyRotationCheckpoint::KeyRingStarted => "key_ring_started",
        VaultKeyRotationCheckpoint::AccountsMigrated => "accounts_migrated",
        VaultKeyRotationCheckpoint::DependenciesCleared => "dependencies_cleared",
        VaultKeyRotationCheckpoint::PredecessorRetired => "predecessor_retired",
        VaultKeyRotationCheckpoint::RollbackStarted => "rollback_started",
        VaultKeyRotationCheckpoint::AccountsRestored => "accounts_restored",
        VaultKeyRotationCheckpoint::NewKeyRetired => "new_key_retired",
    }
}

fn vault_key_rotation_checkpoint_from_str(
    value: &str,
) -> Result<VaultKeyRotationCheckpoint, StoreError> {
    match value {
        "prepared" => Ok(VaultKeyRotationCheckpoint::Prepared),
        "key_ring_started" => Ok(VaultKeyRotationCheckpoint::KeyRingStarted),
        "accounts_migrated" => Ok(VaultKeyRotationCheckpoint::AccountsMigrated),
        "dependencies_cleared" => Ok(VaultKeyRotationCheckpoint::DependenciesCleared),
        "predecessor_retired" => Ok(VaultKeyRotationCheckpoint::PredecessorRetired),
        "rollback_started" => Ok(VaultKeyRotationCheckpoint::RollbackStarted),
        "accounts_restored" => Ok(VaultKeyRotationCheckpoint::AccountsRestored),
        "new_key_retired" => Ok(VaultKeyRotationCheckpoint::NewKeyRetired),
        _ => Err(StoreError::CorruptEnum("vault_key_rotation_checkpoint")),
    }
}

fn validate_snapshot_metadata(
    snapshot: &StatusSnapshot,
    observation: &CompatibilityObservationInput,
) -> Result<(), StoreError> {
    let environment = &snapshot.environment;
    validate_environment_metadata(environment)?;
    if observation.environment_id != environment.environment_id
        || observation.compatibility != snapshot.data_status.compatibility
    {
        return Err(StoreError::CompatibilityObservationMismatch);
    }
    if !bounded_metadata(&observation.executable_id, 256)
        || observation
            .codex_version
            .as_deref()
            .is_some_and(|value| !bounded_metadata(value, 128))
        || observation
            .protocol_schema_fingerprint
            .as_deref()
            .is_some_and(|value| !bounded_metadata(value, 128))
    {
        return Err(StoreError::InvalidMetadata("compatibility_observation"));
    }

    let executable = snapshot
        .codex_executable
        .as_ref()
        .ok_or(StoreError::MissingExecutableIdentity)?;
    if executable.executable_id != observation.executable_id
        || executable.version != observation.codex_version
    {
        return Err(StoreError::CompatibilityObservationMismatch);
    }

    for window in &snapshot.quota.windows {
        if !bounded_metadata(&window.limit_id, 128)
            || window
                .label
                .as_deref()
                .is_some_and(|value| !bounded_metadata(value, 256))
        {
            return Err(StoreError::InvalidMetadata("quota_window"));
        }
    }
    Ok(())
}

fn persist_environment(
    transaction: &Transaction<'_>,
    snapshot: &StatusSnapshot,
) -> Result<(), StoreError> {
    persist_environment_metadata(transaction, &snapshot.environment, &snapshot.captured_at)
}

fn persist_account_binding(
    transaction: &Transaction<'_>,
    snapshot: &StatusSnapshot,
    fingerprint: &AccountFingerprint,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO account_bindings(
             environment_id, account_fingerprint, created_at, last_seen_at
         ) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(environment_id, account_fingerprint) DO UPDATE SET
             last_seen_at = CASE
                 WHEN julianday(excluded.last_seen_at) > julianday(account_bindings.last_seen_at)
                 THEN excluded.last_seen_at
                 ELSE account_bindings.last_seen_at
             END",
        params![
            snapshot.environment.environment_id,
            fingerprint.as_str(),
            snapshot.captured_at.as_str()
        ],
    )?;
    Ok(())
}

fn persist_compatibility_observation(
    transaction: &Transaction<'_>,
    observation: &CompatibilityObservationInput,
    observation_id: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO compatibility_observations(
             observation_id,
             environment_id,
             executable_id,
             codex_version,
             protocol_schema_fingerprint,
             compatibility,
             observed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            observation_id,
            observation.environment_id,
            observation.executable_id,
            observation.codex_version,
            observation.protocol_schema_fingerprint,
            compatibility_as_str(observation.compatibility),
            observation.observed_at.as_str()
        ],
    )?;
    Ok(())
}

fn persist_quota_snapshot(
    transaction: &Transaction<'_>,
    snapshot: &StatusSnapshot,
    fingerprint: &AccountFingerprint,
    observation_id: &str,
    snapshot_id: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO quota_snapshots(
             snapshot_id,
             environment_id,
             account_fingerprint,
             compatibility_observation_id,
             status_schema_version,
             captured_at,
             availability,
             freshness
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'live')",
        params![
            snapshot_id,
            snapshot.environment.environment_id,
            fingerprint.as_str(),
            observation_id,
            snapshot.schema_version,
            snapshot.captured_at.as_str(),
            availability_as_str(snapshot.data_status.availability)
        ],
    )?;
    Ok(())
}

fn persist_quota_context(
    transaction: &Transaction<'_>,
    snapshot: &StatusSnapshot,
    snapshot_id: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO quota_snapshot_contexts(snapshot_id,account_plan_type) VALUES (?1,?2)",
        params![
            snapshot_id,
            capacity_domain::pace::normalize_plan_type(
                snapshot
                    .account
                    .as_ref()
                    .and_then(|account| account.plan_type.as_deref())
            ),
        ],
    )?;
    Ok(())
}

fn persist_quota_windows(
    transaction: &Transaction<'_>,
    snapshot: &StatusSnapshot,
    snapshot_id: &str,
) -> Result<(), StoreError> {
    let mut statement = transaction.prepare(
        "INSERT INTO quota_windows(
             snapshot_id,
             limit_id,
             label,
             window_minutes,
             used_basis_points,
             remaining_basis_points,
             resets_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for window in &snapshot.quota.windows {
        let remaining_basis_points = percent_to_basis_points(window.remaining_percent);
        let used_basis_points = 10_000 - remaining_basis_points;
        statement.execute(params![
            snapshot_id,
            window.limit_id,
            window.label,
            optional_u64_to_i64(window.window_minutes)?,
            used_basis_points,
            remaining_basis_points,
            window.resets_at.as_ref().map(UtcTimestamp::as_str)
        ])?;
    }
    Ok(())
}

fn persist_reset_credits(
    transaction: &Transaction<'_>,
    snapshot: &StatusSnapshot,
    snapshot_id: &str,
) -> Result<(), StoreError> {
    let summary = &snapshot.quota.reset_credit_summary;
    transaction.execute(
        "INSERT INTO reset_credit_summaries(
             snapshot_id, summary_status, available_count, details_status
         ) VALUES (?1, ?2, ?3, ?4)",
        params![
            snapshot_id,
            summary_status_as_str(summary.summary_status),
            optional_u64_to_i64(summary.available_count)?,
            reset_credit_details_status_as_str(summary.details_status)
        ],
    )?;

    let Some(credits) = &summary.credits else {
        return Ok(());
    };
    let mut statement = transaction.prepare(
        "INSERT INTO reset_credits(
             snapshot_id,
             ordinal,
             opaque_id,
             reset_type,
             status,
             granted_at,
             expires_at,
             title,
             description
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    for (ordinal, credit) in credits.iter().enumerate() {
        statement.execute(params![
            snapshot_id,
            i64::try_from(ordinal).map_err(|_| StoreError::NumericOverflow)?,
            credit.opaque_id,
            credit.reset_type,
            credit.status,
            credit.granted_at.as_ref().map(UtcTimestamp::as_str),
            credit.expires_at.as_ref().map(UtcTimestamp::as_str),
            credit.title,
            credit.description
        ])?;
    }
    Ok(())
}

fn history_point_from_row(row: HistoryRow) -> Result<HistoryPoint, StoreError> {
    Ok(HistoryPoint {
        snapshot_id: row.snapshot_id,
        captured_at: UtcTimestamp::parse(row.captured_at)?,
        limit_id: row.limit_id,
        label: row.label,
        window_minutes: row
            .window_minutes
            .map(u64::try_from)
            .transpose()
            .map_err(|_| StoreError::NumericOverflow)?,
        used_percent: row.used_basis_points as f64 / 100.0,
        remaining_percent: row.remaining_basis_points as f64 / 100.0,
        resets_at: row.resets_at.map(UtcTimestamp::parse).transpose()?,
        availability: availability_from_str(&row.availability)?,
        compatibility: compatibility_from_str(&row.compatibility)?,
    })
}

fn read_work_schedule(connection: &Connection) -> Result<WorkScheduleSettings, StoreError> {
    let (revision, enabled, updated_at) = connection.query_row(
        "SELECT revision, enabled, updated_at
         FROM work_schedule_settings
         WHERE singleton_id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut statement = connection.prepare(
        "SELECT start_minute_of_day, end_minute_of_day
         FROM work_schedule_periods
         WHERE singleton_id = 1
         ORDER BY ordinal",
    )?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    let mut periods = Vec::new();
    for row in rows {
        let (start, end) = row?;
        periods.push(WorkSchedulePeriod::new(
            u16::try_from(start).map_err(|_| StoreError::NumericOverflow)?,
            u16::try_from(end).map_err(|_| StoreError::NumericOverflow)?,
        )?);
    }
    Ok(WorkScheduleSettings {
        revision: u32::try_from(revision).map_err(|_| StoreError::NumericOverflow)?,
        schedule: WorkSchedule::new(integer_to_bool(enabled)?, periods)?,
        updated_at: UtcTimestamp::parse(updated_at)?,
    })
}

fn read_monitor_settings(connection: &Connection) -> Result<MonitorSettings, StoreError> {
    let row = connection.query_row(
        "SELECT
             revision,
             auto_refresh_enabled,
             refresh_interval_seconds,
             notification_threshold_basis_points,
             reset_credit_notice_hours,
             quiet_hours_enabled,
             quiet_hours_start_minute,
             quiet_hours_end_minute,
             language,
             lock_screen_privacy,
             launch_at_login,
             history_retention_days,
             updated_at
         FROM settings
         WHERE singleton_id = 1",
        [],
        |row| {
            Ok(SettingsRow {
                revision: row.get(0)?,
                auto_refresh_enabled: row.get(1)?,
                refresh_interval_seconds: row.get(2)?,
                notification_threshold_basis_points: row.get(3)?,
                reset_credit_notice_hours: row.get(4)?,
                quiet_hours_enabled: row.get(5)?,
                quiet_hours_start_minute: row.get(6)?,
                quiet_hours_end_minute: row.get(7)?,
                language: row.get(8)?,
                lock_screen_privacy: row.get(9)?,
                launch_at_login: row.get(10)?,
                history_retention_days: row.get(11)?,
                updated_at: row.get(12)?,
            })
        },
    )?;
    let settings = MonitorSettings {
        revision: u32::try_from(row.revision).map_err(|_| StoreError::NumericOverflow)?,
        auto_refresh_enabled: integer_to_bool(row.auto_refresh_enabled)?,
        refresh_interval_seconds: u32::try_from(row.refresh_interval_seconds)
            .map_err(|_| StoreError::NumericOverflow)?,
        notification_threshold_basis_points: row
            .notification_threshold_basis_points
            .map(u16::try_from)
            .transpose()
            .map_err(|_| StoreError::NumericOverflow)?,
        reset_credit_notice_hours: u16::try_from(row.reset_credit_notice_hours)
            .map_err(|_| StoreError::NumericOverflow)?,
        quiet_hours_enabled: integer_to_bool(row.quiet_hours_enabled)?,
        quiet_hours_start_minute: row
            .quiet_hours_start_minute
            .map(u16::try_from)
            .transpose()
            .map_err(|_| StoreError::NumericOverflow)?,
        quiet_hours_end_minute: row
            .quiet_hours_end_minute
            .map(u16::try_from)
            .transpose()
            .map_err(|_| StoreError::NumericOverflow)?,
        language: settings_language_from_str(&row.language)?,
        lock_screen_privacy: integer_to_bool(row.lock_screen_privacy)?,
        launch_at_login: integer_to_bool(row.launch_at_login)?,
        history_retention_days: u16::try_from(row.history_retention_days)
            .map_err(|_| StoreError::NumericOverflow)?,
        updated_at: UtcTimestamp::parse(row.updated_at)?,
    };
    validate_settings_update(&MonitorSettingsUpdate::from(&settings))?;
    Ok(settings)
}

impl From<&MonitorSettings> for MonitorSettingsUpdate {
    fn from(settings: &MonitorSettings) -> Self {
        Self {
            expected_revision: settings.revision,
            auto_refresh_enabled: settings.auto_refresh_enabled,
            refresh_interval_seconds: settings.refresh_interval_seconds,
            notification_threshold_basis_points: settings.notification_threshold_basis_points,
            reset_credit_notice_hours: settings.reset_credit_notice_hours,
            quiet_hours_enabled: settings.quiet_hours_enabled,
            quiet_hours_start_minute: settings.quiet_hours_start_minute,
            quiet_hours_end_minute: settings.quiet_hours_end_minute,
            language: settings.language,
            lock_screen_privacy: settings.lock_screen_privacy,
            launch_at_login: settings.launch_at_login,
            history_retention_days: settings.history_retention_days,
        }
    }
}

fn validate_settings_update(update: &MonitorSettingsUpdate) -> Result<(), StoreError> {
    if update.expected_revision == 0 {
        return Err(StoreError::InvalidSettings("expected_revision"));
    }
    if !SUPPORTED_REFRESH_INTERVAL_SECONDS.contains(&update.refresh_interval_seconds) {
        return Err(StoreError::InvalidSettings("refresh_interval_seconds"));
    }
    if update
        .notification_threshold_basis_points
        .is_some_and(|value| value > 10_000)
    {
        return Err(StoreError::InvalidSettings(
            "notification_threshold_basis_points",
        ));
    }
    if !(1..=720).contains(&update.reset_credit_notice_hours) {
        return Err(StoreError::InvalidSettings("reset_credit_notice_hours"));
    }
    let valid_quiet_hours = if update.quiet_hours_enabled {
        matches!(
            (
                update.quiet_hours_start_minute,
                update.quiet_hours_end_minute
            ),
            (Some(start), Some(end)) if start < 1_440 && end < 1_440 && start != end
        )
    } else {
        update.quiet_hours_start_minute.is_none() && update.quiet_hours_end_minute.is_none()
    };
    if !valid_quiet_hours {
        return Err(StoreError::InvalidSettings("quiet_hours"));
    }
    if !(7..=730).contains(&update.history_retention_days) {
        return Err(StoreError::InvalidSettings("history_retention_days"));
    }
    Ok(())
}

fn settings_language_as_str(language: SettingsLanguage) -> &'static str {
    match language {
        SettingsLanguage::System => "system",
        SettingsLanguage::English => "en",
        SettingsLanguage::SimplifiedChinese => "zh-CN",
    }
}

fn settings_language_from_str(value: &str) -> Result<SettingsLanguage, StoreError> {
    match value {
        "system" => Ok(SettingsLanguage::System),
        "en" => Ok(SettingsLanguage::English),
        "zh-CN" => Ok(SettingsLanguage::SimplifiedChinese),
        _ => Err(StoreError::CorruptEnum("settings_language")),
    }
}

fn bool_to_integer(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn integer_to_bool(value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(StoreError::CorruptEnum("settings_boolean")),
    }
}

fn insert_default_settings(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO settings(
             singleton_id,
             revision,
             auto_refresh_enabled,
             refresh_interval_seconds,
             notification_threshold_basis_points,
             reset_credit_notice_hours,
             quiet_hours_enabled,
             quiet_hours_start_minute,
             quiet_hours_end_minute,
             language,
             lock_screen_privacy,
             launch_at_login,
             history_retention_days,
             updated_at
         ) VALUES (
             1, 1, 1, 300, 2000, 24, 0, NULL, NULL,
             'system', 1, 0, 180,
             strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         )",
        [],
    )?;
    Ok(())
}

fn insert_default_work_schedule(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO work_schedule_settings(singleton_id, revision, enabled, updated_at)
         VALUES (1, 1, 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        [],
    )?;
    Ok(())
}

fn table_count(connection: &Connection, table: &'static str) -> Result<u64, StoreError> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count = connection.query_row(&sql, [], |row| row.get::<_, i64>(0))?;
    u64::try_from(count).map_err(|_| StoreError::NumericOverflow)
}

fn count_fingerprint_key_dependencies(
    connection: &Connection,
    sql: &str,
    value: &str,
) -> Result<u64, StoreError> {
    let count = connection.query_row(sql, [value], |row| row.get::<_, i64>(0))?;
    u64::try_from(count).map_err(|_| StoreError::NumericOverflow)
}

fn count_exact_fingerprint(
    connection: &Connection,
    table: &'static str,
    fingerprint: &AccountFingerprint,
) -> Result<u64, StoreError> {
    let sql = match table {
        "account_bindings" => {
            "SELECT COUNT(*) FROM account_bindings WHERE account_fingerprint = ?1"
        }
        "quota_snapshots" => "SELECT COUNT(*) FROM quota_snapshots WHERE account_fingerprint = ?1",
        _ => return Err(StoreError::SchemaInvariant),
    };
    count_fingerprint_key_dependencies(connection, sql, fingerprint.as_str())
}

fn percent_to_basis_points(value: f64) -> i64 {
    (value * 100.0).round().clamp(0.0, 10_000.0) as i64
}

fn optional_u64_to_i64(value: Option<u64>) -> Result<Option<i64>, StoreError> {
    value
        .map(i64::try_from)
        .transpose()
        .map_err(|_| StoreError::NumericOverflow)
}

fn bounded_metadata(value: &str, maximum_length: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum_length
        && !value.chars().any(char::is_control)
}

fn availability_as_str(value: Availability) -> &'static str {
    match value {
        Availability::Complete => "complete",
        Availability::Partial => "partial",
        Availability::Unsupported => "unsupported",
        Availability::Failed => "failed",
    }
}

fn availability_from_str(value: &str) -> Result<Availability, StoreError> {
    match value {
        "complete" => Ok(Availability::Complete),
        "partial" => Ok(Availability::Partial),
        _ => Err(StoreError::CorruptEnum("availability")),
    }
}

fn compatibility_as_str(value: Compatibility) -> &'static str {
    match value {
        Compatibility::Tested => "tested",
        Compatibility::ExpectedCompatible => "expected_compatible",
        Compatibility::NotTested => "not_tested",
        Compatibility::Unsupported => "unsupported",
        Compatibility::KnownBroken => "known_broken",
        Compatibility::NotApplicable => "not_applicable",
    }
}

fn compatibility_from_str(value: &str) -> Result<Compatibility, StoreError> {
    match value {
        "tested" => Ok(Compatibility::Tested),
        "expected_compatible" => Ok(Compatibility::ExpectedCompatible),
        "not_tested" => Ok(Compatibility::NotTested),
        "unsupported" => Ok(Compatibility::Unsupported),
        "known_broken" => Ok(Compatibility::KnownBroken),
        "not_applicable" => Ok(Compatibility::NotApplicable),
        _ => Err(StoreError::CorruptEnum("compatibility")),
    }
}

fn summary_status_as_str(value: SummaryStatus) -> &'static str {
    match value {
        SummaryStatus::Available => "available",
        SummaryStatus::Partial => "partial",
        SummaryStatus::Unavailable => "unavailable",
    }
}

fn reset_credit_details_status_as_str(value: ResetCreditDetailsStatus) -> &'static str {
    match value {
        ResetCreditDetailsStatus::Complete => "complete",
        ResetCreditDetailsStatus::Partial => "partial",
        ResetCreditDetailsStatus::Unavailable => "unavailable",
    }
}

#[cfg(unix)]
fn prepare_database_file(path: &Path) -> Result<(), StoreError> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(StoreError::UnsafeDatabaseFileType);
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(StoreError::InsecureDatabasePermissions);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_database_file(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(unix)]
fn verify_database_file(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(StoreError::UnsafeDatabaseFileType);
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(StoreError::InsecureDatabasePermissions);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_database_file(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

fn native_codex_environment_id(environment_id: &str, platform: &str) -> bool {
    matches!(platform, "macos" | "windows" | "linux")
        && environment_id
            .strip_prefix(&format!("{platform}:local:codex-executable-"))
            .is_some_and(|identity| {
                identity.len() == 16
                    && identity
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use capacity_domain::{
        AccountSnapshot, CodexExecutableSnapshot, DataStatus, EnvironmentSnapshot, QuotaSnapshot,
        QuotaWindow, ResetCredit, ResetCreditSummary, UsageCreditSummary, UsageSnapshot,
    };

    use super::*;

    pub(super) const FINGERPRINT: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    pub(super) const FINGERPRINT_2: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:1123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const ROTATED_FINGERPRINT: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73411:2123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const SOURCE_KEY_ID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73311";
    const TARGET_KEY_ID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73411";

    pub(super) fn stable_binding() -> AccountBinding {
        AccountBinding::Stable(
            AccountFingerprint::parse(FINGERPRINT).expect("valid test fingerprint"),
        )
    }

    // Write a genuine pre-v8 fixture without using the current-schema API.
    pub(super) fn persist_legacy_snapshot(
        store: &mut CapacityStore,
        snapshot: &StatusSnapshot,
        observation: &CompatibilityObservationInput,
    ) -> Result<(), StoreError> {
        let fingerprint = AccountFingerprint::parse(FINGERPRINT)?;
        let snapshot_id = Uuid::new_v4().to_string();
        let observation_id = Uuid::new_v4().to_string();
        let transaction = store.connection.transaction()?;
        persist_environment(&transaction, snapshot)?;
        persist_account_binding(&transaction, snapshot, &fingerprint)?;
        persist_compatibility_observation(&transaction, observation, &observation_id)?;
        persist_quota_snapshot(
            &transaction,
            snapshot,
            &fingerprint,
            &observation_id,
            &snapshot_id,
        )?;
        persist_quota_windows(&transaction, snapshot, &snapshot_id)?;
        persist_reset_credits(&transaction, snapshot, &snapshot_id)?;
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn snapshot(captured_at: &str) -> StatusSnapshot {
        StatusSnapshot {
            schema_version: "1.0".into(),
            captured_at: UtcTimestamp::parse(captured_at).expect("valid capture time"),
            environment: EnvironmentSnapshot {
                environment_id: "macos-arm64-native".into(),
                platform: "macos".into(),
                architecture: "arm64".into(),
                boundary: "native".into(),
            },
            codex_executable: Some(CodexExecutableSnapshot {
                executable_id: "codex-install-1".into(),
                source: "chatgpt_bundle".into(),
                canonical_path: None,
                file_identity: None,
                version: Some("0.150.0-alpha.12.2".into()),
            }),
            account: Some(AccountSnapshot {
                auth_mode: Some("chatgpt".into()),
                plan_type: Some("prolite".into()),
                binding_status: AccountBindingStatus::Stable,
            }),
            quota: QuotaSnapshot {
                windows: vec![QuotaWindow {
                    limit_id: "codex:primary".into(),
                    label: Some("Weekly".into()),
                    window_minutes: Some(10_080),
                    used_percent: 37.125,
                    remaining_percent: 62.875,
                    resets_at: Some(
                        UtcTimestamp::parse("2026-09-05T21:22:43Z").expect("valid reset time"),
                    ),
                }],
                reset_credit_summary: ResetCreditSummary {
                    summary_status: SummaryStatus::Available,
                    available_count: Some(1),
                    details_status: ResetCreditDetailsStatus::Complete,
                    credits: Some(vec![ResetCredit {
                        opaque_id: "credit-1".into(),
                        reset_type: "weekly".into(),
                        status: "available".into(),
                        granted_at: None,
                        expires_at: Some(
                            UtcTimestamp::parse("2026-09-20T00:00:00Z").expect("valid expiry"),
                        ),
                        title: None,
                        description: None,
                    }]),
                },
                usage_credit_summary: UsageCreditSummary {
                    summary_status: SummaryStatus::Unavailable,
                    entries: Vec::new(),
                    captured_at: UtcTimestamp::parse(captured_at)
                        .expect("valid usage capture time"),
                    upstream_schema_fingerprint: None,
                },
            },
            usage: UsageSnapshot {
                availability: Availability::Unsupported,
                summary: None,
                reason_codes: Vec::new(),
            },
            data_status: DataStatus {
                availability: Availability::Complete,
                freshness: Freshness::Live,
                compatibility: Compatibility::Tested,
                reason_codes: Vec::new(),
            },
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn observation(captured_at: &str) -> CompatibilityObservationInput {
        CompatibilityObservationInput {
            environment_id: "macos-arm64-native".into(),
            executable_id: "codex-install-1".into(),
            codex_version: Some("0.150.0-alpha.12.2".into()),
            protocol_schema_fingerprint: Some("schema-test-v1".into()),
            compatibility: Compatibility::Tested,
            observed_at: UtcTimestamp::parse(captured_at).expect("valid observation time"),
        }
    }

    fn settings_update(expected_revision: u32) -> MonitorSettingsUpdate {
        MonitorSettingsUpdate {
            expected_revision,
            auto_refresh_enabled: true,
            refresh_interval_seconds: 900,
            notification_threshold_basis_points: Some(1_500),
            reset_credit_notice_hours: 48,
            quiet_hours_enabled: true,
            quiet_hours_start_minute: Some(22 * 60),
            quiet_hours_end_minute: Some(7 * 60),
            language: SettingsLanguage::SimplifiedChinese,
            lock_screen_privacy: true,
            launch_at_login: false,
            history_retention_days: 365,
        }
    }

    fn vault_registration(
        fingerprint: &str,
        record_uuid: &str,
        display_name: &str,
    ) -> VaultAccountRegistration {
        VaultAccountRegistration {
            account_fingerprint: AccountFingerprint::parse(fingerprint).expect("fingerprint"),
            protected_record_ref: VaultRecordRef::parse(format!("vault-record:v1:{record_uuid}"))
                .expect("protected record reference"),
            display_name: display_name.into(),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::ManualChatGpt,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: None,
            model: None,
        }
    }

    fn vault_environment() -> EnvironmentSnapshot {
        EnvironmentSnapshot {
            environment_id: "macos-arm64-native".into(),
            platform: "macos".into(),
            architecture: "arm64".into(),
            boundary: "native".into(),
        }
    }

    fn v1_connection() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open SQLite");
        configure_connection(&connection, false).expect("configure SQLite");
        let transaction = connection.transaction().expect("start v1 migration");
        transaction
            .execute_batch(MIGRATION_V1)
            .expect("apply v1 schema");
        record_schema_migration(&transaction, 1).expect("record v1 migration");
        transaction
            .pragma_update(None, "user_version", 1)
            .expect("set v1 user version");
        transaction.commit().expect("commit v1 migration");
        connection
    }

    fn v2_connection() -> Connection {
        let mut connection = v1_connection();
        let transaction = connection.transaction().expect("start v2 migration");
        transaction
            .execute_batch(MIGRATION_V2)
            .expect("apply v2 schema");
        record_schema_migration(&transaction, 2).expect("record v2 migration");
        transaction
            .pragma_update(None, "user_version", 2)
            .expect("set v2 user version");
        transaction.commit().expect("commit v2 migration");
        connection
    }

    fn v3_connection() -> Connection {
        let mut connection = v2_connection();
        let transaction = connection.transaction().expect("start v3 migration");
        transaction
            .execute_batch(MIGRATION_V3)
            .expect("apply v3 schema");
        record_schema_migration(&transaction, 3).expect("record v3 migration");
        transaction
            .pragma_update(None, "user_version", 3)
            .expect("set v3 user version");
        transaction.commit().expect("commit v3 migration");
        connection
    }

    pub(super) fn v4_connection() -> Connection {
        let mut connection = v3_connection();
        let transaction = connection.transaction().expect("start v4 migration");
        transaction
            .execute_batch(MIGRATION_V4)
            .expect("apply v4 schema");
        record_schema_migration(&transaction, 4).expect("record v4 migration");
        transaction
            .pragma_update(None, "user_version", 4)
            .expect("set v4 user version");
        transaction.commit().expect("commit v4 migration");
        connection
    }

    fn register_account(
        store: &mut CapacityStore,
        registration: &VaultAccountRegistration,
        created_at: &str,
    ) -> VaultAccount {
        let outcome = store
            .register_vault_account(
                registration,
                &UtcTimestamp::parse(created_at).expect("created at"),
            )
            .expect("register vault account");
        let VaultAccountRegistrationOutcome::Created(account) = outcome else {
            panic!("new test account must be created");
        };
        account
    }

    fn begin_registration_operation(
        store: &mut CapacityStore,
        registration: &VaultAccountRegistration,
        created_at: &str,
    ) -> VaultOperation {
        let outcome = store
            .begin_vault_registration_operation(
                registration,
                &UtcTimestamp::parse(created_at).expect("created at"),
            )
            .expect("begin registration operation");
        let VaultOperationBeginOutcome::Created(operation) = outcome else {
            panic!("new test operation must be created");
        };
        operation
    }

    fn insert_v3_registration_operation(
        store: &mut CapacityStore,
        registration: &VaultAccountRegistration,
        created_at: &str,
    ) -> VaultOperation {
        let created_at = UtcTimestamp::parse(created_at).expect("created at");
        let operation = VaultOperation {
            operation_id: new_vault_operation_id().expect("operation ID"),
            kind: VaultOperationKind::RegisterAccount,
            status: VaultOperationStatus::InProgress,
            checkpoint: VaultOperationCheckpoint::Prepared,
            revision: 1,
            account_id: None,
            account_fingerprint: registration.account_fingerprint.clone(),
            protected_record_ref: registration.protected_record_ref.clone(),
            display_name: registration.display_name.clone(),
            auth_mode: registration.auth_mode,
            source: registration.source,
            lifecycle: registration.lifecycle,
            provider_id: registration.provider_id.clone(),
            model: registration.model.clone(),
            expected_account_revision: None,
            last_error_code: None,
            created_at: created_at.clone(),
            updated_at: created_at,
        };
        let transaction = store.connection.transaction().expect("start v3 insert");
        insert_vault_operation(&transaction, &operation).expect("insert v3 operation");
        transaction.commit().expect("commit v3 operation");
        operation
    }

    fn advance_operation(
        store: &mut CapacityStore,
        operation: &VaultOperation,
        transition: VaultOperationTransition,
        updated_at: &str,
    ) -> VaultOperation {
        let outcome = store
            .advance_vault_operation(
                &operation.operation_id,
                operation.revision,
                transition,
                &UtcTimestamp::parse(updated_at).expect("updated at"),
            )
            .expect("advance operation");
        let VaultOperationAdvanceOutcome::Updated(operation) = outcome else {
            panic!("current operation revision must advance");
        };
        operation
    }

    fn begin_key_rotation(store: &mut CapacityStore, created_at: &str) -> VaultKeyRotation {
        let outcome = store
            .begin_vault_key_rotation(
                &InstallationKeyId::parse(SOURCE_KEY_ID).expect("source key ID"),
                &InstallationKeyId::parse(TARGET_KEY_ID).expect("target key ID"),
                7,
                &UtcTimestamp::parse(created_at).expect("created at"),
            )
            .expect("begin key rotation");
        let VaultKeyRotationBeginOutcome::Created(rotation) = outcome else {
            panic!("new test key rotation must be created");
        };
        rotation
    }

    fn advance_key_rotation(
        store: &mut CapacityStore,
        rotation: &VaultKeyRotation,
        transition: VaultKeyRotationTransition,
        updated_at: &str,
    ) -> VaultKeyRotation {
        let outcome = store
            .advance_vault_key_rotation(
                &rotation.rotation_id,
                rotation.revision,
                transition,
                &UtcTimestamp::parse(updated_at).expect("updated at"),
            )
            .expect("advance key rotation");
        let VaultKeyRotationAdvanceOutcome::Updated(rotation) = outcome else {
            panic!("current key rotation revision must advance");
        };
        rotation
    }

    #[test]
    fn vault_mutation_recovery_summary_is_identifier_free_and_status_aware() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        assert_eq!(
            store
                .vault_mutation_recovery_summary()
                .expect("empty summary"),
            VaultMutationRecoverySummary {
                managed_accounts: 0,
                pending_account_operations: 0,
                account_operations_needing_review: 0,
                pending_key_rotations: 0,
                key_rotations_needing_review: 0,
            }
        );

        register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T14:25:00Z",
        );
        let pending = begin_registration_operation(
            &mut store,
            &vault_registration(
                FINGERPRINT_2,
                "018f47a2-8a71-7f4a-9c35-1f4234a73313",
                "Backup ChatGPT",
            ),
            "2026-08-30T14:26:00Z",
        );
        let review = advance_operation(
            &mut store,
            &pending,
            VaultOperationTransition::NeedsReview {
                reason_code: "recovery_fixture".into(),
            },
            "2026-08-30T14:27:00Z",
        );

        let summary = store
            .vault_mutation_recovery_summary()
            .expect("account-operation summary");
        assert_eq!(summary.managed_accounts, 1);
        assert_eq!(summary.pending_account_operations, 1);
        assert_eq!(summary.account_operations_needing_review, 1);
        assert_eq!(summary.pending_key_rotations, 0);
        assert_eq!(summary.key_rotations_needing_review, 0);
        assert_eq!(summary.needs_review_count(), 1);
        assert!(summary.has_pending_work());

        let resumed = advance_operation(
            &mut store,
            &review,
            VaultOperationTransition::Resume,
            "2026-08-30T14:28:00Z",
        );
        assert_eq!(resumed.status, VaultOperationStatus::InProgress);
        assert_eq!(
            store
                .vault_mutation_recovery_summary()
                .expect("resumed summary")
                .account_operations_needing_review,
            0
        );
    }

    #[test]
    fn vault_mutation_recovery_summary_includes_rolling_or_review_key_rotation() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let rotation = begin_key_rotation(&mut store, "2026-08-30T14:25:00Z");
        let started = advance_key_rotation(
            &mut store,
            &rotation,
            VaultKeyRotationTransition::KeyRingStarted,
            "2026-08-30T14:26:00Z",
        );
        let rolling_back = advance_key_rotation(
            &mut store,
            &started,
            VaultKeyRotationTransition::BeginRollback {
                reason_code: "explicit_rollback".into(),
            },
            "2026-08-30T14:27:00Z",
        );
        assert_eq!(rolling_back.status, VaultKeyRotationStatus::RollingBack);
        let summary = store
            .vault_mutation_recovery_summary()
            .expect("rollback summary");
        assert_eq!(summary.pending_key_rotations, 1);
        assert_eq!(summary.key_rotations_needing_review, 0);

        let review = advance_key_rotation(
            &mut store,
            &rolling_back,
            VaultKeyRotationTransition::NeedsReview {
                reason_code: "rotation_fixture".into(),
            },
            "2026-08-30T14:28:00Z",
        );
        assert_eq!(review.status, VaultKeyRotationStatus::NeedsReview);
        let summary = store
            .vault_mutation_recovery_summary()
            .expect("review summary");
        assert_eq!(summary.pending_key_rotations, 1);
        assert_eq!(summary.key_rotations_needing_review, 1);
        assert_eq!(summary.needs_review_count(), 1);
    }

    #[test]
    fn settings_defaults_and_revision_guard_round_trip() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let defaults = store.settings().expect("default settings");
        assert_eq!(defaults.revision, 1);
        assert!(defaults.auto_refresh_enabled);
        assert_eq!(defaults.refresh_interval_seconds, 300);
        assert_eq!(defaults.notification_threshold_basis_points, Some(2_000));
        assert_eq!(defaults.language, SettingsLanguage::System);

        let updated_at = UtcTimestamp::parse("2026-08-30T04:00:00Z").expect("update time");
        let updated = store
            .update_settings(&settings_update(1), &updated_at)
            .expect("update settings");
        let SettingsUpdateOutcome::Updated(updated) = updated else {
            panic!("current revision must update");
        };
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.refresh_interval_seconds, 900);
        assert_eq!(updated.quiet_hours_start_minute, Some(1_320));
        assert_eq!(updated.language, SettingsLanguage::SimplifiedChinese);
        assert_eq!(updated.updated_at, updated_at);

        let conflict = store
            .update_settings(
                &settings_update(1),
                &UtcTimestamp::parse("2026-08-30T04:01:00Z").expect("conflict time"),
            )
            .expect("revision conflict response");
        let SettingsUpdateOutcome::RevisionConflict(current) = conflict else {
            panic!("stale revision must not overwrite settings");
        };
        assert_eq!(current, updated);
        assert_eq!(store.settings().expect("stored settings"), updated);
    }

    #[test]
    fn work_schedule_is_normalized_persisted_and_revision_guarded() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let defaults = store.work_schedule().expect("default work schedule");
        assert_eq!(defaults.revision, 1);
        assert!(!defaults.schedule.enabled);
        assert!(defaults.schedule.off_periods.is_empty());

        let updated_at = UtcTimestamp::parse("2026-09-04T05:00:00Z").expect("update time");
        let request = WorkScheduleSettingsUpdate {
            expected_revision: 1,
            enabled: true,
            off_periods: vec![
                WorkSchedulePeriod::new(120, 600).unwrap(),
                WorkSchedulePeriod::new(1_380, 60).unwrap(),
                WorkSchedulePeriod::new(120, 600).unwrap(),
                WorkSchedulePeriod::new(300, 300).unwrap(),
            ],
        };
        let WorkScheduleSettingsUpdateOutcome::Updated(updated) = store
            .update_work_schedule(&request, &updated_at)
            .expect("update work schedule")
        else {
            panic!("current work schedule revision must update");
        };
        assert_eq!(updated.revision, 2);
        assert!(updated.schedule.enabled);
        assert_eq!(updated.schedule.off_periods.len(), 2);
        assert_eq!(updated.updated_at, updated_at);

        let WorkScheduleSettingsUpdateOutcome::RevisionConflict(current) = store
            .update_work_schedule(&request, &updated_at)
            .expect("stale work schedule response")
        else {
            panic!("stale work schedule revision must conflict");
        };
        assert_eq!(current, updated);
        assert_eq!(store.work_schedule().unwrap(), updated);
    }

    #[test]
    fn invalid_settings_are_rejected_before_sql() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let mut invalid = settings_update(1);
        invalid.refresh_interval_seconds = 61;
        assert!(matches!(
            store.update_settings(
                &invalid,
                &UtcTimestamp::parse("2026-08-30T04:00:00Z").expect("update time")
            ),
            Err(StoreError::InvalidSettings("refresh_interval_seconds"))
        ));
        assert_eq!(store.settings().expect("unchanged settings").revision, 1);

        invalid = settings_update(1);
        invalid.quiet_hours_end_minute = invalid.quiet_hours_start_minute;
        assert!(matches!(
            store.update_settings(
                &invalid,
                &UtcTimestamp::parse("2026-08-30T04:00:00Z").expect("update time")
            ),
            Err(StoreError::InvalidSettings("quiet_hours"))
        ));
        assert_eq!(store.settings().expect("unchanged settings").revision, 1);
    }

    #[test]
    fn guarded_delete_all_rejects_stale_confirmation_revision() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot("2026-08-30T02:55:53Z"),
                &observation("2026-08-30T02:55:53Z"),
            )
            .expect("persist snapshot");

        let conflict = store
            .delete_all_database_data_if_revision(2)
            .expect("guarded delete conflict");
        assert!(matches!(
            conflict,
            DeleteDatabaseOutcome::RevisionConflict(MonitorSettings { revision: 1, .. })
        ));
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            1
        );

        let deleted = store
            .delete_all_database_data_if_revision(1)
            .expect("guarded delete");
        let DeleteDatabaseOutcome::Deleted(report) = deleted else {
            panic!("current revision must delete data");
        };
        assert_eq!(report.total_rows(), 10);
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            0
        );
        assert_eq!(store.settings().expect("reset settings").revision, 1);
        let schedule = store.work_schedule().expect("reset work schedule");
        assert_eq!(schedule.revision, 1);
        assert!(!schedule.schedule.enabled);
    }

    #[test]
    fn migration_creates_exact_current_tables_and_enables_guards() {
        let store = CapacityStore::open_in_memory().expect("open store");
        assert_eq!(
            store.schema_version().expect("schema version"),
            STORE_SCHEMA_VERSION
        );

        let tables: BTreeSet<String> = store
            .connection
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .expect("prepare table query")
            .query_map([], |row| row.get(0))
            .expect("query tables")
            .collect::<Result<_, _>>()
            .expect("read tables");
        assert_eq!(
            tables,
            BTreeSet::from([
                "account_bindings".into(),
                "active_time_observations".into(),
                "active_time_timers".into(),
                "capacity_demands".into(),
                "capacity_work_plans".into(),
                "compatibility_observations".into(),
                "environments".into(),
                "quota_snapshots".into(),
                "quota_snapshot_contexts".into(),
                "pace_trials".into(),
                "pace_trial_outcomes".into(),
                "quota_windows".into(),
                "reset_credit_summaries".into(),
                "reset_credits".into(),
                "schema_migrations".into(),
                "settings".into(),
                "vault_accounts".into(),
                "vault_environment_states".into(),
                "vault_key_rotations".into(),
                "vault_operations".into(),
                "work_schedule_periods".into(),
                "work_schedule_settings".into(),
            ])
        );
        let foreign_keys: i64 = store
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("foreign key pragma");
        let secure_delete: i64 = store
            .connection
            .pragma_query_value(None, "secure_delete", |row| row.get(0))
            .expect("secure delete pragma");
        assert_eq!(foreign_keys, 1);
        assert_eq!(secure_delete, 1);
    }

    #[test]
    fn v1_history_schema_upgrades_without_losing_existing_rows() {
        let connection = v1_connection();
        connection
            .execute(
                "INSERT INTO environments(
                     environment_id, platform, architecture, boundary, created_at, last_seen_at
                 ) VALUES ('macos-arm64-native', 'macos', 'arm64', 'native', ?1, ?1)",
                ["2026-08-30T02:55:53Z"],
            )
            .expect("insert v1 environment");
        connection
            .execute(
                "INSERT INTO account_bindings(
                     environment_id, account_fingerprint, created_at, last_seen_at
                 ) VALUES ('macos-arm64-native', ?1, ?2, ?2)",
                params![FINGERPRINT, "2026-08-30T02:55:53Z"],
            )
            .expect("insert v1 binding");
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };

        store.migrate().expect("upgrade v1");
        store.verify_schema().expect("verify current schema");

        assert_eq!(
            store.schema_version().expect("schema version"),
            STORE_SCHEMA_VERSION
        );
        assert_eq!(
            table_count(&store.connection, "account_bindings").unwrap(),
            1
        );
        assert_eq!(table_count(&store.connection, "vault_accounts").unwrap(), 0);
        assert_eq!(
            table_count(&store.connection, "vault_operations").unwrap(),
            0
        );
        assert_eq!(
            table_count(&store.connection, "vault_key_rotations").unwrap(),
            0
        );
        assert_eq!(
            table_count(&store.connection, "schema_migrations").unwrap(),
            u64::from(STORE_SCHEMA_VERSION)
        );
    }

    #[test]
    fn v2_account_schema_upgrades_without_losing_accounts_or_history() {
        let connection = v2_connection();
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };
        let account = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T06:10:00Z",
        );
        persist_legacy_snapshot(
            &mut store,
            &snapshot("2026-08-30T06:11:00Z"),
            &observation("2026-08-30T06:11:00Z"),
        )
        .expect("persist v2 history");

        store.migrate().expect("upgrade v2");
        store.verify_schema().expect("verify current schema");

        assert_eq!(
            store.schema_version().expect("schema version"),
            STORE_SCHEMA_VERSION
        );
        assert_eq!(
            store.vault_account(&account.account_id).unwrap(),
            Some(account)
        );
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            1
        );
        assert_eq!(
            table_count(&store.connection, "vault_operations").unwrap(),
            0
        );
        assert_eq!(
            table_count(&store.connection, "vault_key_rotations").unwrap(),
            0
        );
        assert_eq!(
            table_count(&store.connection, "schema_migrations").unwrap(),
            u64::from(STORE_SCHEMA_VERSION)
        );
    }

    #[test]
    fn v3_operation_schema_upgrades_without_losing_recovery_work() {
        let connection = v3_connection();
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };
        let operation = insert_v3_registration_operation(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T07:20:00Z",
        );

        store.migrate().expect("upgrade v3");
        store.verify_schema().expect("verify current schema");

        assert_eq!(
            store.schema_version().expect("schema version"),
            STORE_SCHEMA_VERSION
        );
        assert_eq!(
            store
                .recoverable_vault_operations(MAX_VAULT_RECOVERY_QUERY)
                .expect("preserved operation"),
            vec![operation]
        );
        assert_eq!(
            table_count(&store.connection, "vault_key_rotations").unwrap(),
            0
        );
        assert_eq!(
            table_count(&store.connection, "schema_migrations").unwrap(),
            u64::from(STORE_SCHEMA_VERSION)
        );
    }

    #[test]
    fn key_rotation_begin_is_exact_idempotent_and_cross_journal_exclusive() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let created = begin_key_rotation(&mut store, "2026-08-30T08:00:00Z");
        let source = InstallationKeyId::parse(SOURCE_KEY_ID).expect("source key ID");
        let target = InstallationKeyId::parse(TARGET_KEY_ID).expect("target key ID");

        assert_eq!(
            store
                .begin_vault_key_rotation(
                    &source,
                    &target,
                    7,
                    &UtcTimestamp::parse("2026-08-30T08:01:00Z").expect("timestamp"),
                )
                .expect("repeat exact intent"),
            VaultKeyRotationBeginOutcome::Existing(created.clone())
        );
        let other_target = InstallationKeyId::parse("018f47a2-8a71-7f4a-9c35-1f4234a73511")
            .expect("other target key ID");
        assert!(matches!(
            store.begin_vault_key_rotation(
                &source,
                &other_target,
                7,
                &UtcTimestamp::parse("2026-08-30T08:02:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultKeyRotationConflict)
        ));
        assert!(matches!(
            store.begin_vault_key_rotation(
                &source,
                &target,
                8,
                &UtcTimestamp::parse("2026-08-30T08:02:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultKeyRotationConflict)
        ));
        assert!(matches!(
            store.begin_vault_key_rotation(
                &source,
                &target,
                0,
                &UtcTimestamp::parse("2026-08-30T08:02:00Z").expect("timestamp")
            ),
            Err(StoreError::InvalidVaultOperation(
                "expected_key_ring_revision"
            ))
        ));
        assert!(matches!(
            store.begin_vault_registration_operation(
                &vault_registration(
                    FINGERPRINT,
                    "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                    "Blocked registration",
                ),
                &UtcTimestamp::parse("2026-08-30T08:03:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultOperationConflict)
        ));

        assert_eq!(
            store
                .vault_key_dependency_audit(SOURCE_KEY_ID)
                .expect("general dependency audit"),
            VaultKeyDependencyAudit {
                vault_accounts: 0,
                account_bindings: 0,
                unmanaged_account_bindings: 0,
                quota_snapshots: 0,
                active_vault_operations: 0,
                active_key_rotations: 1,
            }
        );
        assert_eq!(
            store
                .vault_key_dependency_audit_for_rotation(&source, &created.rotation_id)
                .expect("owner-scoped retirement audit")
                .active_key_rotations,
            0
        );
        assert!(matches!(
            store.vault_key_dependency_audit_for_rotation(&other_target, &created.rotation_id),
            Err(StoreError::InvalidVaultOperation(
                "key_rotation_dependency_audit"
            ))
        ));
        assert!(matches!(
            store.recoverable_vault_key_rotations(0),
            Err(StoreError::InvalidVaultOperation("recovery_limit"))
        ));
        assert!(matches!(
            store.recoverable_vault_key_rotations(MAX_VAULT_RECOVERY_QUERY + 1),
            Err(StoreError::InvalidVaultOperation("recovery_limit"))
        ));

        let mut operation_store = CapacityStore::open_in_memory().expect("open second store");
        begin_registration_operation(
            &mut operation_store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Active registration",
            ),
            "2026-08-30T08:00:00Z",
        );
        assert!(matches!(
            operation_store.begin_vault_key_rotation(
                &source,
                &target,
                7,
                &UtcTimestamp::parse("2026-08-30T08:01:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultKeyRotationConflict)
        ));
    }

    #[test]
    fn key_rotation_forward_recovery_is_durable_and_revision_guarded() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("capacity.sqlite3");
        let started = {
            let mut store = CapacityStore::open(&path).expect("open file store");
            let prepared = begin_key_rotation(&mut store, "2026-08-30T08:00:00Z");
            advance_key_rotation(
                &mut store,
                &prepared,
                VaultKeyRotationTransition::KeyRingStarted,
                "2026-08-30T08:01:00Z",
            )
        };

        let mut store = CapacityStore::open(&path).expect("reopen file store");
        assert_eq!(
            store
                .recoverable_vault_key_rotations(1)
                .expect("recovery queue after reopen"),
            vec![started.clone()]
        );
        assert_eq!(
            store.vault_key_rotation(&started.rotation_id).unwrap(),
            Some(started.clone())
        );
        let stale = store
            .advance_vault_key_rotation(
                &started.rotation_id,
                started.revision - 1,
                VaultKeyRotationTransition::AccountsMigrated,
                &UtcTimestamp::parse("2026-08-30T08:02:00Z").expect("timestamp"),
            )
            .expect("stale revision outcome");
        assert_eq!(
            stale,
            VaultKeyRotationAdvanceOutcome::RevisionConflict(started.clone())
        );
        assert!(matches!(
            store.advance_vault_key_rotation(
                &started.rotation_id,
                started.revision,
                VaultKeyRotationTransition::DependenciesCleared,
                &UtcTimestamp::parse("2026-08-30T08:02:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultValidation(
                VaultValidationError::InvalidKeyRotationTransition
            ))
        ));

        let paused = advance_key_rotation(
            &mut store,
            &started,
            VaultKeyRotationTransition::NeedsReview {
                reason_code: "backend_temporarily_unavailable".into(),
            },
            "2026-08-30T08:02:00Z",
        );
        let resumed = advance_key_rotation(
            &mut store,
            &paused,
            VaultKeyRotationTransition::Resume,
            "2026-08-30T08:03:00Z",
        );
        let migrated = advance_key_rotation(
            &mut store,
            &resumed,
            VaultKeyRotationTransition::AccountsMigrated,
            "2026-08-30T08:04:00Z",
        );
        let cleared = advance_key_rotation(
            &mut store,
            &migrated,
            VaultKeyRotationTransition::DependenciesCleared,
            "2026-08-30T08:05:00Z",
        );
        let retired = advance_key_rotation(
            &mut store,
            &cleared,
            VaultKeyRotationTransition::PredecessorRetired,
            "2026-08-30T08:06:00Z",
        );
        let succeeded = advance_key_rotation(
            &mut store,
            &retired,
            VaultKeyRotationTransition::Succeeded,
            "2026-08-30T08:07:00Z",
        );
        assert_eq!(succeeded.status, VaultKeyRotationStatus::Succeeded);
        assert!(
            store
                .recoverable_vault_key_rotations(MAX_VAULT_RECOVERY_QUERY)
                .expect("terminal row excluded")
                .is_empty()
        );
        assert_eq!(
            store
                .prune_terminal_vault_key_rotations_before(
                    &UtcTimestamp::parse("2026-08-30T08:08:00Z").expect("cutoff")
                )
                .expect("prune terminal rotation"),
            1
        );
        assert!(
            store
                .vault_key_rotation(&succeeded.rotation_id)
                .unwrap()
                .is_none()
        );
        let missing_id =
            VaultKeyRotationId::parse("vault-key-rotation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73999")
                .expect("missing rotation ID");
        assert_eq!(
            store
                .advance_vault_key_rotation(
                    &missing_id,
                    1,
                    VaultKeyRotationTransition::KeyRingStarted,
                    &UtcTimestamp::parse("2026-08-30T08:09:00Z").expect("timestamp")
                )
                .expect("missing outcome"),
            VaultKeyRotationAdvanceOutcome::Missing
        );
    }

    #[test]
    fn key_rotation_rollback_is_resumable_and_schema_rejects_invalid_states() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let prepared = begin_key_rotation(&mut store, "2026-08-30T08:00:00Z");
        let started = advance_key_rotation(
            &mut store,
            &prepared,
            VaultKeyRotationTransition::KeyRingStarted,
            "2026-08-30T08:01:00Z",
        );

        let duplicate_active_sql = format!(
            "INSERT INTO vault_key_rotations
             SELECT 'vault-key-rotation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73998',
                    status, checkpoint, revision, source_key_id, target_key_id,
                    expected_key_ring_revision, last_error_code, created_at, updated_at
             FROM vault_key_rotations WHERE rotation_id = '{}'",
            started.rotation_id.as_str()
        );
        assert!(store.connection.execute(&duplicate_active_sql, []).is_err());

        let rolling_back = advance_key_rotation(
            &mut store,
            &started,
            VaultKeyRotationTransition::BeginRollback {
                reason_code: "account_migration_failed".into(),
            },
            "2026-08-30T08:02:00Z",
        );
        let paused = advance_key_rotation(
            &mut store,
            &rolling_back,
            VaultKeyRotationTransition::NeedsReview {
                reason_code: "record_restore_interrupted".into(),
            },
            "2026-08-30T08:03:00Z",
        );
        let resumed = advance_key_rotation(
            &mut store,
            &paused,
            VaultKeyRotationTransition::Resume,
            "2026-08-30T08:04:00Z",
        );
        assert_eq!(resumed.status, VaultKeyRotationStatus::RollingBack);
        let restored = advance_key_rotation(
            &mut store,
            &resumed,
            VaultKeyRotationTransition::AccountsRestored,
            "2026-08-30T08:05:00Z",
        );
        let retired = advance_key_rotation(
            &mut store,
            &restored,
            VaultKeyRotationTransition::NewKeyRetired,
            "2026-08-30T08:06:00Z",
        );
        let compensated = advance_key_rotation(
            &mut store,
            &retired,
            VaultKeyRotationTransition::Compensated,
            "2026-08-30T08:07:00Z",
        );
        assert_eq!(compensated.status, VaultKeyRotationStatus::Compensated);

        let invalid_state_sql = format!(
            "INSERT INTO vault_key_rotations
             SELECT 'vault-key-rotation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73997',
                    'succeeded', 'prepared', 1, source_key_id, target_key_id,
                    expected_key_ring_revision, NULL, created_at, updated_at
             FROM vault_key_rotations WHERE rotation_id = '{}'",
            compensated.rotation_id.as_str()
        );
        assert!(store.connection.execute(&invalid_state_sql, []).is_err());
        let same_key_sql = format!(
            "INSERT INTO vault_key_rotations
             SELECT 'vault-key-rotation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73996',
                    status, checkpoint, revision, source_key_id, source_key_id,
                    expected_key_ring_revision, last_error_code, created_at, updated_at
             FROM vault_key_rotations WHERE rotation_id = '{}'",
            compensated.rotation_id.as_str()
        );
        assert!(store.connection.execute(&same_key_sql, []).is_err());
    }

    #[test]
    fn key_rotation_terminal_history_has_a_hard_capacity_bound() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let prepared = begin_key_rotation(&mut store, "2026-08-30T08:00:00Z");
        let started = advance_key_rotation(
            &mut store,
            &prepared,
            VaultKeyRotationTransition::KeyRingStarted,
            "2026-08-30T08:01:00Z",
        );
        let migrated = advance_key_rotation(
            &mut store,
            &started,
            VaultKeyRotationTransition::AccountsMigrated,
            "2026-08-30T08:02:00Z",
        );
        let cleared = advance_key_rotation(
            &mut store,
            &migrated,
            VaultKeyRotationTransition::DependenciesCleared,
            "2026-08-30T08:03:00Z",
        );
        let retired = advance_key_rotation(
            &mut store,
            &cleared,
            VaultKeyRotationTransition::PredecessorRetired,
            "2026-08-30T08:04:00Z",
        );
        let terminal = advance_key_rotation(
            &mut store,
            &retired,
            VaultKeyRotationTransition::Succeeded,
            "2026-08-30T08:05:00Z",
        );

        let transaction = store.connection.transaction().expect("bulk transaction");
        for index in 1..MAX_TOTAL_VAULT_KEY_ROTATIONS {
            let mut rotation = terminal.clone();
            rotation.rotation_id = VaultKeyRotationId::parse(format!(
                "vault-key-rotation:v1:{}",
                Uuid::from_u128(u128::from(index))
            ))
            .expect("rotation ID");
            insert_vault_key_rotation(&transaction, &rotation, 7)
                .expect("insert terminal rotation");
        }
        transaction.commit().expect("commit terminal rotations");
        assert_eq!(
            table_count(&store.connection, "vault_key_rotations").unwrap(),
            u64::from(MAX_TOTAL_VAULT_KEY_ROTATIONS)
        );

        assert!(matches!(
            store.begin_vault_key_rotation(
                &InstallationKeyId::parse(SOURCE_KEY_ID).expect("source key ID"),
                &InstallationKeyId::parse(TARGET_KEY_ID).expect("target key ID"),
                7,
                &UtcTimestamp::parse("2026-08-30T08:06:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultKeyRotationJournalFull)
        ));
    }

    #[test]
    fn vault_operation_begin_is_exactly_idempotent_and_rejects_aliases() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let registration = vault_registration(
            FINGERPRINT,
            "018f47a2-8a71-7f4a-9c35-1f4234a73312",
            "Primary ChatGPT",
        );
        let created =
            begin_registration_operation(&mut store, &registration, "2026-08-30T07:20:00Z");

        let repeated = store
            .begin_vault_registration_operation(
                &registration,
                &UtcTimestamp::parse("2026-08-30T07:21:00Z").expect("timestamp"),
            )
            .expect("repeat exact operation");
        assert_eq!(
            repeated,
            VaultOperationBeginOutcome::Existing(created.clone())
        );

        let mut changed_metadata = registration.clone();
        changed_metadata.display_name = "Different intent".into();
        assert!(matches!(
            store.begin_vault_registration_operation(
                &changed_metadata,
                &UtcTimestamp::parse("2026-08-30T07:22:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultOperationConflict)
        ));

        let aliased_record = vault_registration(
            FINGERPRINT_2,
            "018f47a2-8a71-7f4a-9c35-1f4234a73312",
            "Alias attempt",
        );
        assert!(matches!(
            store.begin_vault_registration_operation(
                &aliased_record,
                &UtcTimestamp::parse("2026-08-30T07:23:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultOperationConflict)
        ));
        assert_eq!(
            table_count(&store.connection, "vault_operations").unwrap(),
            1
        );
    }

    #[test]
    fn vault_operation_recovery_queue_and_revision_transitions_are_durable() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let first = begin_registration_operation(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T07:20:00Z",
        );
        let second = begin_registration_operation(
            &mut store,
            &vault_registration(
                FINGERPRINT_2,
                "018f47a2-8a71-7f4a-9c35-1f4234a73313",
                "Backup ChatGPT",
            ),
            "2026-08-30T07:21:00Z",
        );
        assert_eq!(
            store
                .active_vault_operation_by_record_ref(&first.protected_record_ref)
                .expect("active operation lookup"),
            Some(first.clone())
        );
        assert_eq!(
            store
                .recoverable_vault_operations(1)
                .expect("bounded queue"),
            vec![first.clone()]
        );
        assert!(matches!(
            store.recoverable_vault_operations(0),
            Err(StoreError::InvalidVaultOperation("recovery_limit"))
        ));
        assert!(matches!(
            store.recoverable_vault_operations(MAX_VAULT_RECOVERY_QUERY + 1),
            Err(StoreError::InvalidVaultOperation("recovery_limit"))
        ));

        let ready = advance_operation(
            &mut store,
            &first,
            VaultOperationTransition::RecordReady,
            "2026-08-30T07:22:00Z",
        );
        let stale = store
            .advance_vault_operation(
                &ready.operation_id,
                first.revision,
                VaultOperationTransition::NeedsReview {
                    reason_code: "stale_worker".into(),
                },
                &UtcTimestamp::parse("2026-08-30T07:23:00Z").expect("timestamp"),
            )
            .expect("stale outcome");
        assert_eq!(
            stale,
            VaultOperationAdvanceOutcome::RevisionConflict(ready.clone())
        );
        assert!(matches!(
            store.advance_vault_operation(
                &ready.operation_id,
                ready.revision,
                VaultOperationTransition::MetadataRemoved,
                &UtcTimestamp::parse("2026-08-30T07:23:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultValidation(
                VaultValidationError::InvalidOperationTransition
            ))
        ));

        let paused = advance_operation(
            &mut store,
            &ready,
            VaultOperationTransition::NeedsReview {
                reason_code: "record_backend_unavailable".into(),
            },
            "2026-08-30T07:23:00Z",
        );
        assert_eq!(paused.status, VaultOperationStatus::NeedsReview);
        let resumed = advance_operation(
            &mut store,
            &paused,
            VaultOperationTransition::Resume,
            "2026-08-30T07:24:00Z",
        );
        let committed = advance_operation(
            &mut store,
            &resumed,
            VaultOperationTransition::MetadataCommitted {
                account_id: VaultAccountId::parse(
                    "account:v1:018f47a2-8a71-7f4a-9c35-1f4234a73314",
                )
                .expect("account ID"),
            },
            "2026-08-30T07:25:00Z",
        );
        let succeeded = advance_operation(
            &mut store,
            &committed,
            VaultOperationTransition::Succeeded,
            "2026-08-30T07:26:00Z",
        );
        assert_eq!(
            store.vault_operation(&succeeded.operation_id).unwrap(),
            Some(succeeded.clone())
        );
        assert!(
            store
                .active_vault_operation_by_record_ref(&succeeded.protected_record_ref)
                .expect("terminal operation excluded")
                .is_none()
        );
        assert_eq!(
            store
                .active_vault_operation_by_record_ref(&second.protected_record_ref)
                .expect("second active operation"),
            Some(second.clone())
        );
        assert_eq!(
            store
                .recoverable_vault_operations(MAX_VAULT_RECOVERY_QUERY)
                .expect("recovery queue"),
            vec![second]
        );

        let invalid_sql = format!(
            "INSERT INTO vault_operations
             SELECT 'vault-operation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73399',
                    kind, 'succeeded', 'prepared', 1, NULL,
                    account_fingerprint, protected_record_ref, display_name,
                    auth_mode, source, lifecycle, provider_id, model, NULL,
                    NULL, created_at, updated_at
             FROM vault_operations WHERE operation_id = '{}'",
            succeeded.operation_id.as_str()
        );
        assert!(store.connection.execute(&invalid_sql, []).is_err());

        let malformed_id_sql = format!(
            "INSERT INTO vault_operations
             SELECT 'vault-operation:v1:-18f47a2-8a71-7f4a-9c35-1f4234a73399',
                    kind, status, checkpoint, revision, account_id,
                    account_fingerprint, protected_record_ref, display_name,
                    auth_mode, source, lifecycle, provider_id, model,
                    expected_account_revision, last_error_code, created_at, updated_at
             FROM vault_operations WHERE operation_id = '{}'",
            succeeded.operation_id.as_str()
        );
        assert!(store.connection.execute(&malformed_id_sql, []).is_err());
    }

    #[test]
    fn forget_operation_compensates_and_retention_never_prunes_active_work() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let account = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T07:20:00Z",
        );
        let outcome = store
            .begin_vault_forget_operation(
                &account,
                &UtcTimestamp::parse("2026-08-30T07:21:00Z").expect("timestamp"),
            )
            .expect("begin forget");
        let VaultOperationBeginOutcome::Created(forget) = outcome else {
            panic!("new forget operation must be created");
        };
        assert_eq!(
            store
                .begin_vault_forget_operation(
                    &account,
                    &UtcTimestamp::parse("2026-08-30T07:22:00Z").expect("timestamp"),
                )
                .expect("idempotent forget"),
            VaultOperationBeginOutcome::Existing(forget.clone())
        );
        let mut stale_account = account.clone();
        stale_account.revision += 1;
        assert!(matches!(
            store.begin_vault_forget_operation(
                &stale_account,
                &UtcTimestamp::parse("2026-08-30T07:22:30Z").expect("timestamp")
            ),
            Err(StoreError::VaultOperationConflict)
        ));

        let quarantined = advance_operation(
            &mut store,
            &forget,
            VaultOperationTransition::RecordQuarantined,
            "2026-08-30T07:23:00Z",
        );
        let restored = advance_operation(
            &mut store,
            &quarantined,
            VaultOperationTransition::RecordRestored,
            "2026-08-30T07:24:00Z",
        );
        let compensated = advance_operation(
            &mut store,
            &restored,
            VaultOperationTransition::Compensated {
                reason_code: "metadata_revision_conflict".into(),
            },
            "2026-08-30T07:25:00Z",
        );
        let active = begin_registration_operation(
            &mut store,
            &vault_registration(
                FINGERPRINT_2,
                "018f47a2-8a71-7f4a-9c35-1f4234a73313",
                "Backup ChatGPT",
            ),
            "2026-08-30T07:26:00Z",
        );
        let pruned = store
            .prune_terminal_vault_operations_before(
                &UtcTimestamp::parse("2026-08-30T07:30:00Z").expect("cutoff"),
            )
            .expect("prune terminal operations");
        assert_eq!(pruned, 1);
        assert!(
            store
                .vault_operation(&compensated.operation_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.vault_operation(&active.operation_id).unwrap(),
            Some(active)
        );
    }

    #[test]
    fn vault_operation_active_set_has_a_hard_capacity_bound() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let base = begin_registration_operation(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T07:20:00Z",
        );
        let transaction = store.connection.transaction().expect("bulk transaction");
        for index in 1..MAX_ACTIVE_VAULT_OPERATIONS {
            let mut operation = base.clone();
            operation.operation_id = VaultOperationId::parse(format!(
                "vault-operation:v1:{}",
                Uuid::from_u128(u128::from(index))
            ))
            .expect("operation ID");
            operation.account_fingerprint = AccountFingerprint::parse(format!(
                "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:{index:064x}"
            ))
            .expect("fingerprint");
            operation.protected_record_ref = VaultRecordRef::parse(format!(
                "vault-record:v1:{}",
                Uuid::from_u128(u128::from(index) + 10_000)
            ))
            .expect("record reference");
            insert_vault_operation(&transaction, &operation).expect("insert bounded operation");
        }
        transaction.commit().expect("commit bounded operations");
        assert_eq!(
            table_count(&store.connection, "vault_operations").unwrap(),
            u64::from(MAX_ACTIVE_VAULT_OPERATIONS)
        );

        let overflow = vault_registration(
            FINGERPRINT_2,
            "018f47a2-8a71-7f4a-9c35-1f4234a73313",
            "Overflow",
        );
        assert!(matches!(
            store.begin_vault_registration_operation(
                &overflow,
                &UtcTimestamp::parse("2026-08-30T07:30:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultOperationJournalFull)
        ));
    }

    #[test]
    fn vault_operation_terminal_history_has_a_hard_capacity_bound() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let prepared = begin_registration_operation(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T07:20:00Z",
        );
        let ready = advance_operation(
            &mut store,
            &prepared,
            VaultOperationTransition::RecordReady,
            "2026-08-30T07:21:00Z",
        );
        let committed = advance_operation(
            &mut store,
            &ready,
            VaultOperationTransition::MetadataCommitted {
                account_id: VaultAccountId::parse(
                    "account:v1:018f47a2-8a71-7f4a-9c35-1f4234a73314",
                )
                .expect("account ID"),
            },
            "2026-08-30T07:22:00Z",
        );
        let terminal = advance_operation(
            &mut store,
            &committed,
            VaultOperationTransition::Succeeded,
            "2026-08-30T07:23:00Z",
        );
        let transaction = store.connection.transaction().expect("bulk transaction");
        for index in 1..MAX_TOTAL_VAULT_OPERATIONS {
            let mut operation = terminal.clone();
            operation.operation_id = VaultOperationId::parse(format!(
                "vault-operation:v1:{}",
                Uuid::from_u128(u128::from(index))
            ))
            .expect("operation ID");
            insert_vault_operation(&transaction, &operation).expect("insert terminal operation");
        }
        transaction.commit().expect("commit terminal operations");
        assert_eq!(
            table_count(&store.connection, "vault_operations").unwrap(),
            u64::from(MAX_TOTAL_VAULT_OPERATIONS)
        );

        let overflow = vault_registration(
            FINGERPRINT_2,
            "018f47a2-8a71-7f4a-9c35-1f4234a73313",
            "Overflow",
        );
        assert!(matches!(
            store.begin_vault_registration_operation(
                &overflow,
                &UtcTimestamp::parse("2026-08-30T07:30:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultOperationJournalFull)
        ));
    }

    #[test]
    fn file_vault_operation_survives_close_and_reopens_in_recovery_queue() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("capacity.sqlite3");
        let operation = {
            let mut store = CapacityStore::open(&path).expect("open file store");
            begin_registration_operation(
                &mut store,
                &vault_registration(
                    FINGERPRINT,
                    "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                    "Primary ChatGPT",
                ),
                "2026-08-30T07:20:00Z",
            )
        };

        let reopened = CapacityStore::open(&path).expect("reopen file store");
        assert_eq!(
            reopened
                .recoverable_vault_operations(MAX_VAULT_RECOVERY_QUERY)
                .expect("recovery queue after reopen"),
            vec![operation]
        );
    }

    #[test]
    fn vault_registration_is_bounded_idempotent_and_rejects_record_aliasing() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let registration = vault_registration(
            FINGERPRINT,
            "018f47a2-8a71-7f4a-9c35-1f4234a73312",
            "Primary ChatGPT",
        );
        let created = register_account(&mut store, &registration, "2026-08-30T06:10:00Z");
        assert!(created.account_id.as_str().starts_with("account:v1:"));
        assert_eq!(created.revision, 1);
        assert_eq!(created.display_order, 0);
        assert_eq!(
            store.vault_account(&created.account_id).unwrap(),
            Some(created.clone())
        );
        assert_eq!(
            store
                .vault_account_by_fingerprint(&created.account_fingerprint)
                .unwrap(),
            Some(created.clone())
        );
        assert_eq!(
            store
                .vault_account_by_record_ref(&created.protected_record_ref)
                .unwrap(),
            Some(created.clone())
        );

        let mut duplicate = registration.clone();
        duplicate.display_name = "Ignored duplicate label".into();
        duplicate.protected_record_ref =
            VaultRecordRef::parse("vault-record:v1:018f47a2-8a71-7f4a-9c35-1f4234a73313")
                .expect("alternate record ref");
        let outcome = store
            .register_vault_account(
                &duplicate,
                &UtcTimestamp::parse("2026-08-30T06:11:00Z").expect("timestamp"),
            )
            .expect("idempotent registration");
        assert_eq!(
            outcome,
            VaultAccountRegistrationOutcome::Existing(created.clone())
        );
        assert_eq!(table_count(&store.connection, "vault_accounts").unwrap(), 1);

        let aliased = vault_registration(
            FINGERPRINT_2,
            "018f47a2-8a71-7f4a-9c35-1f4234a73312",
            "Second ChatGPT",
        );
        assert!(matches!(
            store.register_vault_account(
                &aliased,
                &UtcTimestamp::parse("2026-08-30T06:12:00Z").expect("timestamp")
            ),
            Err(StoreError::VaultRecordConflict)
        ));

        let catalog = store
            .vault_catalog("macos-arm64-native")
            .expect("read catalog");
        assert_eq!(catalog.accounts, vec![created]);
        assert!(catalog.environment_state.is_none());
    }

    #[test]
    fn vault_account_mutations_use_revision_guards() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let account = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T06:10:00Z",
        );
        let renamed = store
            .rename_vault_account(
                &account.account_id,
                1,
                "Work ChatGPT",
                &UtcTimestamp::parse("2026-08-30T06:11:00Z").expect("timestamp"),
            )
            .expect("rename account");
        let VaultAccountMutationOutcome::Updated(renamed) = renamed else {
            panic!("current revision must rename");
        };
        assert_eq!(renamed.display_name, "Work ChatGPT");
        assert_eq!(renamed.revision, 2);

        let stale = store
            .rename_vault_account(
                &account.account_id,
                1,
                "Stale overwrite",
                &UtcTimestamp::parse("2026-08-30T06:12:00Z").expect("timestamp"),
            )
            .expect("stale rename outcome");
        assert_eq!(
            stale,
            VaultAccountMutationOutcome::RevisionConflict(renamed.clone())
        );

        let used = store
            .note_vault_account_used(
                &account.account_id,
                2,
                &UtcTimestamp::parse("2026-08-30T06:13:00Z").expect("timestamp"),
            )
            .expect("note account used");
        let VaultAccountMutationOutcome::Updated(used) = used else {
            panic!("current revision must update last-used time");
        };
        assert_eq!(used.revision, 3);
        assert_eq!(
            used.last_used_at.as_ref().map(UtcTimestamp::as_str),
            Some("2026-08-30T06:13:00Z")
        );

        let reviewed = store
            .set_vault_account_lifecycle(
                &account.account_id,
                3,
                VaultAccountLifecycle::NeedsReview,
                &UtcTimestamp::parse("2026-08-30T06:14:00Z").expect("timestamp"),
            )
            .expect("set lifecycle");
        let VaultAccountMutationOutcome::Updated(reviewed) = reviewed else {
            panic!("current revision must update lifecycle");
        };
        assert_eq!(reviewed.lifecycle, VaultAccountLifecycle::NeedsReview);
        assert_eq!(reviewed.revision, 4);

        assert!(matches!(
            store.rename_vault_account(
                &account.account_id,
                4,
                " padded ",
                &UtcTimestamp::parse("2026-08-30T06:15:00Z").expect("timestamp")
            ),
            Err(StoreError::InvalidVaultOperation("display_name"))
        ));
    }

    #[test]
    fn fingerprint_cascade_updates_vault_metadata_and_history_atomically() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let account = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T06:10:00Z",
        );
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot("2026-08-30T06:11:00Z"),
                &observation("2026-08-30T06:11:00Z"),
            )
            .expect("persist old-key history");

        assert_eq!(
            store
                .vault_key_dependency_audit(account.account_fingerprint.key_id())
                .expect("old-key audit"),
            VaultKeyDependencyAudit {
                vault_accounts: 1,
                account_bindings: 1,
                unmanaged_account_bindings: 0,
                quota_snapshots: 1,
                active_vault_operations: 0,
                active_key_rotations: 0,
            }
        );

        let replacement =
            AccountFingerprint::parse(ROTATED_FINGERPRINT).expect("rotated fingerprint");
        let changed_at = UtcTimestamp::parse("2026-08-30T06:12:00Z").expect("changed at");
        let mut stale = account.clone();
        stale.revision = 2;
        assert_eq!(
            store
                .cascade_vault_account_fingerprint(&stale, &replacement, &changed_at)
                .expect("typed stale outcome"),
            VaultFingerprintCascadeOutcome::RevisionConflict(account.clone())
        );
        let same_key_rewrite =
            AccountFingerprint::parse(FINGERPRINT_2).expect("same-key fingerprint");
        assert!(matches!(
            store.cascade_vault_account_fingerprint(&account, &same_key_rewrite, &changed_at),
            Err(StoreError::InvalidVaultOperation("rotation_fingerprint"))
        ));
        assert!(matches!(
            store.cascade_vault_account_fingerprint(
                &account,
                &replacement,
                &UtcTimestamp::parse("2026-08-30T06:09:59Z").expect("older timestamp")
            ),
            Err(StoreError::InvalidVaultOperation("updated_at"))
        ));
        let outcome = store
            .cascade_vault_account_fingerprint(&account, &replacement, &changed_at)
            .expect("cascade fingerprint");
        let VaultFingerprintCascadeOutcome::Updated(report) = outcome else {
            panic!("first cascade must update");
        };
        assert_eq!(report.account_bindings, 1);
        assert_eq!(report.quota_snapshots, 1);
        assert_eq!(report.account.account_fingerprint, replacement);
        assert_eq!(report.account.revision, 2);
        assert_eq!(report.account.updated_at, changed_at);
        assert!(
            store
                .vault_account_by_fingerprint(&account.account_fingerprint)
                .expect("old account lookup")
                .is_none()
        );
        assert_eq!(
            store
                .vault_account_by_fingerprint(&replacement)
                .expect("new account lookup"),
            Some(report.account.clone())
        );
        assert!(
            store
                .history(
                    "macos-arm64-native",
                    &account.account_fingerprint,
                    "codex:primary",
                    100,
                )
                .expect("old history")
                .is_empty()
        );
        assert_eq!(
            store
                .history("macos-arm64-native", &replacement, "codex:primary", 100,)
                .expect("new history")
                .len(),
            1
        );
        assert_eq!(
            store
                .vault_key_dependency_audit(account.account_fingerprint.key_id())
                .expect("retired-key audit"),
            VaultKeyDependencyAudit {
                vault_accounts: 0,
                account_bindings: 0,
                unmanaged_account_bindings: 0,
                quota_snapshots: 0,
                active_vault_operations: 0,
                active_key_rotations: 0,
            }
        );

        assert_eq!(
            store
                .cascade_vault_account_fingerprint(&account, &replacement, &changed_at)
                .expect("exact retry"),
            VaultFingerprintCascadeOutcome::AlreadyApplied(report.account)
        );
    }

    #[test]
    fn dependency_audit_exposes_unmanaged_history_and_active_journal_work() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let registration = vault_registration(
            FINGERPRINT,
            "018f47a2-8a71-7f4a-9c35-1f4234a73312",
            "Primary ChatGPT",
        );
        let account = register_account(&mut store, &registration, "2026-08-30T06:10:00Z");
        begin_registration_operation(&mut store, &registration, "2026-08-30T06:11:00Z");
        let unmanaged = AccountFingerprint::parse(FINGERPRINT_2).expect("unmanaged fingerprint");
        store
            .record_snapshot(
                &AccountBinding::Stable(unmanaged),
                &snapshot("2026-08-30T06:12:00Z"),
                &observation("2026-08-30T06:12:00Z"),
            )
            .expect("persist unmanaged history");

        assert_eq!(
            store
                .vault_key_dependency_audit(account.account_fingerprint.key_id())
                .expect("dependency audit"),
            VaultKeyDependencyAudit {
                vault_accounts: 1,
                account_bindings: 1,
                unmanaged_account_bindings: 1,
                quota_snapshots: 1,
                active_vault_operations: 1,
                active_key_rotations: 0,
            }
        );
        let replacement =
            AccountFingerprint::parse(ROTATED_FINGERPRINT).expect("rotated fingerprint");
        assert!(matches!(
            store.cascade_vault_account_fingerprint(
                &account,
                &replacement,
                &UtcTimestamp::parse("2026-08-30T06:13:00Z").expect("changed at")
            ),
            Err(StoreError::VaultOperationConflict)
        ));
        assert!(matches!(
            store.vault_key_dependency_audit("not-a-key-id"),
            Err(StoreError::InvalidVaultOperation("installation_key_id"))
        ));
    }

    #[test]
    fn fingerprint_cascade_rejects_binding_collision_without_mutation() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let account = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T06:10:00Z",
        );
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot("2026-08-30T06:11:00Z"),
                &observation("2026-08-30T06:11:00Z"),
            )
            .expect("persist old binding");
        let replacement =
            AccountFingerprint::parse(ROTATED_FINGERPRINT).expect("rotated fingerprint");
        store
            .record_snapshot(
                &AccountBinding::Stable(replacement.clone()),
                &snapshot("2026-08-30T06:12:00Z"),
                &observation("2026-08-30T06:12:00Z"),
            )
            .expect("persist colliding binding");

        assert_eq!(
            store
                .cascade_vault_account_fingerprint(
                    &account,
                    &replacement,
                    &UtcTimestamp::parse("2026-08-30T06:13:00Z").expect("changed at"),
                )
                .expect("typed collision"),
            VaultFingerprintCascadeOutcome::BindingConflict
        );
        assert_eq!(
            store.vault_account(&account.account_id).unwrap(),
            Some(account)
        );
        assert_eq!(
            table_count(&store.connection, "account_bindings").unwrap(),
            2
        );
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            2
        );
    }

    #[test]
    fn injected_account_update_failure_rolls_back_binding_and_history_cascade() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let account = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T06:10:00Z",
        );
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot("2026-08-30T06:11:00Z"),
                &observation("2026-08-30T06:11:00Z"),
            )
            .expect("persist history");
        store
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER reject_fingerprint_rotation
                 BEFORE UPDATE OF account_fingerprint ON vault_accounts
                 BEGIN
                     SELECT RAISE(ABORT, 'injected fingerprint rotation failure');
                 END;",
            )
            .expect("install failure trigger");
        let replacement =
            AccountFingerprint::parse(ROTATED_FINGERPRINT).expect("rotated fingerprint");
        let error = store
            .cascade_vault_account_fingerprint(
                &account,
                &replacement,
                &UtcTimestamp::parse("2026-08-30T06:12:00Z").expect("changed at"),
            )
            .expect_err("injected failure");
        assert!(matches!(error, StoreError::Database(_)));
        assert_eq!(
            store.vault_account(&account.account_id).unwrap(),
            Some(account.clone())
        );
        assert_eq!(
            store
                .history(
                    "macos-arm64-native",
                    &account.account_fingerprint,
                    "codex:primary",
                    100,
                )
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .history("macos-arm64-native", &replacement, "codex:primary", 100,)
                .unwrap()
                .is_empty()
        );
        let rendered = error.to_string();
        assert!(!rendered.contains(FINGERPRINT));
        assert!(!rendered.contains(ROTATED_FINGERPRINT));
    }

    #[test]
    fn vault_selection_observation_and_removal_are_environment_scoped() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let first = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T06:10:00Z",
        );
        let second = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT_2,
                "018f47a2-8a71-7f4a-9c35-1f4234a73313",
                "Backup ChatGPT",
            ),
            "2026-08-30T06:11:00Z",
        );
        let missing = VaultAccountId::parse("account:v1:018f47a2-8a71-7f4a-9c35-1f4234a73399")
            .expect("missing account ID");
        let missing_outcome = store
            .set_vault_environment_state(
                &vault_environment(),
                0,
                Some(&missing),
                None,
                &UtcTimestamp::parse("2026-08-30T06:12:00Z").expect("timestamp"),
            )
            .expect("missing account outcome");
        assert_eq!(
            missing_outcome,
            VaultEnvironmentStateUpdateOutcome::MissingAccount(missing)
        );
        assert_eq!(table_count(&store.connection, "environments").unwrap(), 0);

        let created = store
            .set_vault_environment_state(
                &vault_environment(),
                0,
                Some(&first.account_id),
                Some(&second.account_id),
                &UtcTimestamp::parse("2026-08-30T06:13:00Z").expect("timestamp"),
            )
            .expect("create environment state");
        let VaultEnvironmentStateUpdateOutcome::Updated(created) = created else {
            panic!("revision zero must create missing state");
        };
        assert_eq!(created.revision, 1);
        assert_eq!(created.selected_account_id, Some(first.account_id.clone()));
        assert_eq!(created.observed_account_id, Some(second.account_id.clone()));

        let stale = store
            .set_vault_environment_state(
                &vault_environment(),
                0,
                Some(&second.account_id),
                Some(&second.account_id),
                &UtcTimestamp::parse("2026-08-30T06:14:00Z").expect("timestamp"),
            )
            .expect("stale environment state outcome");
        assert_eq!(
            stale,
            VaultEnvironmentStateUpdateOutcome::RevisionConflict(Some(created))
        );

        let updated = store
            .set_vault_environment_state(
                &vault_environment(),
                1,
                Some(&second.account_id),
                Some(&second.account_id),
                &UtcTimestamp::parse("2026-08-30T06:15:00Z").expect("timestamp"),
            )
            .expect("update environment state");
        let VaultEnvironmentStateUpdateOutcome::Updated(updated) = updated else {
            panic!("current revision must update state");
        };
        assert_eq!(updated.revision, 2);

        let removal = store
            .remove_vault_account_metadata(
                &second.account_id,
                1,
                &UtcTimestamp::parse("2026-08-30T06:16:00Z").expect("timestamp"),
            )
            .expect("remove account metadata");
        let VaultAccountRemovalOutcome::Removed(removal) = removal else {
            panic!("current account revision must remove metadata");
        };
        assert_eq!(removal.account, second);
        assert_eq!(removal.cleared_environment_states, 1);

        let catalog = store
            .vault_catalog("macos-arm64-native")
            .expect("read catalog");
        assert_eq!(catalog.accounts, vec![first]);
        let state = catalog.environment_state.expect("environment state");
        assert_eq!(state.revision, 3);
        assert!(state.selected_account_id.is_none());
        assert!(state.observed_account_id.is_none());
    }

    #[test]
    fn vault_reorder_requires_an_exact_duplicate_free_catalog() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let first = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T06:10:00Z",
        );
        let second = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT_2,
                "018f47a2-8a71-7f4a-9c35-1f4234a73313",
                "Backup ChatGPT",
            ),
            "2026-08-30T06:11:00Z",
        );
        let reordered = store
            .reorder_vault_accounts(
                &[second.account_id.clone(), first.account_id.clone()],
                &UtcTimestamp::parse("2026-08-30T06:12:00Z").expect("timestamp"),
            )
            .expect("reorder accounts");
        let VaultReorderOutcome::Updated(reordered) = reordered else {
            panic!("exact catalog must reorder");
        };
        assert_eq!(reordered[0].account_id, second.account_id);
        assert_eq!(reordered[0].display_order, 0);
        assert_eq!(reordered[0].revision, 2);
        assert_eq!(reordered[1].account_id, first.account_id);
        assert_eq!(reordered[1].display_order, 1);
        assert_eq!(reordered[1].revision, 2);

        let repeated = store
            .reorder_vault_accounts(
                &[second.account_id.clone(), first.account_id.clone()],
                &UtcTimestamp::parse("2026-08-30T06:12:30Z").expect("timestamp"),
            )
            .expect("repeat exact order");
        assert_eq!(repeated, VaultReorderOutcome::Updated(reordered));

        let conflict = store
            .reorder_vault_accounts(
                std::slice::from_ref(&first.account_id),
                &UtcTimestamp::parse("2026-08-30T06:13:00Z").expect("timestamp"),
            )
            .expect("catalog conflict");
        assert!(matches!(conflict, VaultReorderOutcome::CatalogConflict(_)));
        assert!(matches!(
            store.reorder_vault_accounts(
                &[first.account_id.clone(), first.account_id.clone()],
                &UtcTimestamp::parse("2026-08-30T06:14:00Z").expect("timestamp")
            ),
            Err(StoreError::InvalidVaultOperation("duplicate_account_id"))
        ));
    }

    #[test]
    fn stable_live_snapshot_round_trips_as_basis_points() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let outcome = store
            .record_snapshot(
                &stable_binding(),
                &snapshot("2026-08-30T02:55:53Z"),
                &observation("2026-08-30T02:55:53Z"),
            )
            .expect("persist snapshot");
        assert!(matches!(
            outcome,
            SnapshotPersistenceOutcome::Persisted { .. }
        ));

        let fingerprint = AccountFingerprint::parse(FINGERPRINT).expect("fingerprint");
        let history = store
            .history("macos-arm64-native", &fingerprint, "codex:primary", 100)
            .expect("read history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].used_percent, 37.12);
        assert_eq!(history[0].remaining_percent, 62.88);
        assert_eq!(history[0].compatibility, Compatibility::Tested);
    }

    fn record_history_sample(
        store: &mut CapacityStore,
        captured_at: &str,
        remaining: f64,
        reset: Option<&str>,
    ) {
        let mut value = snapshot(captured_at);
        value.quota.windows[0].remaining_percent = remaining;
        value.quota.windows[0].used_percent = 100.0 - remaining;
        value.quota.windows[0].resets_at = reset.map(|value| UtcTimestamp::parse(value).unwrap());
        store
            .record_snapshot(&stable_binding(), &value, &observation(captured_at))
            .unwrap();
    }

    #[test]
    fn quota_changes_use_raw_followups_before_hourly_reduction() {
        use capacity_domain::quota_change::QuotaRiseClassification;
        let mut store = CapacityStore::open_in_memory().unwrap();
        let old = Some("2026-09-17T00:00:00Z");
        let new = Some("2026-09-18T00:00:00Z");
        for (time, remaining, reset) in [
            ("2026-09-10T10:00:00Z", 18.4, old),
            ("2026-09-10T10:05:00Z", 99.2, new),
            ("2026-09-10T10:06:00Z", 98.7, new),
            ("2026-09-10T10:59:00Z", 97.0, new),
        ] {
            record_history_sample(&mut store, time, remaining, reset);
        }
        let fingerprint = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let overview = store
            .history_overview("macos-arm64-native", &fingerprint, "codex:primary")
            .unwrap();
        // The middle follow-up is neither an extremum nor the last plotted point.
        assert_eq!(overview.points.len(), 3);
        let item = &overview.quota_changes.observations[0];
        assert_eq!(
            item.classification,
            QuotaRiseClassification::BeforeScheduledBoundary
        );
        assert_eq!(
            item.confirmed_at.as_ref().unwrap().as_str(),
            "2026-09-10T10:06:00Z"
        );
        assert_eq!(item.confirmed_remaining_percent, Some(98.7));
        assert_eq!(overview.quota_changes.valid_samples, 4);
        assert_eq!(
            store
                .history("macos-arm64-native", &fingerprint, "codex:primary", 100)
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn quota_changes_do_not_use_another_accounts_followup_or_create_mutations() {
        use capacity_domain::quota_change::QuotaRiseClassification;
        let mut store = CapacityStore::open_in_memory().unwrap();
        record_history_sample(
            &mut store,
            "2026-09-10T10:00:00Z",
            20.0,
            Some("2026-09-17T00:00:00Z"),
        );
        record_history_sample(
            &mut store,
            "2026-09-10T10:05:00Z",
            99.0,
            Some("2026-09-18T00:00:00Z"),
        );
        let mut other = snapshot("2026-09-10T10:06:00Z");
        other.quota.windows[0].remaining_percent = 98.0;
        other.quota.windows[0].used_percent = 2.0;
        other.quota.windows[0].resets_at =
            Some(UtcTimestamp::parse("2026-09-18T00:00:00Z").unwrap());
        store
            .record_snapshot(
                &AccountBinding::Stable(AccountFingerprint::parse(FINGERPRINT_2).unwrap()),
                &other,
                &observation("2026-09-10T10:06:00Z"),
            )
            .unwrap();
        let changes_before = store.connection.total_changes();
        let first = store
            .history_overview(
                "macos-arm64-native",
                &AccountFingerprint::parse(FINGERPRINT).unwrap(),
                "codex:primary",
            )
            .unwrap();
        let second = store
            .history_overview(
                "macos-arm64-native",
                &AccountFingerprint::parse(FINGERPRINT_2).unwrap(),
                "codex:primary",
            )
            .unwrap();
        assert_eq!(
            first.quota_changes.observations[0].classification,
            QuotaRiseClassification::AwaitingFollowup
        );
        assert!(second.quota_changes.observations.is_empty());
        assert_eq!(store.connection.total_changes(), changes_before);
    }

    #[test]
    fn history_overview_is_time_bounded_not_recent_sample_bounded_and_is_account_scoped() {
        let mut store = CapacityStore::open_in_memory().unwrap();
        let reset = Some("2026-09-17T00:00:00Z");
        record_history_sample(&mut store, "2026-08-01T00:00:00Z", 99.0, reset);
        record_history_sample(&mut store, "2026-08-20T00:00:00Z", 80.0, reset);
        for i in 0..180 {
            let time = format!("2026-09-10T10:{:02}:{:02}Z", i / 60, i % 60);
            record_history_sample(&mut store, &time, 29.0, reset);
        }
        let other = AccountBinding::Stable(AccountFingerprint::parse(FINGERPRINT_2).unwrap());
        store
            .record_snapshot(
                &other,
                &snapshot("2026-10-30T00:00:00Z"),
                &observation("2026-10-30T00:00:00Z"),
            )
            .unwrap();
        let fingerprint = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let overview = store
            .history_overview("macos-arm64-native", &fingerprint, "codex:primary")
            .unwrap();
        assert_eq!(overview.sample_count, 181);
        assert_eq!(overview.points.len(), 3);
        assert_eq!(
            overview.points[0].captured_at.as_str(),
            "2026-08-20T00:00:00Z"
        );
        assert_eq!(
            overview.points.last().unwrap().captured_at.as_str(),
            "2026-09-10T10:02:59Z"
        );
        assert_eq!(
            overview
                .daily_activity
                .iter()
                .map(|day| day.sample_count)
                .sum::<u64>(),
            181
        );
        // Legacy callers still receive raw samples; no observations were pruned.
        assert_eq!(
            store
                .history("macos-arm64-native", &fingerprint, "codex:primary", 1000)
                .unwrap()
                .len(),
            182
        );
        assert_eq!(
            store
                .history_overview("other-environment", &fingerprint, "codex:primary")
                .unwrap(),
            HistoryOverview::default()
        );
        assert_eq!(
            store
                .history_overview("macos-arm64-native", &fingerprint, "other:window")
                .unwrap(),
            HistoryOverview::default()
        );
    }

    #[test]
    fn local_codex_overview_survives_updates_without_rewriting_or_merging_accounts() {
        let mut store = CapacityStore::open_in_memory().unwrap();
        let before = "macos:local:codex-executable-0123456789abcdef";
        let after = "macos:local:codex-executable-fedcba9876543210";
        for (environment, time, remaining, fingerprint) in [
            (before, "2026-09-10T10:00:00Z", 30.0, FINGERPRINT),
            (after, "2026-09-10T10:10:00Z", 29.0, FINGERPRINT),
            (before, "2026-10-30T00:00:00Z", 80.0, FINGERPRINT_2),
        ] {
            let mut value = snapshot(time);
            value.environment.environment_id = environment.into();
            value.quota.windows[0].remaining_percent = remaining;
            value.quota.windows[0].used_percent = 100.0 - remaining;
            let mut compatibility = observation(time);
            compatibility.environment_id = environment.into();
            let binding = AccountBinding::Stable(AccountFingerprint::parse(fingerprint).unwrap());
            store
                .record_snapshot(&binding, &value, &compatibility)
                .unwrap();
        }
        let fingerprint = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let overview = store
            .local_codex_history_overview(after, &fingerprint, "codex:primary")
            .unwrap();
        assert_eq!(overview.sample_count, 2);
        assert_eq!(
            overview
                .points
                .iter()
                .map(|point| point.remaining_percent)
                .collect::<Vec<_>>(),
            [30.0, 29.0]
        );
        assert_eq!(overview.daily_activity[0].consumed_percent, 1.0);
        assert_eq!(overview.daily_activity[0].comparable_intervals, 1);
        assert!(
            store
                .local_codex_history_overview(after, &fingerprint, "other:window")
                .unwrap()
                .points
                .is_empty()
        );
        // Strict consumers and the stored per-binary provenance are unchanged.
        assert_eq!(
            store
                .history_overview(after, &fingerprint, "codex:primary")
                .unwrap()
                .sample_count,
            1
        );
        assert_eq!(
            store
                .history(before, &fingerprint, "codex:primary", 100)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            3
        );
        assert_eq!(table_count(&store.connection, "environments").unwrap(), 2);
    }

    #[test]
    fn local_codex_overview_never_crosses_native_platform_architecture_or_unknown_environment() {
        let mut store = CapacityStore::open_in_memory().unwrap();
        let cases = [
            (
                "macos:local:codex-executable-0000000000000001",
                "macos",
                "arm64",
                "native",
            ),
            (
                "macos:local:codex-executable-0000000000000002",
                "macos",
                "arm64",
                "native",
            ),
            (
                "macos:local:codex-executable-0000000000000003",
                "macos",
                "x86_64",
                "native",
            ),
            (
                "macos:local:codex-executable-0000000000000004",
                "macos",
                "arm64",
                "remote",
            ),
            (
                "macos:local:codex-executable-0000000000000005",
                "linux",
                "arm64",
                "native",
            ),
            (
                "windows:local:codex-executable-0000000000000006",
                "windows",
                "arm64",
                "native",
            ),
            (
                "macos:local:codex-executable-0000000000000007-extra",
                "macos",
                "arm64",
                "native",
            ),
            (
                "macos:local:codex-executable-000000000000000g",
                "macos",
                "arm64",
                "native",
            ),
            ("custom-environment", "macos", "arm64", "native"),
        ];
        for (i, (id, platform, architecture, boundary)) in cases.into_iter().enumerate() {
            let time = format!("2026-09-10T10:{i:02}:00Z");
            let mut value = snapshot(&time);
            value.environment = EnvironmentSnapshot {
                environment_id: id.into(),
                platform: platform.into(),
                architecture: architecture.into(),
                boundary: boundary.into(),
            };
            let mut compatibility = observation(&time);
            compatibility.environment_id = id.into();
            store
                .record_snapshot(&stable_binding(), &value, &compatibility)
                .unwrap();
        }
        let fingerprint = AccountFingerprint::parse(FINGERPRINT).unwrap();
        for (i, (id, _, _, _)) in cases.into_iter().enumerate() {
            let overview = store
                .local_codex_history_overview(id, &fingerprint, "codex:primary")
                .unwrap();
            assert_eq!(overview.sample_count, if i < 2 { 2 } else { 1 }, "case {i}");
        }
        assert!(
            store
                .local_codex_history_overview("missing-environment", &fingerprint, "codex:primary")
                .unwrap()
                .points
                .is_empty()
        );
    }

    #[test]
    fn native_history_family_requires_the_runtime_id_shape() {
        for platform in ["macos", "windows", "linux"] {
            assert!(native_codex_environment_id(
                &format!("{platform}:local:codex-executable-0123456789abcdef"),
                platform
            ));
        }
        for invalid in [
            "",
            "macos:local:codex-executable-",
            "macos:local:codex-executable-0123456789abcdeF",
            "macos:local:codex-executable-0123456789abcdef0",
            "macos:local:codex-executable-0123456789abcde_",
            "macos:remote:codex-executable-0123456789abcdef",
        ] {
            assert!(!native_codex_environment_id(invalid, "macos"));
        }
        assert!(!native_codex_environment_id(
            "unknown:local:codex-executable-0123456789abcdef",
            "unknown"
        ));
    }

    #[test]
    fn history_overview_preserves_hourly_extrema_and_computes_usage_before_reduction() {
        let mut store = CapacityStore::open_in_memory().unwrap();
        let reset = Some("2026-09-17T00:00:00Z");
        for (minute, remaining) in [80.0, 79.0, 82.0, 78.0, 83.0, 77.0].into_iter().enumerate() {
            record_history_sample(
                &mut store,
                &format!("2026-09-10T10:{minute:02}:00Z"),
                remaining,
                reset,
            );
        }
        let fingerprint = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let overview = store
            .history_overview("macos-arm64-native", &fingerprint, "codex:primary")
            .unwrap();
        assert_eq!(
            overview
                .points
                .iter()
                .map(|point| point.remaining_percent)
                .collect::<Vec<_>>(),
            [80.0, 83.0, 77.0]
        );
        assert_eq!(overview.sample_count, 6);
        assert_eq!(overview.daily_activity.len(), 1);
        assert_eq!(overview.daily_activity[0].consumed_percent, 11.0);
        assert_eq!(overview.daily_activity[0].comparable_intervals, 5);
    }

    #[test]
    fn history_overview_activity_does_not_guess_across_midnight_gaps_or_reset_changes() {
        let mut store = CapacityStore::open_in_memory().unwrap();
        let reset = Some("2026-09-17T00:00:00Z");
        for (time, remaining, boundary) in [
            ("2026-09-09T23:50:00", 70.0, reset),
            ("2026-09-10T00:10:00", 60.0, reset),
            ("2026-09-10T02:00:00", 50.0, reset),
            ("2026-09-10T02:10:00", 40.0, Some("2026-09-18T00:00:00Z")),
            ("2026-09-10T02:20:00", 30.0, None),
        ] {
            // Convert fixture-local wall time to UTC without changing process TZ.
            let utc: String = store
                .connection
                .query_row(
                    "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', ?1, 'utc')",
                    [time],
                    |row| row.get(0),
                )
                .unwrap();
            record_history_sample(&mut store, &utc, remaining, boundary);
        }
        let fingerprint = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let overview = store
            .history_overview("macos-arm64-native", &fingerprint, "codex:primary")
            .unwrap();
        assert_eq!(
            overview
                .daily_activity
                .iter()
                .map(|day| day.date.as_str())
                .collect::<Vec<_>>(),
            ["2026-09-09", "2026-09-10"]
        );
        assert!(
            overview
                .daily_activity
                .iter()
                .all(|day| day.comparable_intervals == 0 && day.consumed_percent == 0.0)
        );
    }

    #[test]
    fn history_overview_repeated_local_hour_keeps_both_utc_hours_and_observed_precision() {
        let mut store = CapacityStore::open_in_memory().unwrap();
        for (time, remaining) in [
            ("2026-11-01T05:50:00Z", 28.4),
            ("2026-11-01T06:10:00Z", 28.1),
            ("2026-11-01T06:30:00Z", 28.1),
        ] {
            record_history_sample(&mut store, time, remaining, Some("2026-11-07T00:00:00Z"));
        }
        let overview = store
            .history_overview(
                "macos-arm64-native",
                &AccountFingerprint::parse(FINGERPRINT).unwrap(),
                "codex:primary",
            )
            .unwrap();
        assert_eq!(overview.points.len(), 3);
        assert_eq!(
            overview
                .daily_activity
                .iter()
                .map(|day| day.consumed_percent)
                .sum::<f64>(),
            0.3
        );
        assert_eq!(
            overview
                .daily_activity
                .iter()
                .map(|day| day.comparable_intervals)
                .sum::<u64>(),
            2
        );
    }

    #[test]
    fn ephemeral_and_unavailable_bindings_never_create_rows() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let session = capacity_domain::EphemeralSessionId::parse(
            "session:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311",
        )
        .expect("session ID");
        let ephemeral = store
            .record_snapshot(
                &AccountBinding::Ephemeral(session),
                &snapshot("2026-08-30T02:55:53Z"),
                &observation("2026-08-30T02:55:53Z"),
            )
            .expect("ephemeral decision");
        let unavailable = store
            .record_snapshot(
                &AccountBinding::Unavailable,
                &snapshot("2026-08-30T02:56:53Z"),
                &observation("2026-08-30T02:56:53Z"),
            )
            .expect("unavailable decision");
        assert_eq!(
            ephemeral,
            SnapshotPersistenceOutcome::Skipped(SnapshotPersistenceSkip::EphemeralBinding)
        );
        assert_eq!(
            unavailable,
            SnapshotPersistenceOutcome::Skipped(SnapshotPersistenceSkip::UnavailableBinding)
        );
        assert_eq!(table_count(&store.connection, "environments").unwrap(), 0);
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            0
        );
    }

    #[test]
    fn stale_snapshot_is_not_history() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let mut stale = snapshot("2026-08-30T02:55:53Z");
        stale.data_status.freshness = Freshness::Stale;
        let outcome = store
            .record_snapshot(
                &stable_binding(),
                &stale,
                &observation("2026-08-30T02:55:53Z"),
            )
            .expect("stale decision");
        assert_eq!(
            outcome,
            SnapshotPersistenceOutcome::Skipped(SnapshotPersistenceSkip::NotLive)
        );
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            0
        );
    }

    #[test]
    fn environment_id_cannot_cross_platform_boundary() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot("2026-08-30T02:55:53Z"),
                &observation("2026-08-30T02:55:53Z"),
            )
            .expect("first snapshot");

        let mut conflicting = snapshot("2026-08-30T02:56:53Z");
        conflicting.environment.platform = "windows".into();
        let error = store
            .record_snapshot(
                &stable_binding(),
                &conflicting,
                &observation("2026-08-30T02:56:53Z"),
            )
            .expect_err("environment conflict must fail");
        assert!(matches!(error, StoreError::EnvironmentConflict));
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            1
        );
    }

    #[test]
    fn transaction_rolls_back_every_row_after_injected_window_failure() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        store
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER reject_window
                 BEFORE INSERT ON quota_windows
                 BEGIN
                     SELECT RAISE(ABORT, 'injected window failure');
                 END;",
            )
            .expect("install failure trigger");
        assert!(
            store
                .record_snapshot(
                    &stable_binding(),
                    &snapshot("2026-08-30T02:55:53Z"),
                    &observation("2026-08-30T02:55:53Z"),
                )
                .is_err()
        );
        for table in [
            "environments",
            "account_bindings",
            "compatibility_observations",
            "quota_snapshots",
            "quota_windows",
            "reset_credit_summaries",
            "reset_credits",
        ] {
            assert_eq!(table_count(&store.connection, table).unwrap(), 0, "{table}");
        }
    }

    #[test]
    fn schema_rejects_unprotected_identity_and_has_no_secret_columns() {
        let store = CapacityStore::open_in_memory().expect("open store");
        store
            .connection
            .execute(
                "INSERT INTO environments(
                     environment_id, platform, architecture, boundary, created_at, last_seen_at
                 ) VALUES ('env', 'macos', 'arm64', 'native', ?1, ?1)",
                ["2026-08-30T02:55:53Z"],
            )
            .expect("insert environment");
        assert!(
            store
                .connection
                .execute(
                    "INSERT INTO account_bindings(
                         environment_id, account_fingerprint, created_at, last_seen_at
                     ) VALUES ('env', 'person@example.com', ?1, ?1)",
                    ["2026-08-30T02:55:53Z"],
                )
                .is_err()
        );

        for table in [
            "environments",
            "account_bindings",
            "compatibility_observations",
            "quota_snapshots",
            "quota_windows",
            "reset_credit_summaries",
            "reset_credits",
            "settings",
            "vault_accounts",
            "vault_environment_states",
            "vault_key_rotations",
            "vault_operations",
        ] {
            let pragma = format!("PRAGMA table_info({table})");
            let columns: Vec<String> = store
                .connection
                .prepare(&pragma)
                .expect("prepare column query")
                .query_map([], |row| row.get(1))
                .expect("query columns")
                .collect::<Result<_, _>>()
                .expect("read columns");
            for column in columns {
                assert!(
                    ![
                        "token",
                        "cookie",
                        "email",
                        "auth_json",
                        "prompt",
                        "response",
                        "raw",
                        "api_key",
                        "secret"
                    ]
                    .iter()
                    .any(|forbidden| column.contains(forbidden)),
                    "forbidden column {table}.{column}"
                );
            }
        }
    }

    #[test]
    fn delete_all_removes_history_and_restores_only_default_settings() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        let account = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T02:54:53Z",
        );
        store
            .set_vault_environment_state(
                &vault_environment(),
                0,
                Some(&account.account_id),
                Some(&account.account_id),
                &UtcTimestamp::parse("2026-08-30T02:54:53Z").expect("timestamp"),
            )
            .expect("create environment state");
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot("2026-08-30T02:55:53Z"),
                &observation("2026-08-30T02:55:53Z"),
            )
            .expect("persist snapshot");
        store
            .begin_vault_forget_operation(
                &account,
                &UtcTimestamp::parse("2026-08-30T02:56:53Z").expect("timestamp"),
            )
            .expect("begin durable forget");
        let report = store
            .delete_all_database_data()
            .expect("delete database data");
        assert_eq!(report.quota_snapshots, 1);
        assert_eq!(report.quota_windows, 1);
        assert_eq!(report.reset_credits, 1);
        assert_eq!(report.vault_accounts, 1);
        assert_eq!(report.vault_environment_states, 1);
        assert_eq!(report.vault_operations, 1);
        assert_eq!(table_count(&store.connection, "environments").unwrap(), 0);
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            0
        );
        assert_eq!(table_count(&store.connection, "settings").unwrap(), 1);
        assert_eq!(table_count(&store.connection, "vault_accounts").unwrap(), 0);
        assert_eq!(
            table_count(&store.connection, "vault_operations").unwrap(),
            0
        );
        assert_eq!(
            table_count(&store.connection, "vault_environment_states").unwrap(),
            0
        );
        assert_eq!(store.schema_version().unwrap(), STORE_SCHEMA_VERSION);
    }

    #[test]
    fn delete_all_removes_recoverable_key_rotation_intent() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        begin_key_rotation(&mut store, "2026-08-30T08:00:00Z");

        let report = store
            .delete_all_database_data()
            .expect("delete database data");
        assert_eq!(report.vault_key_rotations, 1);
        assert_eq!(
            table_count(&store.connection, "vault_key_rotations").unwrap(),
            0
        );
        assert!(
            store
                .recoverable_vault_key_rotations(MAX_VAULT_RECOVERY_QUERY)
                .expect("empty recovery queue")
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_created_private_and_rejects_permission_drift() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("capacity.sqlite3");
        let store = CapacityStore::open(&path).expect("open file store");
        drop(store);
        let mode = std::fs::metadata(&path)
            .expect("database metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("broaden permissions");
        let error = CapacityStore::open(&path).expect_err("permission drift must fail closed");
        assert!(matches!(error, StoreError::InsecureDatabasePermissions));
    }

    #[cfg(unix)]
    #[test]
    fn file_delete_all_truncates_private_wal_residue() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("capacity.sqlite3");
        let mut store = CapacityStore::open(&path).expect("open file store");
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot("2026-08-30T02:55:53Z"),
                &observation("2026-08-30T02:55:53Z"),
            )
            .expect("persist snapshot");

        for suffix in ["-wal", "-shm"] {
            let sidecar = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
            if sidecar.exists() {
                let mode = std::fs::metadata(&sidecar)
                    .expect("sidecar metadata")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode & 0o077, 0, "private {suffix}");
            }
        }

        store
            .delete_all_database_data()
            .expect("delete database data");
        let wal = std::path::PathBuf::from(format!("{}-wal", path.display()));
        if wal.exists() {
            assert_eq!(std::fs::metadata(wal).expect("WAL metadata").len(), 0);
        }
    }

    #[test]
    fn newer_schema_fails_closed() {
        let connection = Connection::open_in_memory().expect("open SQLite");
        connection
            .pragma_update(None, "user_version", STORE_SCHEMA_VERSION + 1)
            .expect("set future schema");
        configure_connection(&connection, false).expect("configure SQLite");
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };
        let error = store.migrate().expect_err("future schema must fail");
        assert!(matches!(
            error,
            StoreError::UnsupportedSchema {
                found,
                supported: STORE_SCHEMA_VERSION
            } if found == STORE_SCHEMA_VERSION + 1
        ));
    }

    #[test]
    fn failed_migration_rolls_back_every_v1_table() {
        let connection = Connection::open_in_memory().expect("open SQLite");
        connection
            .execute("CREATE TABLE settings(unrelated TEXT)", [])
            .expect("create incompatible preexisting table");
        configure_connection(&connection, false).expect("configure SQLite");
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };
        assert!(store.migrate().is_err());
        assert_eq!(store.schema_version().expect("schema version"), 0);

        let tables: BTreeSet<String> = store
            .connection
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .expect("prepare table query")
            .query_map([], |row| row.get(0))
            .expect("query tables")
            .collect::<Result<_, _>>()
            .expect("read tables");
        assert_eq!(tables, BTreeSet::from(["settings".into()]));
    }

    #[test]
    fn failed_v2_migration_leaves_v1_schema_and_version_intact() {
        let connection = v1_connection();
        connection
            .execute("CREATE TABLE vault_accounts(unrelated TEXT) STRICT", [])
            .expect("inject incompatible v2 table");
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };

        assert!(store.migrate().is_err());
        assert_eq!(store.schema_version().expect("schema version"), 1);
        assert_eq!(
            table_count(&store.connection, "schema_migrations").unwrap(),
            1
        );
        let environment_state_table: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'vault_environment_states'",
                [],
                |row| row.get(0),
            )
            .expect("query v2 table");
        assert_eq!(environment_state_table, 0);
    }

    #[test]
    fn failed_v3_migration_leaves_v2_schema_ledger_and_rows_intact() {
        let connection = v2_connection();
        connection
            .execute_batch(
                "CREATE TEMP TRIGGER reject_v3_ledger
                 BEFORE INSERT ON schema_migrations
                 WHEN NEW.version = 3
                 BEGIN
                     SELECT RAISE(ABORT, 'injected v3 ledger failure');
                 END;",
            )
            .expect("inject v3 failure");
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };
        let account = register_account(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T06:10:00Z",
        );

        assert!(store.migrate().is_err());
        assert_eq!(store.schema_version().expect("schema version"), 2);
        assert_eq!(
            table_count(&store.connection, "schema_migrations").unwrap(),
            2
        );
        assert_eq!(
            store.vault_account(&account.account_id).unwrap(),
            Some(account)
        );
        let operation_table: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'vault_operations'",
                [],
                |row| row.get(0),
            )
            .expect("query v3 table");
        assert_eq!(operation_table, 0);
    }

    #[test]
    fn failed_v4_migration_leaves_v3_schema_ledger_and_recovery_rows_intact() {
        let connection = v3_connection();
        connection
            .execute_batch(
                "CREATE TEMP TRIGGER reject_v4_ledger
                 BEFORE INSERT ON schema_migrations
                 WHEN NEW.version = 4
                 BEGIN
                     SELECT RAISE(ABORT, 'injected v4 ledger failure');
                 END;",
            )
            .expect("inject v4 failure");
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };
        let operation = insert_v3_registration_operation(
            &mut store,
            &vault_registration(
                FINGERPRINT,
                "018f47a2-8a71-7f4a-9c35-1f4234a73312",
                "Primary ChatGPT",
            ),
            "2026-08-30T07:20:00Z",
        );

        assert!(store.migrate().is_err());
        assert_eq!(store.schema_version().expect("schema version"), 3);
        assert_eq!(
            table_count(&store.connection, "schema_migrations").unwrap(),
            3
        );
        assert_eq!(
            store.vault_operation(&operation.operation_id).unwrap(),
            Some(operation)
        );
        let rotation_table: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'vault_key_rotations'",
                [],
                |row| row.get(0),
            )
            .expect("query v4 table");
        assert_eq!(rotation_table, 0);
    }

    #[test]
    fn failed_v5_migration_leaves_v4_schema_and_ledger_intact() {
        let connection = v4_connection();
        connection
            .execute_batch(
                "CREATE TEMP TRIGGER reject_v5_ledger
                 BEFORE INSERT ON schema_migrations
                 WHEN NEW.version = 5
                 BEGIN
                     SELECT RAISE(ABORT, 'injected v5 ledger failure');
                 END;",
            )
            .expect("inject v5 failure");
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };

        assert!(store.migrate().is_err());
        assert_eq!(store.schema_version().expect("schema version"), 4);
        assert_eq!(
            table_count(&store.connection, "schema_migrations").unwrap(),
            4
        );
        let schedule_table: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'work_schedule_settings'",
                [],
                |row| row.get(0),
            )
            .expect("query v5 table");
        assert_eq!(schedule_table, 0);
    }

    #[test]
    fn schema_drift_is_rejected_before_repository_use() {
        let store = CapacityStore::open_in_memory().expect("open store");
        store
            .connection
            .execute("ALTER TABLE settings ADD COLUMN raw_payload TEXT", [])
            .expect("inject schema drift");
        let error = store.verify_schema().expect_err("schema drift must fail");
        assert!(matches!(error, StoreError::SchemaInvariant));
    }

    #[test]
    fn migration_ledger_drift_is_rejected_even_when_user_version_matches() {
        let store = CapacityStore::open_in_memory().expect("open store");
        store
            .connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                params![STORE_SCHEMA_VERSION + 1, "2026-08-30T06:20:00Z"],
            )
            .expect("inject migration ledger drift");
        let error = store.verify_schema().expect_err("ledger drift must fail");
        assert!(matches!(error, StoreError::SchemaInvariant));
    }

    #[test]
    fn prune_is_scoped_to_environment_and_stable_account() {
        let mut store = CapacityStore::open_in_memory().expect("open store");
        for captured_at in ["2026-08-01T00:00:00Z", "2026-08-30T00:00:00Z"] {
            store
                .record_snapshot(
                    &stable_binding(),
                    &snapshot(captured_at),
                    &observation(captured_at),
                )
                .expect("persist snapshot");
        }
        let fingerprint = AccountFingerprint::parse(FINGERPRINT).expect("fingerprint");
        let deleted = store
            .prune_history_before(
                "macos-arm64-native",
                &fingerprint,
                &UtcTimestamp::parse("2026-08-15T00:00:00Z").expect("cutoff"),
            )
            .expect("prune history");
        assert_eq!(deleted, 1);
        assert_eq!(
            store
                .history("macos-arm64-native", &fingerprint, "codex:primary", 100)
                .expect("history")
                .len(),
            1
        );
    }
}
