use std::collections::BTreeSet;

use capacity_domain::{Availability, Compatibility, Freshness, STATUS_SCHEMA_VERSION};
use codex_runtime::discovery::{CandidateSource, DiscoveryReport, HostPlatform};
use codex_runtime::refresh::{
    RefreshFailure, RefreshOwnerError, RefreshProvenance, RefreshShutdownReport, RefreshSnapshot,
    RefreshSourceDiagnostics,
};
use serde::Serialize;

pub const SOAK_SCHEMA_VERSION: &str = "2.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SoakOutcome {
    Pass,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SoakFailureCategory {
    ProcessFailed,
    Timeout,
    ProtocolError,
    ExecutableChanged,
    AccountChanged,
    AccountBoundaryInvalid,
    Backoff,
    OwnerStopped,
    JoinError,
    DiagnosticIncomplete,
    Mixed,
}

impl SoakFailureCategory {
    fn from_refresh(failure: RefreshFailure) -> Self {
        match failure {
            RefreshFailure::ProcessFailed => Self::ProcessFailed,
            RefreshFailure::Timeout => Self::Timeout,
            RefreshFailure::ProtocolError => Self::ProtocolError,
            RefreshFailure::ExecutableChanged => Self::ExecutableChanged,
            RefreshFailure::AccountChanged => Self::AccountChanged,
            RefreshFailure::AccountBoundaryInvalid => Self::AccountBoundaryInvalid,
        }
    }

    fn from_owner(error: &RefreshOwnerError) -> Self {
        match error {
            RefreshOwnerError::Source(failure) => Self::from_refresh(*failure),
            RefreshOwnerError::Backoff { .. } => Self::Backoff,
            RefreshOwnerError::Stopped => Self::OwnerStopped,
            RefreshOwnerError::InvalidConfiguration | RefreshOwnerError::RuntimeUnavailable => {
                Self::DiagnosticIncomplete
            }
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct SoakFailureRoundCounts {
    pub process_failed: u64,
    pub timeout: u64,
    pub protocol_error: u64,
    pub executable_changed: u64,
    pub account_changed: u64,
    pub account_boundary_invalid: u64,
    pub backoff: u64,
    pub owner_stopped: u64,
    pub join_error: u64,
    pub diagnostic_incomplete: u64,
    pub mixed: u64,
}

impl SoakFailureRoundCounts {
    fn increment(&mut self, category: SoakFailureCategory) {
        let count = match category {
            SoakFailureCategory::ProcessFailed => &mut self.process_failed,
            SoakFailureCategory::Timeout => &mut self.timeout,
            SoakFailureCategory::ProtocolError => &mut self.protocol_error,
            SoakFailureCategory::ExecutableChanged => &mut self.executable_changed,
            SoakFailureCategory::AccountChanged => &mut self.account_changed,
            SoakFailureCategory::AccountBoundaryInvalid => &mut self.account_boundary_invalid,
            SoakFailureCategory::Backoff => &mut self.backoff,
            SoakFailureCategory::OwnerStopped => &mut self.owner_stopped,
            SoakFailureCategory::JoinError => &mut self.join_error,
            SoakFailureCategory::DiagnosticIncomplete => &mut self.diagnostic_incomplete,
            SoakFailureCategory::Mixed => &mut self.mixed,
        };
        *count = count.saturating_add(1);
    }

    pub fn total(&self) -> u64 {
        self.process_failed
            .saturating_add(self.timeout)
            .saturating_add(self.protocol_error)
            .saturating_add(self.executable_changed)
            .saturating_add(self.account_changed)
            .saturating_add(self.account_boundary_invalid)
            .saturating_add(self.backoff)
            .saturating_add(self.owner_stopped)
            .saturating_add(self.join_error)
            .saturating_add(self.diagnostic_incomplete)
            .saturating_add(self.mixed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SoakArchitecture {
    Aarch64,
    X86_64,
    X86,
    Arm,
    Other,
}

impl From<&str> for SoakArchitecture {
    fn from(value: &str) -> Self {
        match value {
            "aarch64" => Self::Aarch64,
            "x86_64" => Self::X86_64,
            "x86" => Self::X86,
            "arm" => Self::Arm,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoakExecutableProvenance {
    pub platform: HostPlatform,
    pub architecture: SoakArchitecture,
    pub candidate_source: CandidateSource,
    pub codex_version: Option<String>,
    pub status_schema_version: String,
}

impl SoakExecutableProvenance {
    pub fn from_discovery(discovery: &DiscoveryReport) -> Option<Self> {
        let candidate = discovery.selected_candidate()?;
        let candidate_source = candidate.sources.first().copied()?;
        Some(Self {
            platform: discovery.platform,
            architecture: SoakArchitecture::from(discovery.architecture.as_str()),
            candidate_source,
            codex_version: candidate.version.clone(),
            status_schema_version: STATUS_SCHEMA_VERSION.to_owned(),
        })
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct SoakSourceLifecycle {
    pub refresh_attempts: u64,
    pub identity_verification_failures: u64,
    pub connection_start_attempts: u64,
    pub connections_started: u64,
    pub connection_start_failures: u64,
    pub disconnect_attempts: u64,
    pub disconnects_completed: u64,
    pub disconnect_failures: u64,
    pub connection_handle_active: bool,
}

impl From<RefreshSourceDiagnostics> for SoakSourceLifecycle {
    fn from(value: RefreshSourceDiagnostics) -> Self {
        Self {
            refresh_attempts: value.refresh_attempts,
            identity_verification_failures: value.identity_verification_failures,
            connection_start_attempts: value.connection_start_attempts,
            connections_started: value.connections_started,
            connection_start_failures: value.connection_start_failures,
            disconnect_attempts: value.disconnect_attempts,
            disconnects_completed: value.disconnects_completed,
            disconnect_failures: value.disconnect_failures,
            connection_handle_active: value.connection_handle_active,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SoakReportV2 {
    pub schema_version: &'static str,
    pub outcome: SoakOutcome,
    pub requested_duration_seconds: u64,
    pub elapsed_milliseconds: u64,
    pub interval_seconds: u64,
    pub concurrent_requests: u64,
    pub scheduled_rounds: u64,
    pub successful_rounds: u64,
    pub failed_rounds: u64,
    pub failure_round_counts: SoakFailureRoundCounts,
    pub maximum_consecutive_failed_rounds: u64,
    pub first_failure_elapsed_seconds: Option<u64>,
    pub last_failure_elapsed_seconds: Option<u64>,
    pub request_responses: u64,
    pub source_generations: u64,
    pub coalesced_responses: u64,
    pub current_responses: u64,
    pub stale_responses: u64,
    pub error_responses: u64,
    pub maximum_round_latency_ms: u64,
    pub final_availability: Option<Availability>,
    pub final_freshness: Option<Freshness>,
    pub final_compatibility: Option<Compatibility>,
    pub final_window_count: Option<usize>,
    pub shutdown_clean: bool,
    pub shutdown_failure: Option<SoakFailureCategory>,
    pub diagnostics_complete: bool,
    pub source_lifecycle: SoakSourceLifecycle,
    pub executable: SoakExecutableProvenance,
    pub path_redacted: bool,
}

impl SoakReportV2 {
    pub fn passed(&self) -> bool {
        self.outcome == SoakOutcome::Pass
    }
}

#[derive(Debug)]
pub enum SoakRequestResult {
    Snapshot {
        generation: u64,
        provenance: RefreshProvenance,
        failure: Option<RefreshFailure>,
    },
    OwnerError(RefreshOwnerError),
    JoinError,
}

impl From<RefreshSnapshot> for SoakRequestResult {
    fn from(snapshot: RefreshSnapshot) -> Self {
        Self::Snapshot {
            generation: snapshot.generation,
            provenance: snapshot.provenance,
            failure: snapshot.failure,
        }
    }
}

#[derive(Debug)]
pub struct SoakAccumulator {
    requested_duration_seconds: u64,
    interval_seconds: u64,
    concurrent_requests: u64,
    scheduled_rounds: u64,
    successful_rounds: u64,
    failed_rounds: u64,
    failure_round_counts: SoakFailureRoundCounts,
    current_consecutive_failed_rounds: u64,
    maximum_consecutive_failed_rounds: u64,
    first_failure_elapsed_seconds: Option<u64>,
    last_failure_elapsed_seconds: Option<u64>,
    request_responses: u64,
    generations: BTreeSet<u64>,
    current_responses: u64,
    stale_responses: u64,
    error_responses: u64,
    maximum_round_latency_ms: u64,
}

impl SoakAccumulator {
    pub fn new(
        requested_duration_seconds: u64,
        interval_seconds: u64,
        concurrent_requests: u64,
    ) -> Self {
        Self {
            requested_duration_seconds,
            interval_seconds,
            concurrent_requests,
            scheduled_rounds: 0,
            successful_rounds: 0,
            failed_rounds: 0,
            failure_round_counts: SoakFailureRoundCounts::default(),
            current_consecutive_failed_rounds: 0,
            maximum_consecutive_failed_rounds: 0,
            first_failure_elapsed_seconds: None,
            last_failure_elapsed_seconds: None,
            request_responses: 0,
            generations: BTreeSet::new(),
            current_responses: 0,
            stale_responses: 0,
            error_responses: 0,
            maximum_round_latency_ms: 0,
        }
    }

    pub fn observe_round(
        &mut self,
        elapsed_seconds: u64,
        round_latency_ms: u64,
        responses: impl IntoIterator<Item = SoakRequestResult>,
    ) {
        self.scheduled_rounds = self.scheduled_rounds.saturating_add(1);
        self.maximum_round_latency_ms = self.maximum_round_latency_ms.max(round_latency_ms);
        let mut categories = BTreeSet::new();
        let mut response_count = 0_u64;

        for response in responses {
            response_count = response_count.saturating_add(1);
            self.request_responses = self.request_responses.saturating_add(1);
            match response {
                SoakRequestResult::Snapshot {
                    generation,
                    provenance,
                    failure,
                } => {
                    if generation == 0 {
                        categories.insert(SoakFailureCategory::DiagnosticIncomplete);
                    }
                    self.generations.insert(generation);
                    match provenance {
                        RefreshProvenance::CurrentRead => {
                            self.current_responses = self.current_responses.saturating_add(1);
                            if failure.is_some() {
                                categories.insert(SoakFailureCategory::DiagnosticIncomplete);
                            }
                        }
                        RefreshProvenance::StaleFallback => {
                            self.stale_responses = self.stale_responses.saturating_add(1);
                            categories.insert(failure.map_or(
                                SoakFailureCategory::DiagnosticIncomplete,
                                SoakFailureCategory::from_refresh,
                            ));
                        }
                    }
                }
                SoakRequestResult::OwnerError(error) => {
                    self.error_responses = self.error_responses.saturating_add(1);
                    categories.insert(SoakFailureCategory::from_owner(&error));
                }
                SoakRequestResult::JoinError => {
                    self.error_responses = self.error_responses.saturating_add(1);
                    categories.insert(SoakFailureCategory::JoinError);
                }
            }
        }

        if response_count != self.concurrent_requests {
            categories.insert(SoakFailureCategory::DiagnosticIncomplete);
        }

        if categories.is_empty() {
            self.successful_rounds = self.successful_rounds.saturating_add(1);
            self.current_consecutive_failed_rounds = 0;
            return;
        }

        self.failed_rounds = self.failed_rounds.saturating_add(1);
        self.current_consecutive_failed_rounds =
            self.current_consecutive_failed_rounds.saturating_add(1);
        self.maximum_consecutive_failed_rounds = self
            .maximum_consecutive_failed_rounds
            .max(self.current_consecutive_failed_rounds);
        self.first_failure_elapsed_seconds
            .get_or_insert(elapsed_seconds);
        self.last_failure_elapsed_seconds = Some(elapsed_seconds);
        let category = if categories.len() == 1 {
            *categories
                .first()
                .expect("one failure category was observed")
        } else {
            SoakFailureCategory::Mixed
        };
        self.failure_round_counts.increment(category);
    }

    pub fn finish(
        self,
        elapsed_milliseconds: u64,
        latest: Option<&RefreshSnapshot>,
        shutdown: Result<RefreshShutdownReport, RefreshOwnerError>,
        executable: SoakExecutableProvenance,
    ) -> SoakReportV2 {
        let source_generations = u64::try_from(self.generations.len()).unwrap_or(u64::MAX);
        let snapshot_responses = self.current_responses.saturating_add(self.stale_responses);
        let coalesced_responses = snapshot_responses.saturating_sub(source_generations);
        let (shutdown_clean, shutdown_failure, source_lifecycle, shutdown_report_received) =
            match shutdown {
                Ok(report) => {
                    let clean = report.is_clean();
                    let failure = report
                        .failure
                        .map(SoakFailureCategory::from_refresh)
                        .or_else(|| (!clean).then_some(SoakFailureCategory::DiagnosticIncomplete));
                    (clean, failure, report.source.into(), true)
                }
                Err(error) => (
                    false,
                    Some(SoakFailureCategory::from_owner(&error)),
                    SoakSourceLifecycle::default(),
                    false,
                ),
            };
        let diagnostics_complete = shutdown_report_received
            && lifecycle_is_complete(&source_lifecycle, source_generations, snapshot_responses)
            && self.failure_round_counts.total() == self.failed_rounds
            && self.successful_rounds.saturating_add(self.failed_rounds) == self.scheduled_rounds;
        let pass = self.scheduled_rounds > 0
            && self.current_responses > 0
            && self.stale_responses == 0
            && self.error_responses == 0
            && self.failed_rounds == 0
            && shutdown_clean
            && shutdown_failure.is_none()
            && diagnostics_complete
            && source_lifecycle.identity_verification_failures == 0
            && source_lifecycle.connection_start_failures == 0
            && source_lifecycle.disconnect_failures == 0;

        SoakReportV2 {
            schema_version: SOAK_SCHEMA_VERSION,
            outcome: if pass {
                SoakOutcome::Pass
            } else {
                SoakOutcome::Failed
            },
            requested_duration_seconds: self.requested_duration_seconds,
            elapsed_milliseconds,
            interval_seconds: self.interval_seconds,
            concurrent_requests: self.concurrent_requests,
            scheduled_rounds: self.scheduled_rounds,
            successful_rounds: self.successful_rounds,
            failed_rounds: self.failed_rounds,
            failure_round_counts: self.failure_round_counts,
            maximum_consecutive_failed_rounds: self.maximum_consecutive_failed_rounds,
            first_failure_elapsed_seconds: self.first_failure_elapsed_seconds,
            last_failure_elapsed_seconds: self.last_failure_elapsed_seconds,
            request_responses: self.request_responses,
            source_generations,
            coalesced_responses,
            current_responses: self.current_responses,
            stale_responses: self.stale_responses,
            error_responses: self.error_responses,
            maximum_round_latency_ms: self.maximum_round_latency_ms,
            final_availability: latest.map(|value| value.snapshot.data_status.availability),
            final_freshness: latest.map(|value| value.snapshot.data_status.freshness),
            final_compatibility: latest.map(|value| value.snapshot.data_status.compatibility),
            final_window_count: latest.map(|value| value.snapshot.quota.windows.len()),
            shutdown_clean,
            shutdown_failure,
            diagnostics_complete,
            source_lifecycle,
            executable,
            path_redacted: true,
        }
    }
}

fn lifecycle_is_complete(
    lifecycle: &SoakSourceLifecycle,
    source_generations: u64,
    snapshot_responses: u64,
) -> bool {
    lifecycle.refresh_attempts > 0
        && source_generations <= snapshot_responses
        && source_generations <= lifecycle.refresh_attempts
        && lifecycle.connection_start_attempts
            == lifecycle
                .connections_started
                .saturating_add(lifecycle.connection_start_failures)
        && lifecycle.disconnect_attempts
            == lifecycle
                .disconnects_completed
                .saturating_add(lifecycle.disconnect_failures)
        && lifecycle
            .identity_verification_failures
            .saturating_add(lifecycle.connection_start_attempts)
            <= lifecycle.refresh_attempts
        && lifecycle.connections_started == lifecycle.disconnect_attempts
        && !lifecycle.connection_handle_active
}

#[cfg(test)]
mod tests {
    use codex_runtime::discovery::{
        CandidateVerification, CodexExecutableCandidate, DiscoveryOutcome, FileIdentityStrength,
    };

    use super::*;

    fn snapshot(
        generation: u64,
        provenance: RefreshProvenance,
        failure: Option<RefreshFailure>,
    ) -> RefreshSnapshot {
        RefreshSnapshot {
            generation,
            provenance,
            failure,
            snapshot: crate::load_fixture_status("plus-normal").unwrap(),
        }
    }

    fn executable() -> SoakExecutableProvenance {
        SoakExecutableProvenance {
            platform: HostPlatform::Macos,
            architecture: SoakArchitecture::Aarch64,
            candidate_source: CandidateSource::Explicit,
            codex_version: Some("codex-cli 0.151.0-alpha.7.1".to_owned()),
            status_schema_version: STATUS_SCHEMA_VERSION.to_owned(),
        }
    }

    fn clean_shutdown(refresh_attempts: u64, connections: u64) -> RefreshShutdownReport {
        RefreshShutdownReport {
            failure: None,
            source: RefreshSourceDiagnostics {
                refresh_attempts,
                connection_start_attempts: connections,
                connections_started: connections,
                disconnect_attempts: connections,
                disconnects_completed: connections,
                ..RefreshSourceDiagnostics::default()
            },
        }
    }

    #[test]
    fn clean_coalesced_round_builds_a_passing_report() {
        let mut accumulator = SoakAccumulator::new(60, 5, 4);
        accumulator.observe_round(
            1,
            20,
            (0..4).map(|_| snapshot(1, RefreshProvenance::CurrentRead, None).into()),
        );
        let latest = snapshot(1, RefreshProvenance::CurrentRead, None);
        let report =
            accumulator.finish(1_000, Some(&latest), Ok(clean_shutdown(1, 1)), executable());

        assert!(report.passed());
        assert_eq!(report.scheduled_rounds, 1);
        assert_eq!(report.successful_rounds, 1);
        assert_eq!(report.source_generations, 1);
        assert_eq!(report.coalesced_responses, 3);
        assert!(report.diagnostics_complete);
    }

    #[test]
    fn stale_and_backoff_rounds_retain_category_and_timing() {
        let mut accumulator = SoakAccumulator::new(60, 5, 4);
        accumulator.observe_round(
            0,
            10,
            (0..4).map(|_| snapshot(1, RefreshProvenance::CurrentRead, None).into()),
        );
        accumulator.observe_round(
            5,
            40,
            (0..4).map(|_| {
                snapshot(
                    2,
                    RefreshProvenance::StaleFallback,
                    Some(RefreshFailure::Timeout),
                )
                .into()
            }),
        );
        accumulator.observe_round(
            10,
            2,
            (0..4).map(|_| {
                SoakRequestResult::OwnerError(RefreshOwnerError::Backoff {
                    retry_after_ms: 100,
                })
            }),
        );
        let latest = snapshot(
            2,
            RefreshProvenance::StaleFallback,
            Some(RefreshFailure::Timeout),
        );
        let report = accumulator.finish(
            10_000,
            Some(&latest),
            Ok(clean_shutdown(2, 1)),
            executable(),
        );

        assert!(!report.passed());
        assert_eq!(report.failed_rounds, 2);
        assert_eq!(report.failure_round_counts.timeout, 1);
        assert_eq!(report.failure_round_counts.backoff, 1);
        assert_eq!(report.maximum_consecutive_failed_rounds, 2);
        assert_eq!(report.first_failure_elapsed_seconds, Some(5));
        assert_eq!(report.last_failure_elapsed_seconds, Some(10));
        assert_eq!(report.current_responses, 4);
        assert_eq!(report.stale_responses, 4);
        assert_eq!(report.error_responses, 4);
    }

    #[test]
    fn multiple_failure_kinds_in_one_round_are_classified_as_mixed() {
        let mut accumulator = SoakAccumulator::new(5, 5, 2);
        accumulator.observe_round(
            0,
            5,
            [
                snapshot(
                    1,
                    RefreshProvenance::StaleFallback,
                    Some(RefreshFailure::Timeout),
                )
                .into(),
                SoakRequestResult::OwnerError(RefreshOwnerError::Backoff { retry_after_ms: 10 }),
            ],
        );
        let report = accumulator.finish(5, None, Ok(clean_shutdown(1, 1)), executable());

        assert_eq!(report.failure_round_counts.mixed, 1);
        assert_eq!(report.failure_round_counts.total(), 1);
    }

    #[test]
    fn missing_stale_reason_is_diagnostic_incomplete() {
        let mut accumulator = SoakAccumulator::new(5, 5, 1);
        accumulator.observe_round(
            0,
            1,
            [snapshot(1, RefreshProvenance::StaleFallback, None).into()],
        );
        let report = accumulator.finish(1, None, Ok(clean_shutdown(1, 1)), executable());

        assert_eq!(report.failure_round_counts.diagnostic_incomplete, 1);
        assert!(!report.passed());
    }

    #[test]
    fn provenance_discards_paths_and_identifiers_from_serialized_report() {
        let discovery = DiscoveryReport {
            schema_version: "1.0".to_owned(),
            platform: HostPlatform::Macos,
            architecture: "aarch64".to_owned(),
            outcome: DiscoveryOutcome::Selected,
            selected_executable_id: Some("secret@example.com".to_owned()),
            candidates: vec![CodexExecutableCandidate {
                executable_id: "secret@example.com".to_owned(),
                sources: vec![CandidateSource::Explicit],
                discovered_paths: vec!["/Users/private/account/auth.json".to_owned()],
                canonical_path: "/Applications/Private Codex.app/codex".to_owned(),
                file_identity: "private-file-identity".to_owned(),
                identity_strength: FileIdentityStrength::OsFileId,
                version: Some("codex-cli 0.151.0-alpha.7.1".to_owned()),
                verification: CandidateVerification::Verified,
                requires_confirmation: false,
                reason_codes: Vec::new(),
            }],
            diagnostics: Vec::new(),
        };
        let provenance = SoakExecutableProvenance::from_discovery(&discovery).unwrap();
        let mut accumulator = SoakAccumulator::new(5, 5, 1);
        accumulator.observe_round(
            0,
            1,
            [snapshot(1, RefreshProvenance::CurrentRead, None).into()],
        );
        let report = accumulator.finish(1, None, Ok(clean_shutdown(1, 1)), provenance);
        let json = serde_json::to_string(&report).unwrap();

        for forbidden in [
            "secret@example.com",
            "/Users/private",
            "/Applications/Private",
            "private-file-identity",
            "canonical_path",
            "executable_id",
            "selected_executable_id",
            "remaining_percent",
            "reset_at",
        ] {
            assert!(!json.contains(forbidden), "leaked {forbidden}");
        }
        assert!(report.path_redacted);
    }
}
