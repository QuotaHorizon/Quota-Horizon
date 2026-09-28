mod account_binding;
mod account_work_plans;
pub use account_work_plans::work_plans_for_saved_accounts;
mod active_time;
mod demand_plan;
mod diagnostics;
mod pace;
mod planning_archive;
pub use active_time::{
    DesktopActiveTimeEnvelope, DesktopActiveTimeQuotaEnvelope, DesktopActiveTimeUpdate,
    active_time_for_status, active_time_quota_for_status,
};
pub use demand_plan::{DesktopDemandPlanEnvelope, DesktopDemandPlanUpdate, demand_plan_for_status};
pub use pace::{
    DesktopPaceEnvelope, DesktopPaceTrialsEnvelope, pace_for_status, pace_trials_for_status,
};
pub use planning_archive::{
    DesktopPlanningArchiveEnvelope, PlanningArchiveQuery, planning_archive_for_status,
};
mod monitor;
mod mutation_status;
mod native_tray;
pub use native_tray::with_tray_on_main_thread;
mod persistence;
mod quota_observation;
mod tray;
pub use quota_observation::{DesktopQuotaObservation, DesktopQuotaObservationWindow};

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use account_binding::DesktopAccountBindingStore;
pub use account_binding::{AccountBindingError, DesktopAccountIdentity};
use capacity_domain::{
    AccountBindingStatus, Availability, Clock, Compatibility, Freshness, NotificationEngine,
    ResetCreditDetailsStatus, SchedulePaceComparison, StatusSnapshot, SummaryStatus, SystemClock,
    UtcTimestamp, WorkSchedule, WorkSchedulePeriod, evaluate_schedule_pace,
};
use chrono::{DateTime, Timelike, Utc};
use codex_runtime::app_server::RuntimeConfig;
pub use codex_runtime::discovery::bundled_codex_path;
use codex_runtime::discovery::{
    CandidateVerification, CodexExecutableCandidate, DiscoveryOutcome, DiscoveryReport,
    discover_current, is_bundled_codex_path,
};
use codex_runtime::refresh::{
    CodexStatusSource, RefreshFailure, RefreshHandle, RefreshOwnerConfig, RefreshOwnerError,
};
pub use diagnostics::DesktopDiagnosticsEnvelope;
use monitor::{
    AutoRefreshSchedule, MonitorControl, current_local_minute, notification_policy,
    publish_deliverable, run_auto_refresh, run_notification_bridge,
};
pub use mutation_status::DesktopVaultMutationStatusEnvelope;
use mutation_status::{failed_vault_mutation_status, inspect_vault_mutation_status};
use persistence::{
    DesktopDeleteAllEnvelope, DesktopDeleteAllRequest, DesktopDeleteAllStatus, DesktopPersistence,
    DesktopSettingsEnvelope, DesktopSettingsUpdateRequest, DesktopWorkScheduleEnvelope,
    DesktopWorkScheduleStatus, DesktopWorkScheduleView, PersistenceRecordResult,
};
pub use persistence::{DesktopHistoryEnvelope, DesktopWorkScheduleUpdateRequest};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

const DESKTOP_SCHEMA_VERSION: &str = "1.0";
const MAX_DESKTOP_CANDIDATES: usize = 16;
const CAPACITY_DATA_DIRECTORY: &str = "capacity-v1";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImplementationStatus {
    phase: &'static str,
    desktop_live_reads_enabled: bool,
    store_backend_available: bool,
    mutation_status_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DesktopLifecycle {
    Idle,
    Ready,
    Stale,
    SelectionRequired,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopIssue {
    code: &'static str,
    message: &'static str,
    retry_after_ms: Option<u64>,
}

impl std::fmt::Display for DesktopIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for DesktopIssue {}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexCandidateView {
    executable_id: String,
    canonical_path: Option<String>,
    version: Option<String>,
    sources: Vec<String>,
    verification: &'static str,
    requires_confirmation: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopDataStatusView {
    availability: Availability,
    freshness: Freshness,
    compatibility: Compatibility,
    reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopAccountView {
    auth_mode: Option<String>,
    plan_type: Option<String>,
    binding_status: AccountBindingStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopQuotaWindowView {
    limit_id: String,
    label: Option<String>,
    window_minutes: Option<u64>,
    used_percent: f64,
    remaining_percent: f64,
    resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopResetCreditView {
    summary_status: SummaryStatus,
    available_count: Option<u64>,
    details_status: ResetCreditDetailsStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopUsageView {
    availability: Availability,
    has_summary: bool,
    reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopStatusView {
    schema_version: String,
    captured_at: String,
    codex_version: Option<String>,
    account: Option<DesktopAccountView>,
    data_status: DesktopDataStatusView,
    quota_windows: Vec<DesktopQuotaWindowView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quota_observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quota_freshness: Option<Freshness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quota_source: Option<&'static str>,
    reset_credits: DesktopResetCreditView,
    usage: DesktopUsageView,
    diagnostic_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopStatusEnvelope {
    schema_version: &'static str,
    sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_context_id: Option<String>,
    lifecycle: DesktopLifecycle,
    status: Option<DesktopStatusView>,
    candidates: Vec<CodexCandidateView>,
    selected_executable_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_executable_source: Option<&'static str>,
    issue: Option<DesktopIssue>,
    persistence_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopWorkPlanStatus {
    Available,
    Updated,
    RevisionConflict,
    InvalidRequest,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopScheduleBaselineView {
    kind: &'static str,
    limit_id: String,
    source: &'static str,
    observed_at: String,
    freshness: Freshness,
    comparison: SchedulePaceComparison,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWorkPlanEnvelope {
    schema_version: &'static str,
    status: DesktopWorkPlanStatus,
    reason_code: &'static str,
    schedule: Option<DesktopWorkScheduleView>,
    baseline: Option<DesktopScheduleBaselineView>,
}

/// A validated weekly quota observation supplied by the formal desktop host.
///
/// This is an in-process fallback for the active managed account when a
/// transient app-server read has no canonical Codex window. It is deliberately
/// not exposed as a WebView command, so renderer input cannot manufacture a
/// planning baseline.
#[derive(Clone)]
pub struct DesktopWorkPlanQuotaObservation {
    captured_at: UtcTimestamp,
    window_minutes: u64,
    remaining_percent: f64,
    resets_at: UtcTimestamp,
}

impl DesktopWorkPlanQuotaObservation {
    pub fn from_managed_account_weekly(
        captured_at: String,
        window_minutes: u64,
        remaining_percent: f64,
        resets_at: String,
    ) -> Option<Self> {
        if window_minutes <= 24 * 60
            || !remaining_percent.is_finite()
            || !(0.0..=100.0).contains(&remaining_percent)
        {
            return None;
        }
        Some(Self {
            captured_at: UtcTimestamp::parse(captured_at).ok()?,
            window_minutes,
            remaining_percent,
            resets_at: UtcTimestamp::parse(resets_at).ok()?,
        })
    }
}

#[derive(Clone)]
struct WorkPlanWindowInput {
    limit_id: String,
    window_minutes: u64,
    remaining_percent: f64,
    resets_at: UtcTimestamp,
    captured_at: UtcTimestamp,
    freshness: Freshness,
    source: &'static str,
    canonical_codex: bool,
}

struct OwnerBinding {
    executable_id: String,
    handle: RefreshHandle,
}

struct DesktopState {
    discovery: Mutex<DiscoveryReport>,
    owner: AsyncMutex<Option<OwnerBinding>>,
    command_gate: AsyncMutex<()>,
    persistence: Arc<Mutex<DesktopPersistence>>,
    account_binding_store: Arc<Mutex<DesktopAccountBindingStore>>,
    ephemeral_account_identity: Mutex<Option<DesktopAccountIdentity>>,
    monitor: MonitorControl,
    notification_engine: Mutex<NotificationEngine>,
    notification_sender: broadcast::Sender<capacity_domain::MonitorNotificationDecision>,
    status_sequence: AtomicU64,
}

impl DesktopState {
    #[cfg(test)]
    fn new(persistence: DesktopPersistence) -> Self {
        Self::new_with_discovery(persistence, discover_current(None))
    }

    fn new_with_explicit_executable(
        persistence: DesktopPersistence,
        explicit_executable: Option<PathBuf>,
    ) -> Self {
        Self::new_with_discovery(persistence, discover_current(explicit_executable))
    }

    fn new_with_discovery(persistence: DesktopPersistence, discovery: DiscoveryReport) -> Self {
        Self::new_with_account_binding_store_and_discovery(
            persistence,
            DesktopAccountBindingStore::platform_required(),
            discovery,
        )
    }

    #[cfg(test)]
    fn new_with_account_binding_store(
        persistence: DesktopPersistence,
        account_binding_store: DesktopAccountBindingStore,
    ) -> Self {
        Self::new_with_account_binding_store_and_discovery(
            persistence,
            account_binding_store,
            discover_current(None),
        )
    }

    fn new_with_account_binding_store_and_discovery(
        persistence: DesktopPersistence,
        account_binding_store: DesktopAccountBindingStore,
        discovery: DiscoveryReport,
    ) -> Self {
        let (notification_sender, _) = broadcast::channel(32);
        Self {
            discovery: Mutex::new(discovery),
            owner: AsyncMutex::new(None),
            command_gate: AsyncMutex::new(()),
            persistence: Arc::new(Mutex::new(persistence)),
            account_binding_store: Arc::new(Mutex::new(account_binding_store)),
            ephemeral_account_identity: Mutex::new(None),
            monitor: MonitorControl::new(),
            notification_engine: Mutex::new(NotificationEngine::default()),
            notification_sender,
            status_sequence: AtomicU64::new(0),
        }
    }

    fn stamp_status(&self, mut envelope: DesktopStatusEnvelope) -> DesktopStatusEnvelope {
        envelope.sequence = self
            .status_sequence
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if let Ok(store) = self.persistence.lock() {
            envelope.persistence_enabled = store.backend_available() && store.has_stable_binding();
            envelope.history_context_id = Some(store.history_context_id().to_owned());
        }
        envelope
    }

    fn discovery_snapshot(&self) -> Result<DiscoveryReport, DesktopIssue> {
        self.discovery
            .lock()
            .map(|report| report.clone())
            .map_err(|_| desktop_issue("desktop_state_failed"))
    }

    async fn owner_for_selected(&self) -> Result<RefreshHandle, DesktopIssue> {
        let discovery = self.discovery_snapshot()?;
        if discovery.outcome != DiscoveryOutcome::Selected {
            return Err(issue_for_discovery(&discovery));
        }
        let selected = discovery
            .selected_candidate()
            .ok_or_else(|| desktop_issue("codex_selection_required"))?;
        let mut owner = self.owner.lock().await;
        if let Some(binding) = owner.as_ref()
            && binding.executable_id == selected.executable_id
        {
            return Ok(binding.handle.clone());
        }
        if let Some(previous) = owner.take() {
            let _ = previous.handle.shutdown().await;
        }
        let source = CodexStatusSource::from_discovery(&discovery, RuntimeConfig::default())
            .map_err(|_| desktop_issue("codex_source_failed"))?;
        let handle = RefreshHandle::start(source, RefreshOwnerConfig::default())
            .map_err(|error| issue_for_refresh(&error))?;
        *owner = Some(OwnerBinding {
            executable_id: selected.executable_id.clone(),
            handle: handle.clone(),
        });
        Ok(handle)
    }

    async fn select_executable(&self, executable_id: &str) -> Result<(), DesktopIssue> {
        if !valid_executable_id(executable_id) {
            return Err(desktop_issue("codex_candidate_invalid"));
        }
        let discovery = self.discovery_snapshot()?;
        let Some(candidate) = discovery
            .candidates
            .iter()
            .find(|candidate| candidate.executable_id == executable_id)
            .cloned()
        else {
            // A queued click may refer to the identity from before an app update.
            // Return a fresh choice, not an error paired with the same stale list.
            self.rediscover_after_executable_change().await?;
            return Err(desktop_issue("codex_candidate_changed"));
        };
        let explicit_path = PathBuf::from(&candidate.canonical_path);
        let verified = tokio::task::spawn_blocking(move || discover_current(Some(explicit_path)))
            .await
            .map_err(|_| desktop_issue("codex_verification_failed"))?;
        let (next, issue) = confirmed_executable_discovery(&candidate, verified);
        self.replace_executable_discovery(next).await?;
        issue.map_or(Ok(()), Err)
    }

    async fn replace_executable_discovery(
        &self,
        discovery: DiscoveryReport,
    ) -> Result<(), DesktopIssue> {
        self.stop_owner().await;
        // Executable provenance is not account identity. Keep the independently
        // verified HMAC binding and its last history scope during installation
        // review. Actual account/boundary changes still invalidate the binding.
        self.reset_notification_engine();
        *self
            .discovery
            .lock()
            .map_err(|_| desktop_issue("desktop_state_failed"))? = discovery;
        Ok(())
    }

    async fn rediscover_after_executable_change(&self) -> Result<DiscoveryReport, DesktopIssue> {
        self.stop_owner().await;
        let discovery = tokio::task::spawn_blocking(|| {
            require_confirmation_after_executable_change(discover_current(None))
        })
        .await
        .map_err(|_| desktop_issue("codex_verification_failed"))?;
        self.replace_executable_discovery(discovery.clone()).await?;
        Ok(discovery)
    }

    async fn shutdown(&self) {
        self.monitor.shutdown();
        let _command = self.command_gate.lock().await;
        if let Some(binding) = self.owner.lock().await.take() {
            let _ = binding.handle.shutdown().await;
        }
    }

    fn persistence_backend_available(&self) -> bool {
        self.persistence
            .lock()
            .is_ok_and(|persistence| persistence.backend_available())
    }

    async fn persist_snapshot(&self, snapshot: &StatusSnapshot) -> PersistenceRecordResult {
        let persistence = Arc::clone(&self.persistence);
        let snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || {
            persistence
                .lock()
                .map_or(PersistenceRecordResult::Failed, |mut persistence| {
                    persistence.record_snapshot(&snapshot)
                })
        })
        .await
        .unwrap_or(PersistenceRecordResult::Failed)
    }

    fn apply_stable_account_binding(&self, refresh: &mut codex_runtime::refresh::RefreshSnapshot) {
        let stable = self
            .persistence
            .lock()
            .is_ok_and(|persistence| persistence.has_stable_binding());
        if !stable {
            return;
        }
        if let Some(account) = refresh.snapshot.account.as_mut()
            && account
                .auth_mode
                .as_deref()
                .is_some_and(|mode| mode.eq_ignore_ascii_case("chatgpt"))
        {
            account.binding_status = AccountBindingStatus::Stable;
        }
    }

    async fn bind_account_identity(
        &self,
        identity: DesktopAccountIdentity,
    ) -> Result<bool, DesktopIssue> {
        let _command = self.command_gate.lock().await;
        let binding_store = Arc::clone(&self.account_binding_store);
        let requested_identity = identity.clone();
        let fingerprint = tokio::task::spawn_blocking(move || {
            binding_store
                .lock()
                .map_err(|_| AccountBindingError::PlatformCredentialUnavailable)?
                .fingerprint(&identity)
        })
        .await
        .map_err(|_| desktop_issue("account_binding_failed"))?
        .map_err(|error| desktop_issue(error.reason_code()));
        let fingerprint = match fingerprint {
            Ok(fingerprint) => fingerprint,
            Err(issue) => {
                let same_ephemeral_account = self
                    .ephemeral_account_identity
                    .lock()
                    .is_ok_and(|current| current.as_ref() == Some(&requested_identity));
                // Protected history can be unavailable while live reads work.
                // Repeatedly checking the same identity must not erase its
                // quota snapshot or stop its app-server connection.
                if !same_ephemeral_account {
                    self.stop_owner().await;
                    self.rotate_ephemeral_binding();
                }
                if let Ok(mut persistence) = self.persistence.lock() {
                    persistence.set_binding_failure(issue.code);
                }
                *self
                    .ephemeral_account_identity
                    .lock()
                    .map_err(|_| desktop_issue("desktop_state_failed"))? = Some(requested_identity);
                return Err(issue);
            }
        };
        *self
            .ephemeral_account_identity
            .lock()
            .map_err(|_| desktop_issue("desktop_state_failed"))? = None;
        let changed = self
            .persistence
            .lock()
            .map_err(|_| desktop_issue("desktop_state_failed"))?
            .set_stable_binding(fingerprint);
        if changed {
            if let Some(binding) = self.owner.lock().await.take() {
                let _ = binding.handle.shutdown().await;
            }
            self.reset_notification_engine();
        }
        Ok(changed)
    }

    async fn history(
        &self,
        limit_id: String,
        maximum_points: Option<u32>,
    ) -> DesktopHistoryEnvelope {
        let persistence = Arc::clone(&self.persistence);
        let fallback_limit_id = limit_id.clone();
        tokio::task::spawn_blocking(move || {
            persistence.lock().map_or_else(
                |_| DesktopPersistence::unavailable().history(&limit_id, maximum_points),
                |persistence| persistence.history(&limit_id, maximum_points),
            )
        })
        .await
        .unwrap_or_else(|_| {
            DesktopPersistence::unavailable().history(&fallback_limit_id, maximum_points)
        })
    }

    async fn settings(&self) -> DesktopSettingsEnvelope {
        let persistence = Arc::clone(&self.persistence);
        tokio::task::spawn_blocking(move || {
            persistence.lock().map_or_else(
                |_| DesktopPersistence::unavailable().settings(),
                |persistence| persistence.settings(),
            )
        })
        .await
        .unwrap_or_else(|_| DesktopPersistence::unavailable().settings())
    }

    async fn work_schedule(&self) -> DesktopWorkScheduleEnvelope {
        let persistence = Arc::clone(&self.persistence);
        tokio::task::spawn_blocking(move || {
            persistence.lock().map_or_else(
                |_| DesktopPersistence::unavailable().work_schedule(),
                |persistence| persistence.work_schedule(),
            )
        })
        .await
        .unwrap_or_else(|_| DesktopPersistence::unavailable().work_schedule())
    }

    async fn monitor_settings(&self) -> Option<capacity_store::MonitorSettings> {
        let persistence = Arc::clone(&self.persistence);
        tokio::task::spawn_blocking(move || {
            persistence
                .lock()
                .ok()
                .and_then(|persistence| persistence.monitor_settings())
        })
        .await
        .ok()
        .flatten()
    }

    async fn vault_mutation_status(
        &self,
        lock_root: Option<PathBuf>,
    ) -> DesktopVaultMutationStatusEnvelope {
        let persistence = Arc::clone(&self.persistence);
        tokio::task::spawn_blocking(move || inspect_vault_mutation_status(persistence, lock_root))
            .await
            .unwrap_or_else(|_| failed_vault_mutation_status("mutation_status_task_failed"))
    }

    async fn auto_refresh_schedule(&self) -> AutoRefreshSchedule {
        let settings = self.monitor_settings().await;
        AutoRefreshSchedule::from_settings(settings.as_ref())
    }

    async fn observe_monitor_snapshot(&self, snapshot: &StatusSnapshot) {
        let Some(settings) = self.monitor_settings().await else {
            return;
        };
        let Some(policy) = notification_policy(&settings) else {
            return;
        };
        let evaluated_at = SystemClock.now();
        let decisions = self.notification_engine.lock().map_or_else(
            |_| Vec::new(),
            |mut engine| engine.observe(snapshot, policy, &evaluated_at, current_local_minute()),
        );
        publish_deliverable(&self.notification_sender, decisions);
    }

    async fn update_settings(
        &self,
        update: DesktopSettingsUpdateRequest,
    ) -> DesktopSettingsEnvelope {
        let persistence = Arc::clone(&self.persistence);
        let fallback = update.clone();
        let updated_at = SystemClock.now();
        tokio::task::spawn_blocking(move || {
            persistence.lock().map_or_else(
                |_| DesktopPersistence::unavailable().update_settings(fallback, &updated_at),
                |mut persistence| persistence.update_settings(update, &updated_at),
            )
        })
        .await
        .unwrap_or_else(|_| DesktopPersistence::unavailable().settings())
    }

    async fn update_work_schedule(
        &self,
        update: DesktopWorkScheduleUpdateRequest,
    ) -> DesktopWorkScheduleEnvelope {
        let persistence = Arc::clone(&self.persistence);
        let fallback = update.clone();
        let updated_at = SystemClock.now();
        tokio::task::spawn_blocking(move || {
            persistence.lock().map_or_else(
                |_| DesktopPersistence::unavailable().update_work_schedule(fallback, &updated_at),
                |mut persistence| persistence.update_work_schedule(update, &updated_at),
            )
        })
        .await
        .unwrap_or_else(|_| DesktopPersistence::unavailable().work_schedule())
    }

    async fn delete_all_local_data(
        &self,
        request: DesktopDeleteAllRequest,
    ) -> DesktopDeleteAllEnvelope {
        let persistence = Arc::clone(&self.persistence);
        tokio::task::spawn_blocking(move || {
            persistence.lock().map_or_else(
                |_| {
                    DesktopPersistence::unavailable().delete_all_local_data(
                        DesktopDeleteAllRequest {
                            expected_settings_revision: 1,
                            confirmed: true,
                        },
                    )
                },
                |mut persistence| persistence.delete_all_local_data(request),
            )
        })
        .await
        .unwrap_or_else(|_| {
            DesktopPersistence::unavailable().delete_all_local_data(DesktopDeleteAllRequest {
                expected_settings_revision: 1,
                confirmed: true,
            })
        })
    }

    async fn stop_owner(&self) {
        if let Some(binding) = self.owner.lock().await.take() {
            let _ = binding.handle.shutdown().await;
        }
    }

    fn rotate_ephemeral_binding(&self) {
        if let Ok(mut identity) = self.ephemeral_account_identity.lock() {
            *identity = None;
        }
        if let Ok(mut persistence) = self.persistence.lock() {
            persistence.rotate_ephemeral_binding();
        }
        self.reset_notification_engine();
    }

    fn reset_notification_engine(&self) {
        if let Ok(mut engine) = self.notification_engine.lock() {
            engine.reset();
        }
    }
}

/// Embed the trusted capacity application service into a Tauri product host.
///
/// The formal QuotaHorizon shell already owns its tray, so callers can disable
/// the preview tray while keeping the same refresh, persistence, diagnostics,
/// notification, and vault-recovery service.
pub fn initialize(app: &mut tauri::App, install_capacity_tray: bool) -> tauri::Result<()> {
    initialize_with_explicit_executable(app, install_capacity_tray, None)
}

/// Embed the capacity service with a host-selected executable.
///
/// This is a privileged Rust-host input, not an IPC surface. It lets a product
/// host choose its own trusted bundled Codex while the generic preview keeps
/// the normal multi-candidate confirmation flow.
pub fn initialize_with_explicit_executable(
    app: &mut tauri::App,
    install_capacity_tray: bool,
    explicit_executable: Option<PathBuf>,
) -> tauri::Result<()> {
    let persistence = app
        .path()
        .app_data_dir()
        .map(|data_directory| {
            DesktopPersistence::open(&data_directory.join(CAPACITY_DATA_DIRECTORY))
        })
        .unwrap_or_else(|_| DesktopPersistence::unavailable());
    let state = DesktopState::new_with_explicit_executable(persistence, explicit_executable);
    let monitor_commands = state.monitor.subscribe();
    let notification_events = state.notification_sender.subscribe();
    app.manage(state);
    if install_capacity_tray {
        tray::install(app)?;
    }
    let monitor_handle = app.handle().clone();
    tauri::async_runtime::spawn(run_auto_refresh(monitor_handle, monitor_commands));
    let notification_handle = app.handle().clone();
    tauri::async_runtime::spawn(run_notification_bridge(
        notification_handle,
        notification_events,
    ));
    Ok(())
}

pub fn implementation_status(app_handle: &AppHandle) -> ImplementationStatus {
    let state = app_handle.state::<DesktopState>();
    ImplementationStatus {
        phase: "wpr4a_stable_account_history",
        desktop_live_reads_enabled: true,
        store_backend_available: state.persistence_backend_available(),
        mutation_status_enabled: true,
    }
}

/// Binds future snapshots to the formal host's one-way account digest. This is
/// an in-process Rust capability and is intentionally not registered as a
/// WebView or loopback command.
pub async fn bind_account_identity(
    app_handle: AppHandle,
    identity: DesktopAccountIdentity,
) -> Result<bool, DesktopIssue> {
    app_handle
        .state::<DesktopState>()
        .bind_account_identity(identity)
        .await
}

/// Explicit native user action only. This does not take the quota command
/// gate: live reads must continue while macOS waits for the user's decision.
pub async fn authorize_history_keychain() -> Result<(), DesktopIssue> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(|| {
            capacity_vault::MacOsKeychainInstallationKeyStore::authorize_local_history_access()
                .map_err(|error| {
                    desktop_issue(AccountBindingError::from_key_error(error).reason_code())
                })
        })
        .await
        .map_err(|_| desktop_issue("history_keychain_unavailable"))?
    }
    #[cfg(not(target_os = "macos"))]
    Err(desktop_issue("history_keychain_unavailable"))
}

/// Immediately prevents a switch in progress from persisting new reads under
/// the previous account. The next formal-host read can establish a fresh
/// stable binding after the account transition completes.
pub fn invalidate_account_binding<R: tauri::Runtime>(app_handle: &AppHandle<R>) {
    let state = app_handle.state::<DesktopState>();
    state.rotate_ephemeral_binding();
}

/// Re-reads monitor and Work Plan settings after a trusted host-side logical
/// transaction updates the shared SQLite store. This is deliberately an
/// in-process capability and is not registered as IPC.
pub fn notify_persistence_settings_changed<R: tauri::Runtime>(app_handle: &AppHandle<R>) {
    let state = app_handle.state::<DesktopState>();
    state.reset_notification_engine();
    state.monitor.reconfigure();
}

pub async fn status(app_handle: AppHandle) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let envelope = state.stamp_status(get_status_inner(&state).await);
    tray::update(&app_handle, &envelope);
    Ok(envelope)
}

pub async fn refresh(app_handle: AppHandle) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let envelope = state.stamp_status(refresh_status_inner(&state).await);
    tray::update(&app_handle, &envelope);
    Ok(envelope)
}

pub async fn select_executable(
    app_handle: AppHandle,
    executable_id: String,
) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    if let Err(issue) = state.select_executable(&executable_id).await {
        let envelope = state.stamp_status(envelope_from_discovery(
            state.discovery_snapshot().ok(),
            None,
            Some(issue),
        ));
        tray::update(&app_handle, &envelope);
        return Ok(envelope);
    }
    let envelope = state.stamp_status(refresh_status_inner(&state).await);
    tray::update(&app_handle, &envelope);
    Ok(envelope)
}

pub async fn history(
    app_handle: AppHandle,
    limit_id: String,
    maximum_points: Option<u32>,
) -> Result<DesktopHistoryEnvelope, DesktopIssue> {
    Ok(app_handle
        .state::<DesktopState>()
        .history(limit_id, maximum_points)
        .await)
}

pub async fn work_plan(app_handle: AppHandle) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    work_plan_with_quota_observation(app_handle, None).await
}

/// Use the formal host's reconciled quota snapshot, not a second independent read.
pub async fn work_plan_for_status(
    app_handle: AppHandle,
    status: DesktopStatusEnvelope,
) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    Ok(work_plan_envelope(
        state.work_schedule().await,
        &status,
        &SystemClock.now(),
        None,
    ))
}

pub async fn update_work_schedule_for_status(
    app_handle: AppHandle,
    request: DesktopWorkScheduleUpdateRequest,
    status: DesktopStatusEnvelope,
) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    Ok(work_plan_envelope(
        state.update_work_schedule(request).await,
        &status,
        &SystemClock.now(),
        None,
    ))
}

pub async fn work_plan_with_quota_observation(
    app_handle: AppHandle,
    observation: Option<DesktopWorkPlanQuotaObservation>,
) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let status = get_status_inner(&state).await;
    let schedule = state.work_schedule().await;
    Ok(work_plan_envelope(
        schedule,
        &status,
        &SystemClock.now(),
        observation.as_ref(),
    ))
}

pub async fn update_work_schedule(
    app_handle: AppHandle,
    request: DesktopWorkScheduleUpdateRequest,
) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    update_work_schedule_with_quota_observation(app_handle, request, None).await
}

pub async fn update_work_schedule_with_quota_observation(
    app_handle: AppHandle,
    request: DesktopWorkScheduleUpdateRequest,
    observation: Option<DesktopWorkPlanQuotaObservation>,
) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let schedule = state.update_work_schedule(request).await;
    let status = get_status_inner(&state).await;
    Ok(work_plan_envelope(
        schedule,
        &status,
        &SystemClock.now(),
        observation.as_ref(),
    ))
}

pub async fn diagnostics(
    app_handle: AppHandle,
) -> Result<DesktopDiagnosticsEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    Ok(diagnostics::preview(&state).await)
}

pub async fn vault_mutation_status(
    app_handle: AppHandle,
) -> Result<DesktopVaultMutationStatusEnvelope, DesktopIssue> {
    let state = app_handle.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    Ok(state
        .vault_mutation_status(trusted_mutation_lock_root(&app_handle))
        .await)
}

pub fn shutdown<R: tauri::Runtime>(app_handle: &AppHandle<R>) {
    let state = app_handle.state::<DesktopState>();
    tauri::async_runtime::block_on(state.shutdown());
}

#[tauri::command]
fn get_implementation_status(app_handle: AppHandle) -> ImplementationStatus {
    implementation_status(&app_handle)
}

#[tauri::command]
async fn get_status(app_handle: AppHandle) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    status(app_handle).await
}

#[tauri::command]
async fn refresh_status(app_handle: AppHandle) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    refresh(app_handle).await
}

#[tauri::command]
async fn select_codex_executable(
    executable_id: String,
    app_handle: AppHandle,
) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    select_executable(app_handle, executable_id).await
}

#[tauri::command]
async fn get_history(
    limit_id: String,
    maximum_points: Option<u32>,
    app_handle: AppHandle,
) -> Result<DesktopHistoryEnvelope, DesktopIssue> {
    history(app_handle, limit_id, maximum_points).await
}

#[tauri::command]
async fn get_work_plan(app_handle: AppHandle) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    work_plan(app_handle).await
}

#[tauri::command]
async fn update_work_plan_schedule(
    request: DesktopWorkScheduleUpdateRequest,
    app_handle: AppHandle,
) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    update_work_schedule(app_handle, request).await
}

#[tauri::command]
async fn get_settings(
    state: State<'_, DesktopState>,
) -> Result<DesktopSettingsEnvelope, DesktopIssue> {
    Ok(state.settings().await)
}

#[tauri::command]
async fn get_diagnostics_preview(
    app_handle: AppHandle,
) -> Result<DesktopDiagnosticsEnvelope, DesktopIssue> {
    diagnostics(app_handle).await
}

#[tauri::command]
async fn get_vault_mutation_status(
    app_handle: AppHandle,
) -> Result<DesktopVaultMutationStatusEnvelope, DesktopIssue> {
    vault_mutation_status(app_handle).await
}

#[tauri::command]
async fn update_settings(
    update: DesktopSettingsUpdateRequest,
    state: State<'_, DesktopState>,
) -> Result<DesktopSettingsEnvelope, DesktopIssue> {
    let _command = state.command_gate.lock().await;
    let result = state.update_settings(update).await;
    if result.status == persistence::DesktopSettingsStatus::Updated {
        state.monitor.reconfigure();
    }
    Ok(result)
}

#[tauri::command]
async fn delete_all_local_data(
    request: DesktopDeleteAllRequest,
    app_handle: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<DesktopDeleteAllEnvelope, DesktopIssue> {
    let _command = state.command_gate.lock().await;
    let result = state.delete_all_local_data(request).await;
    if result.status == DesktopDeleteAllStatus::Deleted {
        state.stop_owner().await;
        state.reset_notification_engine();
        state.monitor.reconfigure();
        tray::reset_after_data_deletion(&app_handle, &state).await;
    }
    Ok(result)
}

async fn get_status_inner(state: &DesktopState) -> DesktopStatusEnvelope {
    let discovery = match state.discovery_snapshot() {
        Ok(discovery) => discovery,
        Err(issue) => return envelope_from_discovery(None, None, Some(issue)),
    };
    if discovery.outcome != DiscoveryOutcome::Selected {
        let issue = issue_for_discovery(&discovery);
        return envelope_from_discovery(Some(discovery), None, Some(issue));
    }

    let owner = state.owner.lock().await;
    let Some(binding) = owner.as_ref() else {
        return envelope_from_discovery(Some(discovery), None, None);
    };
    let handle = binding.handle.clone();
    drop(owner);
    match handle.latest().await {
        Ok(mut status) => {
            if let Some(refresh) = status.as_mut() {
                state.apply_stable_account_binding(refresh);
            }
            envelope_from_discovery(Some(discovery), status, None)
        }
        Err(error) => {
            envelope_from_discovery(Some(discovery), None, Some(issue_for_refresh(&error)))
        }
    }
}

async fn refresh_status_inner(state: &DesktopState) -> DesktopStatusEnvelope {
    let discovery = match state.discovery_snapshot() {
        Ok(discovery) => discovery,
        Err(issue) => return envelope_from_discovery(None, None, Some(issue)),
    };
    if discovery.outcome != DiscoveryOutcome::Selected {
        // Refresh also repairs stale/failed discovery, without silently trusting
        // a replacement executable or starting an unconfirmed app-server.
        return match state.rediscover_after_executable_change().await {
            Ok(current) => {
                let issue = issue_for_discovery(&current);
                envelope_from_discovery(Some(current), None, Some(issue))
            }
            Err(issue) => envelope_from_discovery(Some(discovery), None, Some(issue)),
        };
    }
    let handle = match state.owner_for_selected().await {
        Ok(handle) => handle,
        Err(issue) => return envelope_from_discovery(Some(discovery), None, Some(issue)),
    };
    match handle.refresh().await {
        Ok(mut status) => {
            state.apply_stable_account_binding(&mut status);
            let _ = state.persist_snapshot(&status.snapshot).await;
            state.observe_monitor_snapshot(&status.snapshot).await;
            envelope_from_discovery(Some(discovery), Some(status), None)
        }
        Err(error) => {
            let mut latest = handle.latest().await.ok().flatten();
            if let Some(refresh) = latest.as_mut() {
                state.apply_stable_account_binding(refresh);
            }
            let issue = issue_for_refresh(&error);
            if matches!(
                error,
                RefreshOwnerError::Source(
                    RefreshFailure::AccountChanged | RefreshFailure::AccountBoundaryInvalid
                )
            ) {
                state.rotate_ephemeral_binding();
                return envelope_from_discovery(Some(discovery), None, Some(issue));
            }
            if matches!(
                error,
                RefreshOwnerError::Source(RefreshFailure::ExecutableChanged)
            ) {
                return match state.rediscover_after_executable_change().await {
                    Ok(rediscovered) => {
                        envelope_from_discovery(Some(rediscovered), latest, Some(issue))
                    }
                    Err(rediscovery_issue) => {
                        envelope_from_discovery(Some(discovery), latest, Some(rediscovery_issue))
                    }
                };
            }
            if let Some(refresh) = latest.as_ref() {
                state.observe_monitor_snapshot(&refresh.snapshot).await;
            }
            envelope_from_discovery(Some(discovery), latest, Some(issue))
        }
    }
}

fn work_plan_envelope(
    schedule_envelope: DesktopWorkScheduleEnvelope,
    status_envelope: &DesktopStatusEnvelope,
    now: &UtcTimestamp,
    fallback_observation: Option<&DesktopWorkPlanQuotaObservation>,
) -> DesktopWorkPlanEnvelope {
    work_plan_envelope_with_resolver(
        schedule_envelope,
        status_envelope,
        now,
        fallback_observation,
        |timestamp| {
            let local = timestamp.with_timezone(&chrono::Local);
            u16::try_from(local.hour() * 60 + local.minute()).unwrap_or_default()
        },
    )
}

fn work_plan_envelope_with_resolver<F>(
    schedule_envelope: DesktopWorkScheduleEnvelope,
    status_envelope: &DesktopStatusEnvelope,
    now: &UtcTimestamp,
    fallback_observation: Option<&DesktopWorkPlanQuotaObservation>,
    local_minute: F,
) -> DesktopWorkPlanEnvelope
where
    F: Fn(&DateTime<Utc>) -> u16,
{
    let status = match schedule_envelope.status {
        DesktopWorkScheduleStatus::Available => DesktopWorkPlanStatus::Available,
        DesktopWorkScheduleStatus::Updated => DesktopWorkPlanStatus::Updated,
        DesktopWorkScheduleStatus::RevisionConflict => DesktopWorkPlanStatus::RevisionConflict,
        DesktopWorkScheduleStatus::InvalidRequest => DesktopWorkPlanStatus::InvalidRequest,
        DesktopWorkScheduleStatus::Failed => DesktopWorkPlanStatus::Failed,
    };
    let schedule_view = schedule_envelope.schedule;
    let baseline = schedule_view.as_ref().and_then(|view| {
        let periods = view
            .off_periods
            .iter()
            .map(|period| {
                WorkSchedulePeriod::new(period.start_minute_of_day, period.end_minute_of_day)
            })
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let schedule = WorkSchedule::new(view.enabled, periods).ok()?;
        let now_datetime = DateTime::parse_from_rfc3339(now.as_str())
            .ok()?
            .with_timezone(&Utc);
        let window = select_work_plan_window(status_envelope, fallback_observation, &now_datetime)?;
        let comparison = evaluate_schedule_pace(
            window.window_minutes,
            &window.resets_at,
            window.remaining_percent,
            now,
            &schedule,
            &local_minute,
        )
        .ok()?;
        Some(DesktopScheduleBaselineView {
            kind: "schedule_baseline",
            limit_id: window.limit_id,
            source: window.source,
            observed_at: window.captured_at.as_str().to_owned(),
            freshness: window.freshness,
            comparison,
        })
    });
    let reason_code = match status {
        DesktopWorkPlanStatus::Updated => "work_schedule_updated",
        DesktopWorkPlanStatus::RevisionConflict => "work_schedule_revision_conflict",
        DesktopWorkPlanStatus::InvalidRequest => "work_schedule_request_invalid",
        DesktopWorkPlanStatus::Failed => schedule_envelope.reason_code,
        DesktopWorkPlanStatus::Available
            if baseline
                .as_ref()
                .is_some_and(|baseline| baseline.source == "managed_account_cache") =>
        {
            "work_plan_account_cache"
        }
        DesktopWorkPlanStatus::Available if baseline.is_some() => "work_plan_available",
        DesktopWorkPlanStatus::Available => "work_plan_window_unavailable",
    };
    DesktopWorkPlanEnvelope {
        schema_version: DESKTOP_SCHEMA_VERSION,
        status,
        reason_code,
        schedule: schedule_view,
        baseline,
    }
}

fn select_work_plan_window(
    status_envelope: &DesktopStatusEnvelope,
    fallback_observation: Option<&DesktopWorkPlanQuotaObservation>,
    now: &DateTime<Utc>,
) -> Option<WorkPlanWindowInput> {
    let status = status_envelope.status.as_ref();
    let status_window = status.and_then(|status| {
        let valid_windows = || {
            status.quota_windows.iter().filter_map(|window| {
                let window_minutes = window.window_minutes?;
                let resets_at = UtcTimestamp::parse(window.resets_at.as_ref()?.clone()).ok()?;
                let reset_datetime = DateTime::parse_from_rfc3339(resets_at.as_str())
                    .ok()?
                    .with_timezone(&Utc);
                (window_minutes > 0
                    && window.remaining_percent.is_finite()
                    && (0.0..=100.0).contains(&window.remaining_percent)
                    && reset_datetime > *now)
                    .then_some((window, window_minutes, resets_at))
            })
        };
        let selected = valid_windows()
            .filter(|(window, _, _)| canonical_codex_limit(&window.limit_id))
            .max_by_key(|(_, duration, _)| *duration)
            .or_else(|| valid_windows().max_by_key(|(_, duration, _)| *duration))?;
        Some(WorkPlanWindowInput {
            limit_id: selected.0.limit_id.clone(),
            window_minutes: selected.1,
            remaining_percent: selected.0.remaining_percent,
            resets_at: selected.2,
            captured_at: UtcTimestamp::parse(
                status
                    .quota_observed_at
                    .as_ref()
                    .unwrap_or(&status.captured_at)
                    .clone(),
            )
            .ok()?,
            freshness: status
                .quota_freshness
                .unwrap_or(status.data_status.freshness),
            source: status.quota_source.unwrap_or("app_server_status"),
            canonical_codex: canonical_codex_limit(&selected.0.limit_id),
        })
    });
    let fallback_window = fallback_observation.and_then(|observation| {
        let reset_datetime = DateTime::parse_from_rfc3339(observation.resets_at.as_str())
            .ok()?
            .with_timezone(&Utc);
        (reset_datetime > *now).then(|| WorkPlanWindowInput {
            limit_id: "codex:managed-account-weekly".to_owned(),
            window_minutes: observation.window_minutes,
            remaining_percent: observation.remaining_percent,
            resets_at: observation.resets_at.clone(),
            captured_at: observation.captured_at.clone(),
            freshness: Freshness::Stale,
            source: "managed_account_cache",
            canonical_codex: true,
        })
    });

    match (status_window, fallback_window) {
        (Some(status), Some(fallback))
            if !status.canonical_codex
                || (timestamp_datetime(&fallback.captured_at)
                    > timestamp_datetime(&status.captured_at)) =>
        {
            Some(fallback)
        }
        (Some(status), _) => Some(status),
        (None, fallback) => fallback,
    }
}

fn canonical_codex_limit(limit_id: &str) -> bool {
    limit_id == "codex" || limit_id.starts_with("codex:")
}

fn timestamp_datetime(timestamp: &UtcTimestamp) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp.as_str())
        .expect("validated UTC timestamp")
        .with_timezone(&Utc)
}

fn confirmed_executable_discovery(
    expected: &CodexExecutableCandidate,
    verified: DiscoveryReport,
) -> (DiscoveryReport, Option<DesktopIssue>) {
    if verified.outcome != DiscoveryOutcome::Selected {
        let issue = issue_for_discovery(&verified);
        return (
            require_confirmation_after_executable_change(verified),
            Some(issue),
        );
    }
    if verified.selected_candidate().is_some_and(|candidate| {
        candidate.file_identity == expected.file_identity
            && candidate.canonical_path == expected.canonical_path
            && candidate.identity_strength == expected.identity_strength
            && candidate.executable_id == expected.executable_id
    }) {
        (verified, None)
    } else {
        // Verification raced a replacement. Expose the replacement for a new
        // explicit confirmation instead of retaining an unconfirmable old ID.
        (
            require_confirmation_after_executable_change(verified),
            Some(desktop_issue("codex_candidate_changed")),
        )
    }
}

fn require_confirmation_after_executable_change(mut discovery: DiscoveryReport) -> DiscoveryReport {
    let selectable_count = discovery
        .candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.verification,
                CandidateVerification::Verified | CandidateVerification::ConfirmationRequired
            )
        })
        .count();
    discovery.selected_executable_id = None;
    if selectable_count > 0 {
        for candidate in &mut discovery.candidates {
            candidate.requires_confirmation = true;
        }
        discovery.outcome = if selectable_count == 1 {
            DiscoveryOutcome::ConfirmationRequired
        } else {
            DiscoveryOutcome::Ambiguous
        };
    }
    discovery
}

fn envelope_from_discovery(
    discovery: Option<DiscoveryReport>,
    refresh: Option<codex_runtime::refresh::RefreshSnapshot>,
    issue: Option<DesktopIssue>,
) -> DesktopStatusEnvelope {
    let status = refresh.map(|refresh| DesktopStatusView::from(&refresh.snapshot));
    let lifecycle = match discovery.as_ref().map(|report| report.outcome) {
        Some(DiscoveryOutcome::ConfirmationRequired | DiscoveryOutcome::Ambiguous) => {
            DesktopLifecycle::SelectionRequired
        }
        Some(DiscoveryOutcome::NotFound | DiscoveryOutcome::VerificationFailed) => {
            DesktopLifecycle::Error
        }
        Some(DiscoveryOutcome::Selected) | None => {
            if status
                .as_ref()
                .is_some_and(|status| status.data_status.freshness == Freshness::Stale)
            {
                DesktopLifecycle::Stale
            } else if status.is_some() {
                DesktopLifecycle::Ready
            } else if issue.is_some() {
                DesktopLifecycle::Error
            } else {
                DesktopLifecycle::Idle
            }
        }
    };
    let selected_executable_id = discovery
        .as_ref()
        .and_then(|report| report.selected_executable_id.clone());
    let candidates = discovery.as_ref().map_or_else(Vec::new, |report| {
        if matches!(
            report.outcome,
            DiscoveryOutcome::ConfirmationRequired | DiscoveryOutcome::Ambiguous
        ) {
            candidate_views(&report.candidates)
        } else {
            Vec::new()
        }
    });
    // Only the source category is needed in the normal view. Candidate paths
    // remain restricted to the explicit installation-confirmation view.
    let selected_executable_source = discovery
        .as_ref()
        .and_then(|report| report.selected_candidate())
        .map(|candidate| {
            let path = std::path::Path::new(&candidate.canonical_path);
            let bundled_path = is_bundled_codex_path(path);
            if bundled_path
                || candidate.sources.iter().any(|source| {
                    matches!(
                        source.as_str(),
                        "macos_chatgpt_bundle" | "macos_bundle_registry"
                    )
                })
            {
                "desktop_app"
            } else {
                "cli"
            }
        });
    DesktopStatusEnvelope {
        schema_version: DESKTOP_SCHEMA_VERSION,
        sequence: 0,
        history_context_id: None,
        lifecycle,
        status,
        candidates,
        selected_executable_id,
        selected_executable_source,
        issue,
        persistence_enabled: false,
    }
}

fn candidate_views(candidates: &[CodexExecutableCandidate]) -> Vec<CodexCandidateView> {
    candidates
        .iter()
        .take(MAX_DESKTOP_CANDIDATES)
        .map(|candidate| CodexCandidateView {
            executable_id: candidate.executable_id.clone(),
            canonical_path: safe_candidate_path(&candidate.canonical_path),
            version: candidate.version.clone(),
            sources: candidate
                .sources
                .iter()
                .map(|source| source.as_str().to_owned())
                .collect(),
            verification: match candidate.verification {
                CandidateVerification::Verified => "verified",
                CandidateVerification::ConfirmationRequired => "confirmation_required",
                CandidateVerification::VersionTimeout => "version_timeout",
                CandidateVerification::VersionFailed => "version_failed",
            },
            requires_confirmation: candidate.requires_confirmation,
        })
        .collect()
}

fn safe_candidate_path(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

impl From<&StatusSnapshot> for DesktopStatusView {
    fn from(status: &StatusSnapshot) -> Self {
        Self {
            schema_version: status.schema_version.clone(),
            captured_at: status.captured_at.as_str().to_owned(),
            codex_version: status
                .codex_executable
                .as_ref()
                .and_then(|executable| executable.version.clone()),
            account: status.account.as_ref().map(|account| DesktopAccountView {
                auth_mode: account.auth_mode.clone(),
                plan_type: account.plan_type.clone(),
                binding_status: account.binding_status,
            }),
            data_status: DesktopDataStatusView {
                availability: status.data_status.availability,
                freshness: status.data_status.freshness,
                compatibility: status.data_status.compatibility,
                reason_codes: status
                    .data_status
                    .reason_codes
                    .iter()
                    .map(|code| code.as_str().to_owned())
                    .collect(),
            },
            quota_windows: status
                .quota
                .windows
                .iter()
                .map(|window| DesktopQuotaWindowView {
                    limit_id: window.limit_id.clone(),
                    label: window.label.clone(),
                    window_minutes: window.window_minutes,
                    used_percent: window.used_percent,
                    remaining_percent: window.remaining_percent,
                    resets_at: window
                        .resets_at
                        .as_ref()
                        .map(|timestamp| timestamp.as_str().to_owned()),
                })
                .collect(),
            quota_observed_at: None,
            quota_freshness: None,
            quota_source: None,
            reset_credits: DesktopResetCreditView {
                summary_status: status.quota.reset_credit_summary.summary_status,
                available_count: status.quota.reset_credit_summary.available_count,
                details_status: status.quota.reset_credit_summary.details_status,
            },
            usage: DesktopUsageView {
                availability: status.usage.availability,
                has_summary: status.usage.summary.is_some(),
                reason_codes: status
                    .usage
                    .reason_codes
                    .iter()
                    .map(|code| code.as_str().to_owned())
                    .collect(),
            },
            diagnostic_codes: status
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str().to_owned())
                .collect(),
        }
    }
}

fn valid_executable_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn trusted_mutation_lock_root(app_handle: &AppHandle) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        app_handle
            .path()
            .home_dir()
            .ok()
            .and_then(|home| capacity_mutation::macos_shared_mutation_lock_root(home).ok())
    }

    #[cfg(not(target_os = "macos"))]
    {
        app_handle
            .path()
            .app_data_dir()
            .ok()
            .map(|directory| directory.join("mutation-coordination-v1"))
    }
}

fn issue_for_discovery(discovery: &DiscoveryReport) -> DesktopIssue {
    match discovery.outcome {
        DiscoveryOutcome::Selected
        | DiscoveryOutcome::ConfirmationRequired
        | DiscoveryOutcome::Ambiguous => desktop_issue("codex_selection_required"),
        DiscoveryOutcome::NotFound => desktop_issue("codex_not_found"),
        DiscoveryOutcome::VerificationFailed => desktop_issue("codex_verification_failed"),
    }
}

fn issue_for_refresh(error: &RefreshOwnerError) -> DesktopIssue {
    match error {
        RefreshOwnerError::Backoff { retry_after_ms } => DesktopIssue {
            retry_after_ms: Some(*retry_after_ms),
            ..desktop_issue("refresh_backoff")
        },
        RefreshOwnerError::Source(RefreshFailure::ProcessFailed) => {
            desktop_issue("app_server_process_failed")
        }
        RefreshOwnerError::Source(RefreshFailure::Timeout) => desktop_issue("app_server_timeout"),
        RefreshOwnerError::Source(RefreshFailure::ProtocolError) => {
            desktop_issue("app_server_protocol_error")
        }
        RefreshOwnerError::Source(RefreshFailure::ExecutableChanged) => {
            desktop_issue("codex_candidate_changed")
        }
        RefreshOwnerError::Source(RefreshFailure::AccountChanged) => {
            desktop_issue("account_changed_during_read")
        }
        RefreshOwnerError::Source(RefreshFailure::AccountBoundaryInvalid) => {
            desktop_issue("account_boundary_invalid")
        }
        RefreshOwnerError::InvalidConfiguration
        | RefreshOwnerError::RuntimeUnavailable
        | RefreshOwnerError::Stopped => desktop_issue("desktop_runtime_failed"),
    }
}

fn desktop_issue(code: &'static str) -> DesktopIssue {
    let message = match code {
        "codex_selection_required" => {
            "Choose one verified Codex installation before reading capacity."
        }
        "codex_not_found" => "No supported Codex executable was found.",
        "codex_verification_failed" => "Codex executable verification failed.",
        "codex_candidate_invalid" => "The selected Codex candidate is no longer available.",
        "codex_candidate_changed" => {
            "The selected Codex executable changed and must be reviewed again."
        }
        "codex_source_failed" => "The verified Codex status source could not start.",
        "app_server_process_failed" => "The Codex app-server process could not complete a read.",
        "app_server_timeout" => "The Codex app-server read timed out.",
        "app_server_protocol_error" => "Codex returned data outside the supported read contract.",
        "account_changed_during_read" => {
            "The Codex account changed during refresh; the previous snapshot was cleared."
        }
        "account_boundary_invalid" => {
            "Codex sent an invalid account-change notification; the previous snapshot was cleared."
        }
        "account_binding_failed" => {
            "The account could not be bound to protected local history; live quota remains available."
        }
        "refresh_backoff" => "Refresh is temporarily delayed by crash-loop protection.",
        "desktop_state_failed" | "desktop_runtime_failed" => {
            "The local desktop status service is unavailable."
        }
        _ => "The local desktop status service could not complete the request.",
    };
    DesktopIssue {
        code,
        message,
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capacity_domain::QuotaWindow;
    use codex_runtime::discovery::{CandidateSource, FileIdentityStrength, HostPlatform};
    use persistence::DesktopHistoryStatus;

    fn fixture_candidate(suffix: &str) -> CodexExecutableCandidate {
        CodexExecutableCandidate {
            executable_id: format!("codex-executable-{suffix}"),
            sources: vec![CandidateSource::MacosChatgptBundle],
            discovered_paths: vec![format!("/Applications/{suffix}.app/codex")],
            canonical_path: format!("/Applications/{suffix}.app/codex"),
            file_identity: format!("fixture-file-id-{suffix}"),
            identity_strength: FileIdentityStrength::OsFileId,
            version: Some("codex-cli 0.0.0-fixture".to_owned()),
            verification: CandidateVerification::Verified,
            requires_confirmation: false,
            reason_codes: Vec::new(),
        }
    }

    fn fixture_status(name: &str) -> StatusSnapshot {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../fixtures/app-server")
                .join(name)
                .join("expected-status.json"),
        )
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn desktop_view_is_an_explicit_path_and_credit_allowlist() {
        let source = fixture_status("plus-normal");
        let view = DesktopStatusView::from(&source);
        let json = serde_json::to_value(&view).unwrap();
        let serialized = serde_json::to_string(&json).unwrap();

        assert!(json.get("quotaWindows").is_some());
        assert!(json.get("dataStatus").is_some());
        assert!(json.get("codexExecutable").is_none());
        assert!(!serialized.contains("canonical_path"));
        assert!(!serialized.contains("file_identity"));
        assert!(!serialized.contains("fixture-credit-1"));
        assert!(!serialized.contains("balance"));
    }

    #[test]
    fn selected_envelope_does_not_send_candidate_paths_to_the_webview() {
        let discovery = DiscoveryReport {
            schema_version: "1.0".to_owned(),
            platform: HostPlatform::Macos,
            architecture: "aarch64".to_owned(),
            outcome: DiscoveryOutcome::Selected,
            selected_executable_id: Some("codex-executable-fixture".to_owned()),
            candidates: vec![fixture_candidate("fixture")],
            diagnostics: Vec::new(),
        };

        let envelope = envelope_from_discovery(Some(discovery), None, None);
        let serialized = serde_json::to_string(&envelope).unwrap();

        assert!(envelope.candidates.is_empty());
        assert_eq!(envelope.selected_executable_source, Some("desktop_app"));
        assert!(!serialized.contains("canonicalPath"));
        assert!(!serialized.contains("/Applications/"));
    }

    #[test]
    fn remembered_explicit_app_path_keeps_its_desktop_label_without_exposing_path() {
        for (path, expected) in [
            (
                "/Applications/ChatGPT.app/Contents/Resources/codex",
                "desktop_app",
            ),
            (
                "/Users/example/Applications/Codex.app/Contents/Resources/codex",
                "desktop_app",
            ),
            (
                "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
                "desktop_app",
            ),
            (
                "/Applications/Codex.app/Contents/Resources/codex-cli/bin/codex",
                "desktop_app",
            ),
            ("/opt/homebrew/bin/codex", "cli"),
            ("/tmp/example.app/tools/codex", "cli"),
        ] {
            let mut candidate = fixture_candidate("fixture");
            candidate.sources.clear(); // Persisted explicit selection loses bundle discovery tags.
            candidate.canonical_path = path.into();
            let discovery = DiscoveryReport {
                schema_version: "1.0".into(),
                platform: HostPlatform::Macos,
                architecture: "aarch64".into(),
                outcome: DiscoveryOutcome::Selected,
                selected_executable_id: Some(candidate.executable_id.clone()),
                candidates: vec![candidate],
                diagnostics: Vec::new(),
            };
            let envelope = envelope_from_discovery(Some(discovery), None, None);
            assert_eq!(envelope.selected_executable_source, Some(expected));
            assert!(!serde_json::to_string(&envelope).unwrap().contains(path));
        }
    }

    #[test]
    fn executable_change_forces_review_of_the_replacement_identity() {
        let discovery = DiscoveryReport {
            schema_version: "1.0".to_owned(),
            platform: HostPlatform::Macos,
            architecture: "aarch64".to_owned(),
            outcome: DiscoveryOutcome::Selected,
            selected_executable_id: Some("codex-executable-upgraded".to_owned()),
            candidates: vec![fixture_candidate("upgraded")],
            diagnostics: Vec::new(),
        };

        let changed = require_confirmation_after_executable_change(discovery);
        let envelope = envelope_from_discovery(
            Some(changed),
            None,
            Some(desktop_issue("codex_candidate_changed")),
        );

        assert_eq!(envelope.lifecycle, DesktopLifecycle::SelectionRequired);
        assert!(envelope.selected_executable_id.is_none());
        assert_eq!(envelope.candidates.len(), 1);
        assert!(envelope.candidates[0].requires_confirmation);
        assert_eq!(
            envelope.issue.as_ref().map(|issue| issue.code),
            Some("codex_candidate_changed")
        );
    }

    #[test]
    fn selection_racing_an_update_returns_a_confirmable_replacement() {
        let old = fixture_candidate("before");
        let mut replacement = fixture_candidate("after");
        // App updates can replace an inode without changing either label.
        replacement.canonical_path = old.canonical_path.clone();
        replacement.version = old.version.clone();
        let report = DiscoveryReport {
            schema_version: "1.0".to_owned(),
            platform: HostPlatform::Macos,
            architecture: "aarch64".to_owned(),
            outcome: DiscoveryOutcome::Selected,
            selected_executable_id: Some(replacement.executable_id.clone()),
            candidates: vec![replacement.clone()],
            diagnostics: Vec::new(),
        };
        let (review, issue) = confirmed_executable_discovery(&old, report.clone());
        assert_eq!(issue.unwrap().code, "codex_candidate_changed");
        assert_eq!(review.outcome, DiscoveryOutcome::ConfirmationRequired);
        assert!(review.selected_executable_id.is_none());
        assert_eq!(
            review.candidates[0].executable_id,
            replacement.executable_id
        );
        assert!(review.candidates[0].requires_confirmation);

        // The following explicit click succeeds against the refreshed identity.
        let (selected, issue) = confirmed_executable_discovery(&review.candidates[0], report);
        assert!(issue.is_none());
        assert_eq!(selected.outcome, DiscoveryOutcome::Selected);
        assert_eq!(
            selected.selected_executable_id,
            Some(replacement.executable_id)
        );
    }

    #[test]
    fn failed_selection_keeps_verification_error_and_never_trusts_the_candidate() {
        let expected = fixture_candidate("fixture");
        let mut failed = expected.clone();
        failed.verification = CandidateVerification::VersionTimeout;
        let report = DiscoveryReport {
            schema_version: "1.0".to_owned(),
            platform: HostPlatform::Macos,
            architecture: "aarch64".to_owned(),
            outcome: DiscoveryOutcome::VerificationFailed,
            selected_executable_id: None,
            candidates: vec![failed],
            diagnostics: Vec::new(),
        };
        let (next, issue) = confirmed_executable_discovery(&expected, report);
        assert_eq!(issue.unwrap().code, "codex_verification_failed");
        assert_eq!(next.outcome, DiscoveryOutcome::VerificationFailed);
        assert!(next.selected_executable_id.is_none());
    }

    #[test]
    fn discovery_failures_are_errors_not_empty_selection_prompts() {
        let discovery = DiscoveryReport {
            schema_version: "1.0".to_owned(),
            platform: HostPlatform::Linux,
            architecture: "x86_64".to_owned(),
            outcome: DiscoveryOutcome::NotFound,
            selected_executable_id: None,
            candidates: Vec::new(),
            diagnostics: Vec::new(),
        };

        let envelope = envelope_from_discovery(
            Some(discovery),
            None,
            Some(desktop_issue("codex_not_found")),
        );

        assert_eq!(envelope.lifecycle, DesktopLifecycle::Error);
        assert!(envelope.candidates.is_empty());
    }

    #[test]
    fn executable_selection_accepts_only_bounded_ids() {
        assert!(valid_executable_id("macos:abc-123_def"));
        assert!(!valid_executable_id(""));
        assert!(!valid_executable_id("../../escape"));
        assert!(!valid_executable_id(&"a".repeat(129)));
        assert!(!valid_executable_id("candidate\nsecret"));
    }

    #[test]
    fn candidate_path_preview_rejects_unbounded_or_control_text() {
        assert_eq!(
            safe_candidate_path("/Applications/ChatGPT.app/codex").as_deref(),
            Some("/Applications/ChatGPT.app/codex")
        );
        assert!(safe_candidate_path("/tmp/codex\nspoofed").is_none());
        assert!(safe_candidate_path(&format!("/{}", "a".repeat(4097))).is_none());
    }

    #[test]
    fn candidate_view_payload_has_a_fixed_item_limit() {
        let candidates = (0..MAX_DESKTOP_CANDIDATES + 4)
            .map(|index| fixture_candidate(&index.to_string()))
            .collect::<Vec<_>>();

        assert_eq!(candidate_views(&candidates).len(), MAX_DESKTOP_CANDIDATES);
    }

    #[test]
    fn refresh_errors_map_to_fixed_safe_messages() {
        for error in [
            RefreshOwnerError::Source(RefreshFailure::ProcessFailed),
            RefreshOwnerError::Source(RefreshFailure::Timeout),
            RefreshOwnerError::Source(RefreshFailure::ProtocolError),
            RefreshOwnerError::Source(RefreshFailure::ExecutableChanged),
            RefreshOwnerError::Source(RefreshFailure::AccountChanged),
            RefreshOwnerError::Source(RefreshFailure::AccountBoundaryInvalid),
            RefreshOwnerError::Backoff {
                retry_after_ms: 500,
            },
        ] {
            let issue = issue_for_refresh(&error);
            assert!(!issue.message.contains('/'));
            assert!(!issue.message.contains("Bearer"));
        }
    }

    #[test]
    fn status_envelopes_receive_a_monotonic_process_sequence() {
        let state = DesktopState::new(DesktopPersistence::unavailable());
        let first = state.stamp_status(envelope_from_discovery(None, None, None));
        let second = state.stamp_status(envelope_from_discovery(None, None, None));

        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert_eq!(first.history_context_id, second.history_context_id);
        state.rotate_ephemeral_binding();
        let switched = state.stamp_status(envelope_from_discovery(None, None, None));
        assert_ne!(first.history_context_id, switched.history_context_id);
    }

    #[test]
    fn work_plan_contract_uses_weekly_window_and_exposes_schedule_baseline() {
        let mut snapshot = fixture_status("plus-normal");
        snapshot.captured_at = UtcTimestamp::parse("2026-01-01T06:00:00Z").unwrap();
        snapshot.quota.windows = vec![QuotaWindow {
            limit_id: "codex:primary".to_owned(),
            label: Some("Weekly".to_owned()),
            window_minutes: Some(1_440),
            used_percent: 28.0,
            remaining_percent: 72.0,
            resets_at: Some(UtcTimestamp::parse("2026-01-02T00:00:00Z").unwrap()),
        }];
        let status = DesktopStatusEnvelope {
            schema_version: DESKTOP_SCHEMA_VERSION,
            sequence: 1,
            history_context_id: None,
            lifecycle: DesktopLifecycle::Ready,
            status: Some(DesktopStatusView::from(&snapshot)),
            candidates: Vec::new(),
            selected_executable_id: Some("fixture".to_owned()),
            selected_executable_source: None,
            issue: None,
            persistence_enabled: true,
        };
        let schedule = DesktopWorkScheduleEnvelope {
            schema_version: "1.0",
            status: DesktopWorkScheduleStatus::Available,
            reason_code: "work_schedule_available",
            schedule: Some(DesktopWorkScheduleView {
                revision: 2,
                enabled: true,
                off_periods: vec![persistence::DesktopWorkSchedulePeriodView {
                    start_minute_of_day: 0,
                    end_minute_of_day: 720,
                }],
                updated_at: "2026-01-01T05:00:00Z".to_owned(),
            }),
        };

        let plan = work_plan_envelope_with_resolver(
            schedule,
            &status,
            &UtcTimestamp::parse("2026-01-01T06:00:00Z").unwrap(),
            Some(
                &DesktopWorkPlanQuotaObservation::from_managed_account_weekly(
                    "2026-01-01T05:59:00Z".to_owned(),
                    10_080,
                    99.0,
                    "2026-01-08T00:00:00Z".to_owned(),
                )
                .unwrap(),
            ),
            |timestamp| u16::try_from(timestamp.hour() * 60 + timestamp.minute()).unwrap(),
        );
        assert_eq!(plan.status, DesktopWorkPlanStatus::Available);
        assert_eq!(plan.reason_code, "work_plan_available");
        let baseline = plan.baseline.as_ref().expect("schedule baseline");
        assert_eq!(baseline.kind, "schedule_baseline");
        assert_eq!(baseline.limit_id, "codex:primary");
        assert_eq!(baseline.source, "app_server_status");
        assert_eq!(baseline.observed_at, "2026-01-01T06:00:00Z");
        assert_eq!(baseline.freshness, Freshness::Live);
        assert_eq!(baseline.comparison.actual_remaining_percent, 72.0);
        assert_eq!(
            baseline.comparison.segment,
            capacity_domain::WorkSegmentState::Off
        );
        assert!((baseline.comparison.expected_remaining_percent - 100.0).abs() < 0.001);
        assert_eq!(
            baseline.comparison.target_at,
            Some(UtcTimestamp::parse("2026-01-01T00:00:00.000Z").unwrap())
        );
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("schedule_baseline"));
        assert!(json.contains("observedAt"));
        assert!(!json.contains("canonical_path"));
    }

    #[test]
    fn work_plan_contract_rejects_an_expired_quota_window() {
        let mut snapshot = fixture_status("plus-normal");
        snapshot.quota.windows = vec![QuotaWindow {
            limit_id: "codex:primary".to_owned(),
            label: Some("Weekly".to_owned()),
            window_minutes: Some(10_080),
            used_percent: 100.0,
            remaining_percent: 0.0,
            resets_at: Some(UtcTimestamp::parse("2026-01-01T00:00:00Z").unwrap()),
        }];
        let status = DesktopStatusEnvelope {
            schema_version: DESKTOP_SCHEMA_VERSION,
            sequence: 1,
            history_context_id: None,
            lifecycle: DesktopLifecycle::Ready,
            status: Some(DesktopStatusView::from(&snapshot)),
            candidates: Vec::new(),
            selected_executable_id: Some("fixture".to_owned()),
            selected_executable_source: None,
            issue: None,
            persistence_enabled: true,
        };
        let schedule = DesktopWorkScheduleEnvelope {
            schema_version: "1.0",
            status: DesktopWorkScheduleStatus::Available,
            reason_code: "work_schedule_available",
            schedule: Some(DesktopWorkScheduleView {
                revision: 1,
                enabled: false,
                off_periods: Vec::new(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
            }),
        };

        let plan = work_plan_envelope_with_resolver(
            schedule,
            &status,
            &UtcTimestamp::parse("2026-01-02T00:00:00Z").unwrap(),
            None,
            |_| 0,
        );
        assert_eq!(plan.status, DesktopWorkPlanStatus::Available);
        assert_eq!(plan.reason_code, "work_plan_window_unavailable");
        assert!(plan.baseline.is_none());
    }

    #[test]
    fn work_plan_prefers_the_canonical_codex_bucket_over_equal_length_model_limits() {
        let mut snapshot = fixture_status("plus-normal");
        snapshot.captured_at = UtcTimestamp::parse("2026-01-01T06:00:00Z").unwrap();
        snapshot.quota.windows = vec![
            QuotaWindow {
                limit_id: "codex:primary".to_owned(),
                label: Some("Weekly".to_owned()),
                window_minutes: Some(10_080),
                used_percent: 68.0,
                remaining_percent: 32.0,
                resets_at: Some(UtcTimestamp::parse("2026-01-08T00:00:00Z").unwrap()),
            },
            QuotaWindow {
                limit_id: "codex_bengalfox:secondary".to_owned(),
                label: Some("Weekly".to_owned()),
                window_minutes: Some(10_080),
                used_percent: 0.0,
                remaining_percent: 100.0,
                resets_at: Some(UtcTimestamp::parse("2026-01-09T00:00:00Z").unwrap()),
            },
        ];
        let status = DesktopStatusEnvelope {
            schema_version: DESKTOP_SCHEMA_VERSION,
            sequence: 1,
            history_context_id: None,
            lifecycle: DesktopLifecycle::Ready,
            status: Some(DesktopStatusView::from(&snapshot)),
            candidates: Vec::new(),
            selected_executable_id: Some("fixture".to_owned()),
            selected_executable_source: None,
            issue: None,
            persistence_enabled: true,
        };
        let schedule = DesktopWorkScheduleEnvelope {
            schema_version: "1.0",
            status: DesktopWorkScheduleStatus::Available,
            reason_code: "work_schedule_available",
            schedule: Some(DesktopWorkScheduleView {
                revision: 1,
                enabled: false,
                off_periods: Vec::new(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
            }),
        };

        let plan = work_plan_envelope_with_resolver(
            schedule,
            &status,
            &UtcTimestamp::parse("2026-01-01T06:00:00Z").unwrap(),
            None,
            |_| 360,
        );

        let baseline = plan.baseline.expect("canonical Codex baseline");
        assert_eq!(baseline.limit_id, "codex:primary");
        assert_eq!(baseline.comparison.actual_remaining_percent, 32.0);
    }

    #[test]
    fn work_plan_uses_a_validated_managed_account_window_when_live_status_is_empty() {
        let mut snapshot = fixture_status("plus-normal");
        snapshot.captured_at = UtcTimestamp::parse("2026-01-01T06:00:00Z").unwrap();
        snapshot.quota.windows.clear();
        let status = DesktopStatusEnvelope {
            schema_version: DESKTOP_SCHEMA_VERSION,
            sequence: 1,
            history_context_id: None,
            lifecycle: DesktopLifecycle::Stale,
            status: Some(DesktopStatusView::from(&snapshot)),
            candidates: Vec::new(),
            selected_executable_id: Some("fixture".to_owned()),
            selected_executable_source: None,
            issue: None,
            persistence_enabled: true,
        };
        let schedule = DesktopWorkScheduleEnvelope {
            schema_version: "1.0",
            status: DesktopWorkScheduleStatus::Available,
            reason_code: "work_schedule_available",
            schedule: Some(DesktopWorkScheduleView {
                revision: 1,
                enabled: true,
                off_periods: Vec::new(),
                updated_at: "2026-01-01T05:00:00Z".to_owned(),
            }),
        };
        let observation = DesktopWorkPlanQuotaObservation::from_managed_account_weekly(
            "2026-01-01T05:59:00Z".to_owned(),
            10_080,
            32.0,
            "2026-01-08T00:00:00Z".to_owned(),
        )
        .unwrap();

        let plan = work_plan_envelope_with_resolver(
            schedule,
            &status,
            &UtcTimestamp::parse("2026-01-01T06:00:00Z").unwrap(),
            Some(&observation),
            |_| 360,
        );

        assert_eq!(plan.reason_code, "work_plan_account_cache");
        let baseline = plan.baseline.expect("managed account fallback baseline");
        assert_eq!(baseline.limit_id, "codex:managed-account-weekly");
        assert_eq!(baseline.source, "managed_account_cache");
        assert_eq!(baseline.freshness, Freshness::Stale);
        assert_eq!(baseline.comparison.actual_remaining_percent, 32.0);
    }

    #[test]
    fn managed_account_fallback_rejects_short_or_malformed_observations() {
        assert!(
            DesktopWorkPlanQuotaObservation::from_managed_account_weekly(
                "2026-01-01T00:00:00Z".to_owned(),
                300,
                50.0,
                "2026-01-02T00:00:00Z".to_owned(),
            )
            .is_none()
        );
        assert!(
            DesktopWorkPlanQuotaObservation::from_managed_account_weekly(
                "not-a-timestamp".to_owned(),
                10_080,
                101.0,
                "2026-01-08T00:00:00Z".to_owned(),
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn unavailable_history_preserves_live_quota_until_the_account_changes() {
        struct FixtureSource;
        impl codex_runtime::refresh::RefreshSource for FixtureSource {
            fn refresh(&mut self) -> codex_runtime::refresh::RefreshFuture<'_> {
                Box::pin(async { Ok(fixture_status("plus-normal")) })
            }
            fn disconnect(&mut self) -> codex_runtime::refresh::DisconnectFuture<'_> {
                Box::pin(async { Ok(()) })
            }
        }
        let state = DesktopState::new_with_account_binding_store(
            DesktopPersistence::in_memory_ephemeral(),
            DesktopAccountBindingStore::unavailable(),
        );
        let identity =
            || DesktopAccountIdentity::from_account_digest("0123456789abcdef01234567").unwrap();
        assert!(state.bind_account_identity(identity()).await.is_err());
        assert_eq!(
            state
                .history("codex:primary".into(), None)
                .await
                .reason_code,
            "history_keychain_unavailable"
        );
        let handle = RefreshHandle::start(FixtureSource, RefreshOwnerConfig::default()).unwrap();
        handle.refresh().await.unwrap();
        *state.owner.lock().await = Some(OwnerBinding {
            executable_id: "fixture".to_owned(),
            handle: handle.clone(),
        });

        assert!(state.bind_account_identity(identity()).await.is_err());
        assert!(state.owner.lock().await.is_some());
        assert_eq!(
            handle
                .latest()
                .await
                .unwrap()
                .unwrap()
                .snapshot
                .data_status
                .freshness,
            Freshness::Live
        );
        assert!(!state.persistence.lock().unwrap().has_stable_binding());

        let other =
            DesktopAccountIdentity::from_account_digest("89abcdef0123456789abcdef").unwrap();
        assert!(state.bind_account_identity(other).await.is_err());
        assert!(state.owner.lock().await.is_none());
        assert!(handle.latest().await.is_err());
        state.shutdown().await;
    }

    #[tokio::test]
    async fn formal_identity_binding_unlocks_account_separated_history() {
        let directory = tempfile::tempdir().expect("temporary identity root");
        let binding_store = DesktopAccountBindingStore::explicit_file_fallback(
            &directory.path().join("installation-key"),
        )
        .expect("explicit test identity backend");
        let state = DesktopState::new_with_account_binding_store(
            DesktopPersistence::in_memory_ephemeral(),
            binding_store,
        );
        let identity = DesktopAccountIdentity::from_account_digest("0123456789abcdef01234567")
            .expect("formal account digest");
        assert!(state.bind_account_identity(identity).await.unwrap());

        let mut refresh = codex_runtime::refresh::RefreshSnapshot {
            generation: 1,
            provenance: codex_runtime::refresh::RefreshProvenance::CurrentRead,
            failure: None,
            snapshot: fixture_status("plus-normal"),
        };
        state.apply_stable_account_binding(&mut refresh);
        assert_eq!(
            refresh
                .snapshot
                .account
                .as_ref()
                .expect("fixture account")
                .binding_status,
            AccountBindingStatus::Stable
        );
        assert_eq!(
            state.persist_snapshot(&refresh.snapshot).await,
            PersistenceRecordResult::Persisted
        );
        let history = state.history("codex:primary".to_owned(), None).await;
        assert_eq!(history.status, persistence::DesktopHistoryStatus::Available);
        assert_eq!(history.points.len(), 1);
    }

    #[tokio::test]
    async fn executable_review_preserves_history_but_account_changes_still_clear_it() {
        let directory = tempfile::tempdir().unwrap();
        let keys =
            DesktopAccountBindingStore::explicit_file_fallback(&directory.path().join("test-key"))
                .unwrap();
        let candidate = fixture_candidate("0000000000000001");
        let report = DiscoveryReport {
            schema_version: "1.0".into(),
            platform: HostPlatform::Macos,
            architecture: "aarch64".into(),
            outcome: DiscoveryOutcome::Selected,
            selected_executable_id: Some(candidate.executable_id.clone()),
            candidates: vec![candidate],
            diagnostics: Vec::new(),
        };
        let state = DesktopState::new_with_account_binding_store_and_discovery(
            DesktopPersistence::in_memory_ephemeral(),
            keys,
            report.clone(),
        );
        let identity =
            DesktopAccountIdentity::from_account_digest("0123456789abcdef01234567").unwrap();
        state.bind_account_identity(identity).await.unwrap();
        let mut status = fixture_status("plus-normal");
        status.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
        status.environment.environment_id = "macos:local:codex-executable-0000000000000001".into();
        status.environment.platform = "macos".into();
        status.environment.architecture = "aarch64".into();
        status.environment.boundary = "native".into();
        assert_eq!(
            state.persist_snapshot(&status).await,
            PersistenceRecordResult::Persisted
        );
        let before = state.history("codex:primary".into(), None).await;
        assert_eq!(before.points.len(), 1);

        state
            .replace_executable_discovery(require_confirmation_after_executable_change(report))
            .await
            .unwrap();
        let reviewing = state.history("codex:primary".into(), None).await;
        assert_eq!(reviewing.history_context_id, before.history_context_id);
        assert_eq!(reviewing.points.len(), 1);
        assert!(state.persistence.lock().unwrap().has_stable_binding());
        assert!(state.owner.lock().await.is_none());

        status.environment.environment_id = "macos:local:codex-executable-0000000000000002".into();
        status.captured_at = UtcTimestamp::parse("2026-08-30T02:56:53Z").unwrap();
        assert_eq!(
            state.persist_snapshot(&status).await,
            PersistenceRecordResult::Persisted
        );
        let upgraded = state.history("codex:primary".into(), None).await;
        assert_eq!(upgraded.points.len(), 2);
        assert_ne!(upgraded.history_context_id, before.history_context_id);

        let other =
            DesktopAccountIdentity::from_account_digest("89abcdef0123456789abcdef").unwrap();
        state.bind_account_identity(other).await.unwrap();
        let switched = state.history("codex:primary".into(), None).await;
        assert!(switched.points.is_empty());
        assert_ne!(switched.history_context_id, upgraded.history_context_id);
        state.shutdown().await;
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires the local ChatGPT.app bundled Codex and a signed-in account"]
    async fn bundled_desktop_ipc_live_smoke() {
        let state = DesktopState::new_with_explicit_executable(
            DesktopPersistence::in_memory_ephemeral(),
            Some(
                bundled_codex_path(std::path::Path::new("/Applications/ChatGPT.app"))
                    .expect("installed bundled reader"),
            ),
        );
        let discovery = state.discovery_snapshot().unwrap();
        let candidate = discovery
            .selected_candidate()
            .expect("ChatGPT.app bundled Codex was not selected by the formal host");
        let executable_id = candidate.executable_id.clone();

        let envelope = refresh_status_inner(&state).await;
        let schedule = state.work_schedule().await;
        let work_plan = work_plan_envelope(schedule, &envelope, &SystemClock.now(), None);
        state.shutdown().await;

        assert_eq!(envelope.lifecycle, DesktopLifecycle::Ready);
        assert_eq!(
            envelope.selected_executable_id.as_deref(),
            Some(executable_id.as_str())
        );
        assert!(envelope.candidates.is_empty());
        let status = envelope.status.expect("live desktop status is missing");
        assert_eq!(status.data_status.availability, Availability::Complete);
        assert_eq!(status.data_status.freshness, Freshness::Live);
        assert!(!status.quota_windows.is_empty());
        assert_eq!(
            status.reset_credits.summary_status,
            SummaryStatus::Available
        );
        assert!(status.reset_credits.available_count.is_some());
        let canonical_window = status
            .quota_windows
            .iter()
            .filter(|window| canonical_codex_limit(&window.limit_id))
            .max_by_key(|window| window.window_minutes)
            .expect("live canonical Codex quota window is missing");
        assert_eq!(work_plan.status, DesktopWorkPlanStatus::Available);
        assert_eq!(work_plan.reason_code, "work_plan_available");
        let baseline = work_plan.baseline.expect("live work plan baseline");
        assert_eq!(baseline.limit_id, canonical_window.limit_id);
        assert_eq!(
            baseline.comparison.actual_remaining_percent,
            canonical_window.remaining_percent
        );
        let history = state.history("codex:primary".to_owned(), None).await;
        assert_eq!(history.status, DesktopHistoryStatus::BindingRequired);
        assert_eq!(history.reason_code, "history_ephemeral_binding");
        assert!(history.points.is_empty());
    }
}
