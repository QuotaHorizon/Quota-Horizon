use std::path::Path;

pub(crate) mod active_time;
pub(crate) mod demand;
pub(crate) mod pace;
pub(crate) mod planning_archive;

use capacity_domain::{
    AccountBinding, AccountBindingStatus, Clock, EphemeralSessionId, StatusSnapshot, SystemClock,
    WorkSchedulePeriod,
};
use capacity_store::{
    CapacityStore, CompatibilityObservationInput, DEFAULT_SETTINGS_REVISION, DeleteDatabaseOutcome,
    HistoryOverview, HistoryPoint, MonitorSettings, MonitorSettingsUpdate, SettingsLanguage,
    SettingsUpdateOutcome, SnapshotPersistenceOutcome, SnapshotPersistenceSkip, StoreError,
    VaultMutationRecoverySummary, WorkScheduleSettings, WorkScheduleSettingsUpdate,
    WorkScheduleSettingsUpdateOutcome,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const HISTORY_SCHEMA_VERSION: &str = "1.0";
const SETTINGS_SCHEMA_VERSION: &str = "1.0";
const DELETE_ALL_SCHEMA_VERSION: &str = "1.0";
const WORK_SCHEDULE_SCHEMA_VERSION: &str = "1.0";
const STORE_FILENAME: &str = "capacity.sqlite3";
const MAX_HISTORY_POINTS: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistenceRecordResult {
    Persisted,
    Skipped(SnapshotPersistenceSkip),
    StoreUnavailable,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopHistoryStatus {
    Available,
    BindingRequired,
    Unavailable,
    Failed,
    InvalidRequest,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopHistoryPoint {
    pub(crate) snapshot_id: String,
    pub(crate) captured_at: String,
    pub(crate) limit_id: String,
    pub(crate) label: Option<String>,
    pub(crate) window_minutes: Option<u64>,
    pub(crate) used_percent: f64,
    pub(crate) remaining_percent: f64,
    pub(crate) resets_at: Option<String>,
    pub(crate) availability: capacity_domain::Availability,
    pub(crate) compatibility: capacity_domain::Compatibility,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopHistoryEnvelope {
    pub(crate) schema_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) history_context_id: Option<String>,
    pub(crate) status: DesktopHistoryStatus,
    pub(crate) reason_code: &'static str,
    pub(crate) points: Vec<DesktopHistoryPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) overview: Option<DesktopHistoryOverview>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopHistoryOverview {
    pub(crate) sample_count: u64,
    pub(crate) daily_activity: Vec<DesktopHistoryDayActivity>,
    pub(crate) quota_changes: capacity_domain::quota_change::QuotaChangeSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopHistoryDayActivity {
    pub(crate) date: String,
    pub(crate) sample_count: u64,
    pub(crate) comparable_intervals: u64,
    pub(crate) consumed_percent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopSettingsStatus {
    Available,
    Updated,
    RevisionConflict,
    InvalidRequest,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopSettingsView {
    pub(crate) revision: u32,
    pub(crate) auto_refresh_enabled: bool,
    pub(crate) refresh_interval_seconds: u32,
    pub(crate) notification_threshold_basis_points: Option<u16>,
    pub(crate) reset_credit_notice_hours: u16,
    pub(crate) quiet_hours_enabled: bool,
    pub(crate) quiet_hours_start_minute: Option<u16>,
    pub(crate) quiet_hours_end_minute: Option<u16>,
    pub(crate) language: &'static str,
    pub(crate) lock_screen_privacy: bool,
    pub(crate) launch_at_login: bool,
    pub(crate) history_retention_days: u16,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopSettingsEnvelope {
    pub(crate) schema_version: &'static str,
    pub(crate) status: DesktopSettingsStatus,
    pub(crate) reason_code: &'static str,
    pub(crate) settings: Option<DesktopSettingsView>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopSettingsUpdateRequest {
    pub(crate) expected_revision: u32,
    pub(crate) auto_refresh_enabled: bool,
    pub(crate) refresh_interval_seconds: u32,
    pub(crate) notification_threshold_basis_points: Option<u16>,
    pub(crate) reset_credit_notice_hours: u16,
    pub(crate) quiet_hours_enabled: bool,
    pub(crate) quiet_hours_start_minute: Option<u16>,
    pub(crate) quiet_hours_end_minute: Option<u16>,
    pub(crate) language: String,
    pub(crate) lock_screen_privacy: bool,
    pub(crate) launch_at_login: bool,
    pub(crate) history_retention_days: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopWorkScheduleStatus {
    Available,
    Updated,
    RevisionConflict,
    InvalidRequest,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopWorkSchedulePeriodView {
    pub(crate) start_minute_of_day: u16,
    pub(crate) end_minute_of_day: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopWorkScheduleView {
    pub(crate) revision: u32,
    pub(crate) enabled: bool,
    pub(crate) off_periods: Vec<DesktopWorkSchedulePeriodView>,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopWorkScheduleEnvelope {
    pub(crate) schema_version: &'static str,
    pub(crate) status: DesktopWorkScheduleStatus,
    pub(crate) reason_code: &'static str,
    pub(crate) schedule: Option<DesktopWorkScheduleView>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopWorkScheduleUpdateRequest {
    pub(crate) expected_revision: u32,
    pub(crate) enabled: bool,
    pub(crate) off_periods: Vec<DesktopWorkSchedulePeriodInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopWorkSchedulePeriodInput {
    pub(crate) start_minute_of_day: u16,
    pub(crate) end_minute_of_day: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopDeleteAllStatus {
    Deleted,
    ConfirmationRequired,
    RevisionConflict,
    InvalidRequest,
    Failed,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopDeleteAllRequest {
    pub(crate) expected_settings_revision: u32,
    pub(crate) confirmed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopDeleteAllEnvelope {
    pub(crate) schema_version: &'static str,
    pub(crate) status: DesktopDeleteAllStatus,
    pub(crate) reason_code: &'static str,
    pub(crate) deleted_record_count: Option<u64>,
    pub(crate) settings_revision: Option<u32>,
}

pub(crate) struct DesktopPersistence {
    store: Option<CapacityStore>,
    binding: AccountBinding,
    last_account_status: AccountBindingStatus,
    environment_id: Option<String>,
    binding_failure_reason: Option<&'static str>,
    // Random, process-local presentation boundary, never an account identifier.
    history_context_id: String,
    pace_trials_failed: bool,
}

impl DesktopPersistence {
    pub(crate) fn open(data_directory: &Path) -> Self {
        let store = prepare_app_data_directory(data_directory)
            .ok()
            .and_then(|()| CapacityStore::open(data_directory.join(STORE_FILENAME)).ok());
        Self {
            store,
            binding: new_ephemeral_binding(),
            last_account_status: AccountBindingStatus::Unavailable,
            environment_id: None,
            binding_failure_reason: None,
            history_context_id: Uuid::new_v4().to_string(),
            pace_trials_failed: false,
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            store: None,
            binding: new_ephemeral_binding(),
            last_account_status: AccountBindingStatus::Unavailable,
            environment_id: None,
            binding_failure_reason: None,
            history_context_id: Uuid::new_v4().to_string(),
            pace_trials_failed: false,
        }
    }

    pub(crate) fn backend_available(&self) -> bool {
        self.store.is_some()
    }

    pub(crate) fn monitor_settings(&self) -> Option<MonitorSettings> {
        self.store.as_ref()?.settings().ok()
    }

    pub(crate) fn vault_mutation_recovery_summary(
        &self,
    ) -> Result<VaultMutationRecoverySummary, &'static str> {
        let store = self.store.as_ref().ok_or("vault_store_unavailable")?;
        store
            .vault_mutation_recovery_summary()
            .map_err(|_| "vault_recovery_summary_failed")
    }

    pub(crate) fn rotate_ephemeral_binding(&mut self) {
        self.pace_trials_failed = false;
        self.interrupt_activity();
        self.binding = new_ephemeral_binding();
        self.history_context_id = Uuid::new_v4().to_string();
        self.last_account_status = AccountBindingStatus::Unavailable;
        self.environment_id = None;
        self.binding_failure_reason = None;
    }

    pub(crate) fn set_binding_failure(&mut self, reason: &'static str) {
        if self.binding_failure_reason != Some(reason) {
            self.interrupt_activity();
        }
        self.binding_failure_reason = Some(reason);
    }

    pub(crate) fn set_stable_binding(
        &mut self,
        fingerprint: capacity_domain::AccountFingerprint,
    ) -> bool {
        self.binding_failure_reason = None;
        let next = AccountBinding::Stable(fingerprint);
        if self.binding == next {
            return false;
        }
        self.interrupt_activity();
        self.binding = next;
        self.pace_trials_failed = false;
        self.history_context_id = Uuid::new_v4().to_string();
        self.last_account_status = AccountBindingStatus::Unavailable;
        self.environment_id = None;
        true
    }

    pub(crate) fn has_stable_binding(&self) -> bool {
        matches!(self.binding, AccountBinding::Stable(_))
    }

    pub(crate) fn history_context_id(&self) -> &str {
        &self.history_context_id
    }

    pub(crate) fn record_snapshot(&mut self, snapshot: &StatusSnapshot) -> PersistenceRecordResult {
        let account_status = snapshot
            .account
            .as_ref()
            .map_or(AccountBindingStatus::Unavailable, |account| {
                account.binding_status
            });
        if account_status == AccountBindingStatus::Ephemeral
            && !matches!(self.binding, AccountBinding::Ephemeral(_))
        {
            self.binding = new_ephemeral_binding();
            self.history_context_id = Uuid::new_v4().to_string();
        }
        if account_status != AccountBindingStatus::Stable
            && self.last_account_status == AccountBindingStatus::Stable
        {
            self.interrupt_activity();
            self.history_context_id = Uuid::new_v4().to_string();
        }
        self.last_account_status = account_status;
        if self.environment_id.as_ref() != Some(&snapshot.environment.environment_id) {
            self.pace_trials_failed = false;
            self.interrupt_activity();
            self.history_context_id = Uuid::new_v4().to_string();
        }
        self.environment_id = Some(snapshot.environment.environment_id.clone());

        let effective_binding = match account_status {
            AccountBindingStatus::Unavailable => AccountBinding::Unavailable,
            AccountBindingStatus::Stable | AccountBindingStatus::Ephemeral => self.binding.clone(),
        };
        let Some(store) = self.store.as_mut() else {
            return PersistenceRecordResult::StoreUnavailable;
        };
        let Some(executable) = snapshot.codex_executable.as_ref() else {
            return match effective_binding {
                AccountBinding::Stable(_) => PersistenceRecordResult::Failed,
                AccountBinding::Ephemeral(_) => {
                    PersistenceRecordResult::Skipped(SnapshotPersistenceSkip::EphemeralBinding)
                }
                AccountBinding::Unavailable => {
                    PersistenceRecordResult::Skipped(SnapshotPersistenceSkip::UnavailableBinding)
                }
            };
        };
        let observation = CompatibilityObservationInput {
            environment_id: snapshot.environment.environment_id.clone(),
            executable_id: executable.executable_id.clone(),
            codex_version: executable.version.clone(),
            protocol_schema_fingerprint: None,
            compatibility: snapshot.data_status.compatibility,
            observed_at: snapshot.captured_at.clone(),
        };

        match store.record_snapshot(&effective_binding, snapshot, &observation) {
            Ok(SnapshotPersistenceOutcome::Persisted { .. }) => {
                // Daily monitoring records facts only. Unvalidated prospective
                // experiments are suspended; retain their existing ledger but
                // do not calculate, freeze or settle trials on each quota read.
                PersistenceRecordResult::Persisted
            }
            Ok(SnapshotPersistenceOutcome::Skipped(reason)) => {
                PersistenceRecordResult::Skipped(reason)
            }
            Err(_) => PersistenceRecordResult::Failed,
        }
    }

    pub(crate) fn history(
        &self,
        limit_id: &str,
        maximum_points: Option<u32>,
    ) -> DesktopHistoryEnvelope {
        let mut result = self.history_inner(limit_id, maximum_points);
        result.history_context_id = Some(self.history_context_id.clone());
        result
    }

    fn history_inner(&self, limit_id: &str, maximum_points: Option<u32>) -> DesktopHistoryEnvelope {
        if !valid_limit_id(limit_id)
            || maximum_points.is_some_and(|value| value == 0 || value > MAX_HISTORY_POINTS)
        {
            return history_envelope(
                DesktopHistoryStatus::InvalidRequest,
                "history_request_invalid",
            );
        }
        let Some(store) = self.store.as_ref() else {
            return history_envelope(DesktopHistoryStatus::Failed, "history_store_unavailable");
        };
        if let Some(reason) = self.binding_failure_reason {
            return history_envelope(DesktopHistoryStatus::BindingRequired, reason);
        }
        match self.last_account_status {
            AccountBindingStatus::Ephemeral => {
                return history_envelope(
                    DesktopHistoryStatus::BindingRequired,
                    "history_ephemeral_binding",
                );
            }
            AccountBindingStatus::Unavailable => {
                return history_envelope(
                    DesktopHistoryStatus::Unavailable,
                    "history_account_unavailable",
                );
            }
            AccountBindingStatus::Stable => {}
        }
        let AccountBinding::Stable(fingerprint) = &self.binding else {
            return history_envelope(
                DesktopHistoryStatus::BindingRequired,
                "history_stable_binding_missing",
            );
        };
        let Some(environment_id) = self.environment_id.as_deref() else {
            return history_envelope(
                DesktopHistoryStatus::Unavailable,
                "history_context_unavailable",
            );
        };

        let result = match maximum_points {
            Some(maximum) => store
                .history(environment_id, fingerprint, limit_id, maximum)
                .map(|points| (points, None)),
            None => store
                .local_codex_history_overview(environment_id, fingerprint, limit_id)
                .map(
                    |HistoryOverview {
                         points,
                         sample_count,
                         daily_activity,
                         quota_changes,
                     }| {
                        (
                            points,
                            Some(DesktopHistoryOverview {
                                sample_count,
                                quota_changes,
                                daily_activity: daily_activity
                                    .into_iter()
                                    .map(|day| DesktopHistoryDayActivity {
                                        date: day.date,
                                        sample_count: day.sample_count,
                                        comparable_intervals: day.comparable_intervals,
                                        consumed_percent: day.consumed_percent,
                                    })
                                    .collect(),
                            }),
                        )
                    },
                ),
        };
        match result {
            Ok((points, overview)) => DesktopHistoryEnvelope {
                schema_version: HISTORY_SCHEMA_VERSION,
                history_context_id: None,
                status: DesktopHistoryStatus::Available,
                reason_code: "history_available",
                points: points.into_iter().map(DesktopHistoryPoint::from).collect(),
                overview,
            },
            Err(_) => history_envelope(DesktopHistoryStatus::Failed, "history_query_failed"),
        }
    }

    pub(crate) fn settings(&self) -> DesktopSettingsEnvelope {
        let Some(store) = self.store.as_ref() else {
            return settings_envelope(
                DesktopSettingsStatus::Failed,
                "settings_store_unavailable",
                None,
            );
        };
        match store.settings() {
            Ok(settings) => settings_envelope(
                DesktopSettingsStatus::Available,
                "settings_available",
                Some(DesktopSettingsView::from(settings)),
            ),
            Err(_) => {
                settings_envelope(DesktopSettingsStatus::Failed, "settings_query_failed", None)
            }
        }
    }

    pub(crate) fn update_settings(
        &mut self,
        request: DesktopSettingsUpdateRequest,
        updated_at: &capacity_domain::UtcTimestamp,
    ) -> DesktopSettingsEnvelope {
        let Some(language) = settings_language_from_ipc(&request.language) else {
            return settings_envelope(
                DesktopSettingsStatus::InvalidRequest,
                "settings_request_invalid",
                None,
            );
        };
        let update = MonitorSettingsUpdate {
            expected_revision: request.expected_revision,
            auto_refresh_enabled: request.auto_refresh_enabled,
            refresh_interval_seconds: request.refresh_interval_seconds,
            notification_threshold_basis_points: request.notification_threshold_basis_points,
            reset_credit_notice_hours: request.reset_credit_notice_hours,
            quiet_hours_enabled: request.quiet_hours_enabled,
            quiet_hours_start_minute: request.quiet_hours_start_minute,
            quiet_hours_end_minute: request.quiet_hours_end_minute,
            language,
            lock_screen_privacy: request.lock_screen_privacy,
            launch_at_login: request.launch_at_login,
            history_retention_days: request.history_retention_days,
        };
        let Some(store) = self.store.as_mut() else {
            return settings_envelope(
                DesktopSettingsStatus::Failed,
                "settings_store_unavailable",
                None,
            );
        };
        match store.update_settings(&update, updated_at) {
            Ok(SettingsUpdateOutcome::Updated(settings)) => settings_envelope(
                DesktopSettingsStatus::Updated,
                "settings_updated",
                Some(DesktopSettingsView::from(settings)),
            ),
            Ok(SettingsUpdateOutcome::RevisionConflict(settings)) => settings_envelope(
                DesktopSettingsStatus::RevisionConflict,
                "settings_revision_conflict",
                Some(DesktopSettingsView::from(settings)),
            ),
            Err(StoreError::InvalidSettings(_)) => settings_envelope(
                DesktopSettingsStatus::InvalidRequest,
                "settings_request_invalid",
                None,
            ),
            Err(_) => settings_envelope(
                DesktopSettingsStatus::Failed,
                "settings_update_failed",
                None,
            ),
        }
    }

    pub(crate) fn work_schedule(&self) -> DesktopWorkScheduleEnvelope {
        let Some(store) = self.store.as_ref() else {
            return work_schedule_envelope(
                DesktopWorkScheduleStatus::Failed,
                "work_schedule_store_unavailable",
                None,
            );
        };
        match store.work_schedule() {
            Ok(schedule) => work_schedule_envelope(
                DesktopWorkScheduleStatus::Available,
                "work_schedule_available",
                Some(DesktopWorkScheduleView::from(schedule)),
            ),
            Err(_) => work_schedule_envelope(
                DesktopWorkScheduleStatus::Failed,
                "work_schedule_query_failed",
                None,
            ),
        }
    }

    pub(crate) fn update_work_schedule(
        &mut self,
        request: DesktopWorkScheduleUpdateRequest,
        updated_at: &capacity_domain::UtcTimestamp,
    ) -> DesktopWorkScheduleEnvelope {
        let periods = request
            .off_periods
            .into_iter()
            .map(|period| {
                WorkSchedulePeriod::new(period.start_minute_of_day, period.end_minute_of_day)
            })
            .collect::<Result<Vec<_>, _>>();
        let Ok(off_periods) = periods else {
            return work_schedule_envelope(
                DesktopWorkScheduleStatus::InvalidRequest,
                "work_schedule_request_invalid",
                None,
            );
        };
        let update = WorkScheduleSettingsUpdate {
            expected_revision: request.expected_revision,
            enabled: request.enabled,
            off_periods,
        };
        let Some(store) = self.store.as_mut() else {
            return work_schedule_envelope(
                DesktopWorkScheduleStatus::Failed,
                "work_schedule_store_unavailable",
                None,
            );
        };
        match store.update_work_schedule(&update, updated_at) {
            Ok(WorkScheduleSettingsUpdateOutcome::Updated(schedule)) => work_schedule_envelope(
                DesktopWorkScheduleStatus::Updated,
                "work_schedule_updated",
                Some(DesktopWorkScheduleView::from(schedule)),
            ),
            Ok(WorkScheduleSettingsUpdateOutcome::RevisionConflict(schedule)) => {
                work_schedule_envelope(
                    DesktopWorkScheduleStatus::RevisionConflict,
                    "work_schedule_revision_conflict",
                    Some(DesktopWorkScheduleView::from(schedule)),
                )
            }
            Err(StoreError::InvalidSettings(_) | StoreError::PlannerValidation(_)) => {
                work_schedule_envelope(
                    DesktopWorkScheduleStatus::InvalidRequest,
                    "work_schedule_request_invalid",
                    None,
                )
            }
            Err(_) => work_schedule_envelope(
                DesktopWorkScheduleStatus::Failed,
                "work_schedule_update_failed",
                None,
            ),
        }
    }

    pub(crate) fn delete_all_local_data(
        &mut self,
        request: DesktopDeleteAllRequest,
    ) -> DesktopDeleteAllEnvelope {
        if !request.confirmed {
            return delete_all_envelope(
                DesktopDeleteAllStatus::ConfirmationRequired,
                "delete_all_confirmation_required",
                None,
                None,
            );
        }
        if request.expected_settings_revision == 0 {
            return delete_all_envelope(
                DesktopDeleteAllStatus::InvalidRequest,
                "delete_all_request_invalid",
                None,
                None,
            );
        }
        let Some(store) = self.store.as_mut() else {
            return delete_all_envelope(
                DesktopDeleteAllStatus::Failed,
                "delete_all_store_unavailable",
                None,
                None,
            );
        };
        match store.delete_all_database_data_if_revision(request.expected_settings_revision) {
            Ok(DeleteDatabaseOutcome::Deleted(report)) => {
                self.pace_trials_failed = false;
                self.binding = new_ephemeral_binding();
                self.history_context_id = Uuid::new_v4().to_string();
                self.last_account_status = AccountBindingStatus::Unavailable;
                self.environment_id = None;
                delete_all_envelope(
                    DesktopDeleteAllStatus::Deleted,
                    "delete_all_completed",
                    Some(report.total_rows()),
                    Some(DEFAULT_SETTINGS_REVISION),
                )
            }
            Ok(DeleteDatabaseOutcome::RevisionConflict(settings)) => delete_all_envelope(
                DesktopDeleteAllStatus::RevisionConflict,
                "delete_all_revision_conflict",
                None,
                Some(settings.revision),
            ),
            Err(_) => delete_all_envelope(
                DesktopDeleteAllStatus::Failed,
                "delete_all_failed",
                None,
                None,
            ),
        }
    }

    #[cfg(test)]
    fn in_memory(binding: AccountBinding) -> Self {
        Self {
            store: Some(CapacityStore::open_in_memory().expect("open in-memory store")),
            binding,
            last_account_status: AccountBindingStatus::Unavailable,
            environment_id: None,
            binding_failure_reason: None,
            history_context_id: Uuid::new_v4().to_string(),
            pace_trials_failed: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory_ephemeral() -> Self {
        Self::in_memory(new_ephemeral_binding())
    }
}

impl From<HistoryPoint> for DesktopHistoryPoint {
    fn from(point: HistoryPoint) -> Self {
        Self {
            snapshot_id: point.snapshot_id,
            captured_at: point.captured_at.as_str().to_owned(),
            limit_id: point.limit_id,
            label: point.label,
            window_minutes: point.window_minutes,
            used_percent: point.used_percent,
            remaining_percent: point.remaining_percent,
            resets_at: point
                .resets_at
                .as_ref()
                .map(|timestamp| timestamp.as_str().to_owned()),
            availability: point.availability,
            compatibility: point.compatibility,
        }
    }
}

impl From<MonitorSettings> for DesktopSettingsView {
    fn from(settings: MonitorSettings) -> Self {
        Self {
            revision: settings.revision,
            auto_refresh_enabled: settings.auto_refresh_enabled,
            refresh_interval_seconds: settings.refresh_interval_seconds,
            notification_threshold_basis_points: settings.notification_threshold_basis_points,
            reset_credit_notice_hours: settings.reset_credit_notice_hours,
            quiet_hours_enabled: settings.quiet_hours_enabled,
            quiet_hours_start_minute: settings.quiet_hours_start_minute,
            quiet_hours_end_minute: settings.quiet_hours_end_minute,
            language: settings_language_to_ipc(settings.language),
            lock_screen_privacy: settings.lock_screen_privacy,
            launch_at_login: settings.launch_at_login,
            history_retention_days: settings.history_retention_days,
            updated_at: settings.updated_at.as_str().to_owned(),
        }
    }
}

impl From<WorkScheduleSettings> for DesktopWorkScheduleView {
    fn from(settings: WorkScheduleSettings) -> Self {
        Self {
            revision: settings.revision,
            enabled: settings.schedule.enabled,
            off_periods: settings
                .schedule
                .off_periods
                .into_iter()
                .map(|period| DesktopWorkSchedulePeriodView {
                    start_minute_of_day: period.start_minute_of_day,
                    end_minute_of_day: period.end_minute_of_day,
                })
                .collect(),
            updated_at: settings.updated_at.as_str().to_owned(),
        }
    }
}

fn history_envelope(
    status: DesktopHistoryStatus,
    reason_code: &'static str,
) -> DesktopHistoryEnvelope {
    DesktopHistoryEnvelope {
        schema_version: HISTORY_SCHEMA_VERSION,
        history_context_id: None,
        status,
        reason_code,
        points: Vec::new(),
        overview: None,
    }
}

fn settings_envelope(
    status: DesktopSettingsStatus,
    reason_code: &'static str,
    settings: Option<DesktopSettingsView>,
) -> DesktopSettingsEnvelope {
    DesktopSettingsEnvelope {
        schema_version: SETTINGS_SCHEMA_VERSION,
        status,
        reason_code,
        settings,
    }
}

fn work_schedule_envelope(
    status: DesktopWorkScheduleStatus,
    reason_code: &'static str,
    schedule: Option<DesktopWorkScheduleView>,
) -> DesktopWorkScheduleEnvelope {
    DesktopWorkScheduleEnvelope {
        schema_version: WORK_SCHEDULE_SCHEMA_VERSION,
        status,
        reason_code,
        schedule,
    }
}

fn delete_all_envelope(
    status: DesktopDeleteAllStatus,
    reason_code: &'static str,
    deleted_record_count: Option<u64>,
    settings_revision: Option<u32>,
) -> DesktopDeleteAllEnvelope {
    DesktopDeleteAllEnvelope {
        schema_version: DELETE_ALL_SCHEMA_VERSION,
        status,
        reason_code,
        deleted_record_count,
        settings_revision,
    }
}

fn settings_language_from_ipc(value: &str) -> Option<SettingsLanguage> {
    match value {
        "system" => Some(SettingsLanguage::System),
        "en" => Some(SettingsLanguage::English),
        "zh-CN" => Some(SettingsLanguage::SimplifiedChinese),
        _ => None,
    }
}

fn settings_language_to_ipc(value: SettingsLanguage) -> &'static str {
    match value {
        SettingsLanguage::System => "system",
        SettingsLanguage::English => "en",
        SettingsLanguage::SimplifiedChinese => "zh-CN",
    }
}

fn valid_limit_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn new_ephemeral_binding() -> AccountBinding {
    let session_id = format!("session:v1:{}", Uuid::new_v4());
    AccountBinding::Ephemeral(
        EphemeralSessionId::parse(session_id).expect("UUID must satisfy ephemeral ID format"),
    )
}

#[cfg(unix)]
fn prepare_app_data_directory(path: &Path) -> Result<(), ()> {
    use std::fs::DirBuilder;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    if !path.exists() {
        let mut builder = DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(drop)?;
    }
    let metadata = std::fs::symlink_metadata(path).map_err(drop)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(());
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_app_data_directory(path: &Path) -> Result<(), ()> {
    std::fs::create_dir_all(path).map_err(drop)?;
    if !std::fs::metadata(path).map_err(drop)?.is_dir() {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use capacity_domain::{ACCOUNT_FINGERPRINT_PREFIX, AccountFingerprint};

    use super::*;

    pub(super) fn fixture_status(name: &str) -> StatusSnapshot {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../fixtures/app-server")
                .join(name)
                .join("expected-status.json"),
        )
        .expect("read status fixture");
        serde_json::from_slice(&bytes).expect("parse status fixture")
    }

    fn stable_binding() -> AccountBinding {
        let value = format!(
            "{ACCOUNT_FINGERPRINT_PREFIX}018f47a2-8a71-7f4a-9c35-1f4234a73311:{}",
            "a3".repeat(32)
        );
        AccountBinding::Stable(AccountFingerprint::parse(value).expect("stable fingerprint"))
    }

    #[test]
    fn account_history_delivers_raw_quota_rise_analysis_without_exposing_the_binding() {
        let mut persistence = DesktopPersistence::in_memory(stable_binding());
        for (time, remaining, reset) in [
            ("2026-09-12T10:00:00Z", 18.4, "2026-09-13T12:00:00Z"),
            ("2026-09-12T10:05:00Z", 99.2, "2026-09-19T10:05:00Z"),
            ("2026-09-12T10:06:00Z", 98.7, "2026-09-19T10:05:00Z"),
        ] {
            let mut status = fixture_status("plus-normal");
            status.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
            status.captured_at = capacity_domain::UtcTimestamp::parse(time).unwrap();
            let weekly = status
                .quota
                .windows
                .iter_mut()
                .find(|window| window.window_minutes == Some(10080))
                .unwrap();
            weekly.remaining_percent = remaining;
            weekly.used_percent = 100.0 - remaining;
            weekly.resets_at = Some(capacity_domain::UtcTimestamp::parse(reset).unwrap());
            assert_eq!(
                persistence.record_snapshot(&status),
                PersistenceRecordResult::Persisted
            );
        }
        let history = persistence.history("codex:secondary", None);
        assert_eq!(history.status, DesktopHistoryStatus::Available);
        let changes = &history.overview.as_ref().unwrap().quota_changes;
        assert_eq!(changes.valid_samples, 3);
        assert_eq!(
            changes.observations[0].classification,
            capacity_domain::quota_change::QuotaRiseClassification::BeforeScheduledBoundary
        );
        let json = serde_json::to_value(history).unwrap();
        assert_eq!(
            json["overview"]["quotaChanges"]["observations"][0]["beforeRemainingPercent"],
            18.4
        );
        assert!(!json.to_string().contains(ACCOUNT_FINGERPRINT_PREFIX));
        persistence.set_binding_failure("history_keychain_interaction_required");
        let unavailable = persistence.history("codex:secondary", None);
        assert!(unavailable.points.is_empty());
        assert!(unavailable.overview.is_none());
    }

    fn settings_request(expected_revision: u32) -> DesktopSettingsUpdateRequest {
        DesktopSettingsUpdateRequest {
            expected_revision,
            auto_refresh_enabled: true,
            refresh_interval_seconds: 900,
            notification_threshold_basis_points: Some(1_000),
            reset_credit_notice_hours: 48,
            quiet_hours_enabled: true,
            quiet_hours_start_minute: Some(22 * 60),
            quiet_hours_end_minute: Some(7 * 60),
            language: "zh-CN".to_owned(),
            lock_screen_privacy: true,
            launch_at_login: false,
            history_retention_days: 365,
        }
    }

    fn work_schedule_request(expected_revision: u32) -> DesktopWorkScheduleUpdateRequest {
        DesktopWorkScheduleUpdateRequest {
            expected_revision,
            enabled: true,
            off_periods: vec![DesktopWorkSchedulePeriodInput {
                start_minute_of_day: 120,
                end_minute_of_day: 600,
            }],
        }
    }

    #[test]
    fn settings_defaults_match_fixture_and_updates_use_revision_guard() {
        let mut persistence = DesktopPersistence::in_memory_ephemeral();
        let defaults = persistence.settings();
        assert_eq!(defaults.status, DesktopSettingsStatus::Available);
        capacity_domain::UtcTimestamp::parse(
            defaults
                .settings
                .as_ref()
                .expect("default settings")
                .updated_at
                .clone(),
        )
        .expect("default update timestamp");
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../fixtures/desktop/v1/settings-available-envelope.json"),
            )
            .expect("read settings fixture"),
        )
        .expect("parse settings fixture");
        let mut serialized = serde_json::to_value(&defaults).expect("serialize settings");
        serialized["settings"]["updatedAt"] = expected["settings"]["updatedAt"].clone();
        assert_eq!(serialized, expected);

        let updated = persistence.update_settings(
            settings_request(1),
            &capacity_domain::UtcTimestamp::parse("2026-08-30T04:15:00Z")
                .expect("update timestamp"),
        );
        assert_eq!(updated.status, DesktopSettingsStatus::Updated);
        let updated_settings = updated.settings.expect("updated settings");
        assert_eq!(updated_settings.revision, 2);
        assert_eq!(updated_settings.language, "zh-CN");
        assert_eq!(updated_settings.refresh_interval_seconds, 900);

        let conflict = persistence.update_settings(
            settings_request(1),
            &capacity_domain::UtcTimestamp::parse("2026-08-30T04:16:00Z")
                .expect("conflict timestamp"),
        );
        assert_eq!(conflict.status, DesktopSettingsStatus::RevisionConflict);
        assert_eq!(conflict.settings.expect("current settings").revision, 2);
    }

    #[test]
    fn settings_request_rejects_unknown_language_values_and_fields() {
        let mut persistence = DesktopPersistence::in_memory_ephemeral();
        let mut invalid = settings_request(1);
        invalid.language = "../../locale".to_owned();
        let response = persistence.update_settings(
            invalid,
            &capacity_domain::UtcTimestamp::parse("2026-08-30T04:15:00Z")
                .expect("update timestamp"),
        );
        assert_eq!(response.status, DesktopSettingsStatus::InvalidRequest);
        assert!(response.settings.is_none());

        let request = serde_json::json!({
            "expectedRevision": 1,
            "autoRefreshEnabled": true,
            "refreshIntervalSeconds": 300,
            "notificationThresholdBasisPoints": 2000,
            "resetCreditNoticeHours": 24,
            "quietHoursEnabled": false,
            "quietHoursStartMinute": null,
            "quietHoursEndMinute": null,
            "language": "system",
            "lockScreenPrivacy": true,
            "launchAtLogin": false,
            "historyRetentionDays": 180,
            "path": "/tmp/not-allowed"
        });
        assert!(serde_json::from_value::<DesktopSettingsUpdateRequest>(request).is_err());
    }

    #[test]
    fn work_schedule_round_trips_and_rejects_stale_or_invalid_updates() {
        let mut persistence = DesktopPersistence::in_memory_ephemeral();
        let defaults = persistence.work_schedule();
        assert_eq!(defaults.status, DesktopWorkScheduleStatus::Available);
        let defaults = defaults.schedule.expect("default schedule");
        assert_eq!(defaults.revision, 1);
        assert!(!defaults.enabled);
        assert!(defaults.off_periods.is_empty());

        let updated = persistence.update_work_schedule(
            work_schedule_request(1),
            &capacity_domain::UtcTimestamp::parse("2026-09-04T05:00:00Z")
                .expect("update timestamp"),
        );
        assert_eq!(updated.status, DesktopWorkScheduleStatus::Updated);
        let schedule = updated.schedule.expect("updated schedule");
        assert_eq!(schedule.revision, 2);
        assert!(schedule.enabled);
        assert_eq!(schedule.off_periods.len(), 1);

        let conflict = persistence.update_work_schedule(
            work_schedule_request(1),
            &capacity_domain::UtcTimestamp::parse("2026-09-04T05:01:00Z")
                .expect("conflict timestamp"),
        );
        assert_eq!(conflict.status, DesktopWorkScheduleStatus::RevisionConflict);
        assert_eq!(conflict.schedule.expect("current schedule").revision, 2);

        let mut invalid = work_schedule_request(2);
        invalid.off_periods[0].start_minute_of_day = 1_440;
        let invalid = persistence.update_work_schedule(
            invalid,
            &capacity_domain::UtcTimestamp::parse("2026-09-04T05:02:00Z")
                .expect("invalid timestamp"),
        );
        assert_eq!(invalid.status, DesktopWorkScheduleStatus::InvalidRequest);
        assert!(invalid.schedule.is_none());
    }

    #[test]
    fn delete_all_requires_confirmation_and_current_settings_revision() {
        let mut persistence = DesktopPersistence::in_memory(stable_binding());
        let mut status = fixture_status("plus-normal");
        status
            .account
            .as_mut()
            .expect("fixture account")
            .binding_status = AccountBindingStatus::Stable;
        assert_eq!(
            persistence.record_snapshot(&status),
            PersistenceRecordResult::Persisted
        );

        let confirmation = persistence.delete_all_local_data(DesktopDeleteAllRequest {
            expected_settings_revision: 1,
            confirmed: false,
        });
        assert_eq!(
            confirmation.status,
            DesktopDeleteAllStatus::ConfirmationRequired
        );
        assert_eq!(
            serde_json::to_value(&confirmation).expect("serialize confirmation"),
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../../fixtures/desktop/v1/delete-all-confirmation-envelope.json")
                )
                .expect("read confirmation fixture")
            )
            .expect("parse confirmation fixture")
        );
        assert_eq!(
            persistence.history("codex:primary", None).status,
            DesktopHistoryStatus::Available
        );

        let conflict = persistence.delete_all_local_data(DesktopDeleteAllRequest {
            expected_settings_revision: 2,
            confirmed: true,
        });
        assert_eq!(conflict.status, DesktopDeleteAllStatus::RevisionConflict);
        assert_eq!(conflict.settings_revision, Some(1));
        assert_eq!(
            persistence.history("codex:primary", None).status,
            DesktopHistoryStatus::Available
        );

        let deleted = persistence.delete_all_local_data(DesktopDeleteAllRequest {
            expected_settings_revision: 1,
            confirmed: true,
        });
        assert_eq!(deleted.status, DesktopDeleteAllStatus::Deleted);
        assert_eq!(deleted.reason_code, "delete_all_completed");
        // Includes the capture-time plan context introduced by schema 8.
        assert_eq!(deleted.deleted_record_count, Some(11));
        assert_eq!(deleted.settings_revision, Some(1));
        assert_eq!(
            serde_json::to_value(&deleted).expect("serialize deletion"),
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../../fixtures/desktop/v1/delete-all-completed-envelope.json")
                )
                .expect("read deletion fixture")
            )
            .expect("parse deletion fixture")
        );
        assert_eq!(
            persistence.history("codex:primary", None).status,
            DesktopHistoryStatus::Unavailable
        );
        assert_eq!(
            persistence
                .settings()
                .settings
                .expect("reset settings")
                .revision,
            1
        );
    }

    #[test]
    fn settings_and_delete_fail_closed_when_store_is_unavailable() {
        let mut persistence = DesktopPersistence::unavailable();
        assert_eq!(persistence.settings().status, DesktopSettingsStatus::Failed);
        assert_eq!(
            persistence
                .update_settings(
                    settings_request(1),
                    &capacity_domain::UtcTimestamp::parse("2026-08-30T04:15:00Z")
                        .expect("update timestamp")
                )
                .status,
            DesktopSettingsStatus::Failed
        );
        assert_eq!(
            persistence
                .delete_all_local_data(DesktopDeleteAllRequest {
                    expected_settings_revision: 1,
                    confirmed: true,
                })
                .status,
            DesktopDeleteAllStatus::Failed
        );
    }

    #[test]
    fn ephemeral_refresh_is_explicitly_skipped_and_has_no_history() {
        let mut persistence = DesktopPersistence::in_memory(new_ephemeral_binding());
        let status = fixture_status("plus-normal");

        assert_eq!(
            persistence.record_snapshot(&status),
            PersistenceRecordResult::Skipped(SnapshotPersistenceSkip::EphemeralBinding)
        );
        let history = persistence.history("codex:primary", None);
        assert_eq!(history.status, DesktopHistoryStatus::BindingRequired);
        assert_eq!(history.reason_code, "history_ephemeral_binding");
        assert!(history.points.is_empty());
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../fixtures/desktop/v1/history-binding-required-envelope.json"),
            )
            .expect("read history fixture"),
        )
        .expect("parse history fixture");
        let mut actual = serde_json::to_value(&history).expect("serialize history");
        assert_eq!(actual["historyContextId"], persistence.history_context_id());
        actual.as_object_mut().unwrap().remove("historyContextId");
        assert_eq!(actual, expected);
    }

    #[test]
    fn stable_history_ipc_is_an_identifier_allowlist() {
        let mut persistence = DesktopPersistence::in_memory(stable_binding());
        let mut status = fixture_status("plus-normal");
        status
            .account
            .as_mut()
            .expect("fixture account")
            .binding_status = AccountBindingStatus::Stable;

        assert_eq!(
            persistence.record_snapshot(&status),
            PersistenceRecordResult::Persisted
        );
        let history = persistence.history("codex:primary", Some(20));
        assert_eq!(history.status, DesktopHistoryStatus::Available);
        assert_eq!(history.points.len(), 1);
        let serialized = serde_json::to_string(&history).expect("serialize history");
        assert!(!serialized.contains("hmac-sha256"));
        assert!(!serialized.contains("environment_id"));
        assert!(!serialized.contains("canonical_path"));
        assert!(!serialized.contains("file_identity"));
        assert!(!serialized.contains("@"));
    }

    #[test]
    fn invalid_history_request_never_reaches_sql() {
        let persistence = DesktopPersistence::in_memory(stable_binding());
        for (limit_id, maximum_points) in [
            ("", None),
            ("codex:\nsecret", None),
            ("codex:primary", Some(0)),
            ("codex:primary", Some(MAX_HISTORY_POINTS + 1)),
        ] {
            let history = persistence.history(limit_id, maximum_points);
            assert_eq!(history.status, DesktopHistoryStatus::InvalidRequest);
            assert!(history.points.is_empty());
        }
    }

    #[test]
    fn default_history_returns_time_overview_with_allowlisted_daily_activity() {
        let mut persistence = DesktopPersistence::in_memory(stable_binding());
        let mut status = fixture_status("plus-normal");
        status.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
        persistence.record_snapshot(&status);
        let history = persistence.history("codex:primary", None);
        assert_eq!(history.status, DesktopHistoryStatus::Available);
        let overview = history.overview.as_ref().unwrap();
        assert_eq!(overview.sample_count, 1);
        assert_eq!(overview.daily_activity.len(), 1);
        assert_eq!(overview.daily_activity[0].comparable_intervals, 0);
        assert!(
            persistence
                .history("codex:primary", Some(20))
                .overview
                .is_none()
        );
        let serialized = serde_json::to_string(&history).unwrap();
        assert!(serialized.contains("comparableIntervals"));
        for forbidden in [
            "hmac-sha256",
            "environment_id",
            "canonical_path",
            "file_identity",
            "@",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn unavailable_store_does_not_block_live_status_but_reports_history_failure() {
        let mut persistence = DesktopPersistence::unavailable();
        assert_eq!(
            persistence.record_snapshot(&fixture_status("plus-normal")),
            PersistenceRecordResult::StoreUnavailable
        );
        let history = persistence.history("codex:primary", None);
        assert_eq!(history.status, DesktopHistoryStatus::Failed);
        assert_eq!(history.reason_code, "history_store_unavailable");
        assert!(history.points.is_empty());
    }

    #[test]
    fn account_boundary_rotation_hides_previously_bound_history() {
        let mut persistence = DesktopPersistence::in_memory(stable_binding());
        let mut stable = fixture_status("plus-normal");
        stable
            .account
            .as_mut()
            .expect("fixture account")
            .binding_status = AccountBindingStatus::Stable;
        assert_eq!(
            persistence.record_snapshot(&stable),
            PersistenceRecordResult::Persisted
        );
        assert_eq!(
            persistence.history("codex:primary", None).status,
            DesktopHistoryStatus::Available
        );

        persistence.rotate_ephemeral_binding();
        assert_eq!(
            persistence.history("codex:primary", None).status,
            DesktopHistoryStatus::Unavailable
        );
        assert!(persistence.history("codex:primary", None).points.is_empty());
    }

    #[test]
    fn changing_stable_binding_rotates_the_visible_history_boundary() {
        let first = stable_binding();
        let AccountBinding::Stable(first_fingerprint) = first else {
            unreachable!();
        };
        let mut persistence =
            DesktopPersistence::in_memory(AccountBinding::Stable(first_fingerprint.clone()));
        let mut status = fixture_status("plus-normal");
        status
            .account
            .as_mut()
            .expect("fixture account")
            .binding_status = AccountBindingStatus::Stable;
        assert_eq!(
            persistence.record_snapshot(&status),
            PersistenceRecordResult::Persisted
        );
        assert_eq!(
            persistence.history("codex:primary", None).status,
            DesktopHistoryStatus::Available
        );
        assert!(!persistence.set_stable_binding(first_fingerprint));
        let initial_context = persistence.history_context_id().to_owned();
        persistence.record_snapshot(&status);
        assert_eq!(initial_context, persistence.history_context_id());

        let second = AccountFingerprint::parse(format!(
            "{ACCOUNT_FINGERPRINT_PREFIX}018f47a2-8a71-7f4a-9c35-1f4234a73312:{}",
            "b4".repeat(32)
        ))
        .expect("second stable fingerprint");
        assert!(persistence.set_stable_binding(second));
        assert_ne!(initial_context, persistence.history_context_id());
        assert!(persistence.has_stable_binding());
        let history = persistence.history("codex:primary", None);
        assert_eq!(history.status, DesktopHistoryStatus::Unavailable);
        assert!(history.points.is_empty());
    }

    #[test]
    fn history_context_tracks_identity_and_environment_not_freshness() {
        let mut persistence = DesktopPersistence::in_memory(stable_binding());
        let mut status = fixture_status("plus-normal");
        status.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
        persistence.record_snapshot(&status);
        let context = persistence.history_context_id().to_owned();
        status.data_status.freshness = capacity_domain::Freshness::Stale;
        persistence.record_snapshot(&status);
        assert_eq!(context, persistence.history_context_id());
        assert_eq!(
            persistence
                .history("codex:primary", None)
                .history_context_id
                .as_deref(),
            Some(context.as_str())
        );
        status.environment.environment_id = "another-local-environment".to_owned();
        persistence.record_snapshot(&status);
        let next = persistence.history_context_id().to_owned();
        assert_ne!(context, next);
        persistence.rotate_ephemeral_binding();
        assert_ne!(next, persistence.history_context_id());
        assert!(persistence.history("codex:primary", None).points.is_empty());
    }

    #[test]
    fn two_accounts_keep_separate_history_after_database_and_key_store_reopen() {
        use crate::account_binding::{DesktopAccountBindingStore, DesktopAccountIdentity};
        let root = tempfile::tempdir().unwrap();
        let key_path = root.path().join("test-key-store");
        let database_path = root.path().join("history");
        let a = DesktopAccountIdentity::from_account_digest("0123456789abcdef01234567").unwrap();
        let b = DesktopAccountIdentity::from_account_digest("89abcdef0123456789abcdef").unwrap();
        let snapshot = |remaining: f64, time: &str| {
            let mut value = fixture_status("plus-normal");
            value.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
            value.captured_at = capacity_domain::UtcTimestamp::parse(time).unwrap();
            let weekly = value
                .quota
                .windows
                .iter_mut()
                .find(|window| window.limit_id == "codex:secondary")
                .unwrap();
            weekly.remaining_percent = remaining;
            weekly.used_percent = 100.0 - remaining;
            value
        };
        {
            let mut keys = DesktopAccountBindingStore::explicit_file_fallback(&key_path).unwrap();
            let mut persistence = DesktopPersistence::open(&database_path);
            assert!(persistence.backend_available());
            persistence.set_stable_binding(keys.fingerprint(&a).unwrap());
            assert_eq!(
                persistence.record_snapshot(&snapshot(56.0, "2026-08-30T00:00:00Z")),
                PersistenceRecordResult::Persisted
            );
            persistence.set_stable_binding(keys.fingerprint(&b).unwrap());
            assert!(
                persistence
                    .history("codex:secondary", None)
                    .points
                    .is_empty()
            );
            assert_eq!(
                persistence.record_snapshot(&snapshot(15.0, "2026-08-30T00:01:00Z")),
                PersistenceRecordResult::Persisted
            );
            let history = persistence.history("codex:secondary", None);
            assert_eq!(history.points.len(), 1);
            assert_eq!(history.points[0].remaining_percent, 15.0);
        }
        let mut keys = DesktopAccountBindingStore::explicit_file_fallback(&key_path).unwrap();
        let mut reopened = DesktopPersistence::open(&database_path);
        assert!(reopened.history("codex:secondary", None).points.is_empty());
        for (identity, old, now) in [(&a, 56.0, 55.0), (&b, 15.0, 14.0)] {
            reopened.set_stable_binding(keys.fingerprint(identity).unwrap());
            assert_eq!(
                reopened.record_snapshot(&snapshot(now, "2026-08-30T00:02:00Z")),
                PersistenceRecordResult::Persisted
            );
            let history = reopened.history("codex:secondary", None);
            assert_eq!(history.status, DesktopHistoryStatus::Available);
            assert_eq!(history.points.len(), 2);
            assert!(
                history
                    .points
                    .iter()
                    .all(|point| point.remaining_percent == old || point.remaining_percent == now)
            );
        }
        reopened.set_binding_failure("history_keychain_interaction_required");
        assert!(reopened.history("codex:secondary", None).points.is_empty());
        reopened.set_stable_binding(keys.fingerprint(&b).unwrap());
        assert_eq!(reopened.history("codex:secondary", None).points.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn app_store_uses_a_fixed_private_data_path() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().expect("temp parent");
        let data_directory = parent.path().join("app-data");
        let persistence = DesktopPersistence::open(&data_directory);
        assert!(persistence.backend_available());
        assert!(data_directory.join(STORE_FILENAME).is_file());
        let directory_mode = std::fs::metadata(&data_directory)
            .expect("data directory metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
        let filenames = std::fs::read_dir(&data_directory)
            .expect("read app data directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(filenames.iter().any(|name| name == STORE_FILENAME));
        assert!(filenames.iter().all(|name| {
            matches!(
                name.as_str(),
                "capacity.sqlite3" | "capacity.sqlite3-wal" | "capacity.sqlite3-shm"
            )
        }));
    }

    #[cfg(unix)]
    #[test]
    fn app_store_rejects_directory_permission_drift() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().expect("temp parent");
        let data_directory = parent.path().join("app-data");
        std::fs::create_dir(&data_directory).expect("create app data directory");
        std::fs::set_permissions(&data_directory, std::fs::Permissions::from_mode(0o755))
            .expect("broaden directory permissions");

        let persistence = DesktopPersistence::open(&data_directory);
        assert!(!persistence.backend_available());
        assert!(!data_directory.join(STORE_FILENAME).exists());
    }
}
