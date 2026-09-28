use capacity_domain::{
    Availability, DataStatus, Diagnostic, DiagnosticSeverity, Freshness, ReasonCode,
    StatusSnapshot, SummaryStatus,
};
use codex_runtime::app_server::RuntimeError;
use codex_runtime::discovery::{
    CandidateSource, DiscoveryOutcome, DiscoveryReport, FileIdentityStrength,
};
use serde::Serialize;

use super::LiveReadFailure;

const DOCTOR_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DoctorOutcome {
    Ready,
    Partial,
    ExecutableError,
    AuthenticationRequired,
    Unsupported,
    Timeout,
    ProtocolError,
}

impl DoctorOutcome {
    pub(super) const fn exit_code(self) -> u8 {
        match self {
            Self::Ready | Self::Partial => 0,
            Self::Unsupported => 2,
            Self::AuthenticationRequired => 3,
            Self::ExecutableError => 4,
            Self::Timeout | Self::ProtocolError => 5,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Partial => "partial",
            Self::ExecutableError => "executable_error",
            Self::AuthenticationRequired => "authentication_required",
            Self::Unsupported => "unsupported",
            Self::Timeout => "timeout",
            Self::ProtocolError => "protocol_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DoctorCheckCode {
    ExecutableDiscovery,
    VersionProbe,
    AppServerSpawn,
    InitializeHandshake,
    AccountRead,
    Authentication,
    RateLimitsRead,
    FixedWindows,
    ResetCredits,
    WorkspaceCredits,
    TokenUsage,
    ProtocolContract,
}

impl DoctorCheckCode {
    const ALL: [Self; 12] = [
        Self::ExecutableDiscovery,
        Self::VersionProbe,
        Self::AppServerSpawn,
        Self::InitializeHandshake,
        Self::AccountRead,
        Self::Authentication,
        Self::RateLimitsRead,
        Self::FixedWindows,
        Self::ResetCredits,
        Self::WorkspaceCredits,
        Self::TokenUsage,
        Self::ProtocolContract,
    ];

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::ExecutableDiscovery => "executable_discovery",
            Self::VersionProbe => "version_probe",
            Self::AppServerSpawn => "app_server_spawn",
            Self::InitializeHandshake => "initialize_handshake",
            Self::AccountRead => "account_read",
            Self::Authentication => "authentication",
            Self::RateLimitsRead => "rate_limits_read",
            Self::FixedWindows => "fixed_windows",
            Self::ResetCredits => "reset_credits",
            Self::WorkspaceCredits => "workspace_credits",
            Self::TokenUsage => "token_usage",
            Self::ProtocolContract => "protocol_contract",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DoctorCheckStatus {
    Pass,
    Warning,
    Fail,
    NotRun,
    NotApplicable,
}

impl DoctorCheckStatus {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warning => "warning",
            Self::Fail => "fail",
            Self::NotRun => "not_run",
            Self::NotApplicable => "not_applicable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DoctorCheck {
    pub code: DoctorCheckCode,
    pub status: DoctorCheckStatus,
    pub reason_codes: Vec<ReasonCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DoctorDiscoverySummary {
    pub outcome: DiscoveryOutcome,
    pub candidate_count: u64,
    pub selected_sources: Vec<CandidateSource>,
    pub codex_version: Option<String>,
    pub identity_strength: Option<FileIdentityStrength>,
    pub path_redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct DoctorObservations {
    pub fixed_window_count: Option<u64>,
    pub reset_credit_summary: Option<SummaryStatus>,
    pub workspace_credit_summary: Option<SummaryStatus>,
    pub token_usage: Option<Availability>,
    pub status_axes: Option<DataStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct DoctorReport {
    pub schema_version: String,
    pub product_version: String,
    pub platform: String,
    pub architecture: String,
    pub outcome: DoctorOutcome,
    pub discovery: DoctorDiscoverySummary,
    pub checks: Vec<DoctorCheck>,
    pub observations: DoctorObservations,
    pub diagnostics: Vec<Diagnostic>,
}

impl DoctorReport {
    pub(super) fn discovery_failure(discovery: &DiscoveryReport) -> Self {
        let mut report = Self::base(discovery, DoctorOutcome::ExecutableError);
        report.set_check(
            DoctorCheckCode::ExecutableDiscovery,
            DoctorCheckStatus::Fail,
            discovery_reason_codes(discovery),
        );
        report.set_check(
            DoctorCheckCode::VersionProbe,
            if discovery.outcome == DiscoveryOutcome::VerificationFailed {
                DoctorCheckStatus::Fail
            } else {
                DoctorCheckStatus::NotRun
            },
            Vec::new(),
        );
        report.diagnostics = vec![outcome_diagnostic(DoctorOutcome::ExecutableError, None)];
        report
    }

    pub(super) fn live_failure(discovery: &DiscoveryReport, failure: &LiveReadFailure) -> Self {
        if matches!(failure, LiveReadFailure::MissingSelectedCandidate) {
            return Self::discovery_failure(discovery);
        }

        let outcome = match failure {
            LiveReadFailure::Runtime(RuntimeError::Spawn | RuntimeError::PipeUnavailable) => {
                DoctorOutcome::ExecutableError
            }
            LiveReadFailure::Runtime(RuntimeError::Timeout { .. }) => DoctorOutcome::Timeout,
            LiveReadFailure::Runtime(_)
            | LiveReadFailure::Normalize
            | LiveReadFailure::RuntimeInitialization => DoctorOutcome::ProtocolError,
            LiveReadFailure::MissingSelectedCandidate => DoctorOutcome::ExecutableError,
        };
        let mut report = Self::base(discovery, outcome);
        report.mark_selected_executable(discovery);

        match failure {
            LiveReadFailure::Runtime(RuntimeError::Spawn | RuntimeError::PipeUnavailable) => {
                report.set_check(
                    DoctorCheckCode::AppServerSpawn,
                    DoctorCheckStatus::Fail,
                    vec![reason("app_server_process_failed")],
                );
            }
            LiveReadFailure::Runtime(RuntimeError::Timeout { phase }) => {
                report.set_check(
                    DoctorCheckCode::AppServerSpawn,
                    DoctorCheckStatus::Pass,
                    Vec::new(),
                );
                report.mark_timeout_phase(phase);
            }
            LiveReadFailure::Runtime(RuntimeError::Rpc { method, .. })
            | LiveReadFailure::Runtime(RuntimeError::InvalidResult { method }) => {
                report.set_check(
                    DoctorCheckCode::AppServerSpawn,
                    DoctorCheckStatus::Pass,
                    Vec::new(),
                );
                report.mark_method_failure(method, "app_server_protocol_error");
            }
            LiveReadFailure::Runtime(RuntimeError::AccountChanged) => {
                report.set_check(
                    DoctorCheckCode::AppServerSpawn,
                    DoctorCheckStatus::Pass,
                    Vec::new(),
                );
                report.set_check(
                    DoctorCheckCode::Authentication,
                    DoctorCheckStatus::Fail,
                    vec![reason("account_changed_during_read")],
                );
            }
            LiveReadFailure::Normalize => {
                report.mark_all_reads_passed();
                report.set_check(
                    DoctorCheckCode::ProtocolContract,
                    DoctorCheckStatus::Fail,
                    vec![reason("status_contract_invalid")],
                );
            }
            LiveReadFailure::RuntimeInitialization => {
                report.set_check(
                    DoctorCheckCode::AppServerSpawn,
                    DoctorCheckStatus::Fail,
                    vec![reason("local_runtime_failed")],
                );
            }
            LiveReadFailure::Runtime(_) => {
                report.set_check(
                    DoctorCheckCode::AppServerSpawn,
                    DoctorCheckStatus::Pass,
                    Vec::new(),
                );
                report.set_check(
                    DoctorCheckCode::ProtocolContract,
                    DoctorCheckStatus::Fail,
                    vec![reason("app_server_protocol_error")],
                );
            }
            LiveReadFailure::MissingSelectedCandidate => {}
        }

        report.diagnostics = if matches!(
            failure,
            LiveReadFailure::Runtime(RuntimeError::AccountChanged)
        ) {
            vec![Diagnostic {
                code: reason("account_changed_during_read"),
                severity: DiagnosticSeverity::Error,
                message: "Codex changed account state during the read; no mixed-account snapshot was accepted.".to_owned(),
            }]
        } else {
            vec![outcome_diagnostic(outcome, None)]
        };
        report
    }

    pub(super) fn live_success(discovery: &DiscoveryReport, status: &StatusSnapshot) -> Self {
        let authentication_required = has_reason(status, "authentication_required");
        let authentication_unsupported = has_reason(status, "authentication_mode_unsupported");
        let outcome = if authentication_required {
            DoctorOutcome::AuthenticationRequired
        } else {
            match status.data_status.availability {
                Availability::Unsupported => DoctorOutcome::Unsupported,
                Availability::Failed => DoctorOutcome::ProtocolError,
                Availability::Partial => DoctorOutcome::Partial,
                Availability::Complete if status.data_status.freshness != Freshness::Live => {
                    DoctorOutcome::Partial
                }
                Availability::Complete => DoctorOutcome::Ready,
            }
        };
        let mut report = Self::base(discovery, outcome);
        report.mark_selected_executable(discovery);
        report.set_check(
            DoctorCheckCode::AppServerSpawn,
            DoctorCheckStatus::Pass,
            Vec::new(),
        );
        report.set_check(
            DoctorCheckCode::InitializeHandshake,
            DoctorCheckStatus::Pass,
            Vec::new(),
        );
        report.set_check(
            DoctorCheckCode::AccountRead,
            DoctorCheckStatus::Pass,
            Vec::new(),
        );
        report.set_check(
            DoctorCheckCode::ProtocolContract,
            DoctorCheckStatus::Pass,
            Vec::new(),
        );

        if authentication_required {
            report.set_check(
                DoctorCheckCode::Authentication,
                DoctorCheckStatus::Fail,
                vec![reason("authentication_required")],
            );
        } else if authentication_unsupported {
            report.set_check(
                DoctorCheckCode::Authentication,
                DoctorCheckStatus::Warning,
                vec![reason("authentication_mode_unsupported")],
            );
            report.mark_capacity_not_applicable();
        } else {
            report.set_check(
                DoctorCheckCode::Authentication,
                DoctorCheckStatus::Pass,
                Vec::new(),
            );
            report.mark_capacity_observations(status);
        }

        report.observations = DoctorObservations {
            fixed_window_count: if authentication_required || authentication_unsupported {
                None
            } else {
                Some(status.quota.windows.len() as u64)
            },
            reset_credit_summary: if authentication_required || authentication_unsupported {
                None
            } else {
                Some(status.quota.reset_credit_summary.summary_status)
            },
            workspace_credit_summary: if authentication_required || authentication_unsupported {
                None
            } else {
                Some(status.quota.usage_credit_summary.summary_status)
            },
            token_usage: if authentication_required || authentication_unsupported {
                None
            } else {
                Some(status.usage.availability)
            },
            status_axes: Some(status.data_status.clone()),
        };
        report.diagnostics = match outcome {
            DoctorOutcome::Ready => Vec::new(),
            DoctorOutcome::Unsupported => vec![outcome_diagnostic(
                outcome,
                preferred_unsupported_reason(status),
            )],
            _ => vec![outcome_diagnostic(outcome, None)],
        };
        report
    }

    pub(super) const fn exit_code(&self) -> u8 {
        self.outcome.exit_code()
    }

    fn base(discovery: &DiscoveryReport, outcome: DoctorOutcome) -> Self {
        Self {
            schema_version: DOCTOR_SCHEMA_VERSION.to_owned(),
            product_version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: discovery.platform.as_str().to_owned(),
            architecture: safe_architecture(&discovery.architecture),
            outcome,
            discovery: DoctorDiscoverySummary {
                outcome: discovery.outcome,
                candidate_count: discovery.candidates.len() as u64,
                selected_sources: Vec::new(),
                codex_version: None,
                identity_strength: None,
                path_redacted: true,
            },
            checks: DoctorCheckCode::ALL
                .into_iter()
                .map(|code| DoctorCheck {
                    code,
                    status: DoctorCheckStatus::NotRun,
                    reason_codes: Vec::new(),
                })
                .collect(),
            observations: DoctorObservations {
                fixed_window_count: None,
                reset_credit_summary: None,
                workspace_credit_summary: None,
                token_usage: None,
                status_axes: None,
            },
            diagnostics: Vec::new(),
        }
    }

    fn mark_selected_executable(&mut self, discovery: &DiscoveryReport) {
        let Some(candidate) = discovery.selected_candidate() else {
            return;
        };
        self.discovery.selected_sources = candidate.sources.clone();
        self.discovery.codex_version = candidate
            .version
            .as_deref()
            .filter(|value| safe_codex_version(value))
            .map(str::to_owned);
        self.discovery.identity_strength = Some(candidate.identity_strength);
        self.set_check(
            DoctorCheckCode::ExecutableDiscovery,
            DoctorCheckStatus::Pass,
            Vec::new(),
        );
        self.set_check(
            DoctorCheckCode::VersionProbe,
            if self.discovery.codex_version.is_some() {
                DoctorCheckStatus::Pass
            } else {
                DoctorCheckStatus::Fail
            },
            if self.discovery.codex_version.is_some() {
                Vec::new()
            } else {
                vec![reason("codex_version_unverified")]
            },
        );
    }

    fn mark_capacity_observations(&mut self, status: &StatusSnapshot) {
        self.set_check(
            DoctorCheckCode::RateLimitsRead,
            DoctorCheckStatus::Pass,
            Vec::new(),
        );
        self.set_check(
            DoctorCheckCode::FixedWindows,
            if status.quota.windows.is_empty() {
                DoctorCheckStatus::Warning
            } else {
                DoctorCheckStatus::Pass
            },
            if status.quota.windows.is_empty() {
                vec![reason("no_fixed_window")]
            } else {
                Vec::new()
            },
        );
        self.set_summary_check(
            DoctorCheckCode::ResetCredits,
            status.quota.reset_credit_summary.summary_status,
            "reset_credit_summary_unavailable",
        );
        self.set_summary_check(
            DoctorCheckCode::WorkspaceCredits,
            status.quota.usage_credit_summary.summary_status,
            "workspace_credit_summary_unavailable",
        );
        self.set_check(
            DoctorCheckCode::TokenUsage,
            match status.usage.availability {
                Availability::Complete => DoctorCheckStatus::Pass,
                Availability::Partial | Availability::Unsupported | Availability::Failed => {
                    DoctorCheckStatus::Warning
                }
            },
            status.usage.reason_codes.clone(),
        );
    }

    fn mark_capacity_not_applicable(&mut self) {
        for code in [
            DoctorCheckCode::RateLimitsRead,
            DoctorCheckCode::FixedWindows,
            DoctorCheckCode::ResetCredits,
            DoctorCheckCode::WorkspaceCredits,
            DoctorCheckCode::TokenUsage,
        ] {
            self.set_check(code, DoctorCheckStatus::NotApplicable, Vec::new());
        }
    }

    fn set_summary_check(
        &mut self,
        code: DoctorCheckCode,
        status: SummaryStatus,
        unavailable_reason: &'static str,
    ) {
        self.set_check(
            code,
            match status {
                SummaryStatus::Available => DoctorCheckStatus::Pass,
                SummaryStatus::Partial | SummaryStatus::Unavailable => DoctorCheckStatus::Warning,
            },
            match status {
                SummaryStatus::Available => Vec::new(),
                SummaryStatus::Partial => vec![reason("summary_partial")],
                SummaryStatus::Unavailable => vec![reason(unavailable_reason)],
            },
        );
    }

    fn mark_all_reads_passed(&mut self) {
        for code in [
            DoctorCheckCode::AppServerSpawn,
            DoctorCheckCode::InitializeHandshake,
            DoctorCheckCode::AccountRead,
            DoctorCheckCode::Authentication,
            DoctorCheckCode::RateLimitsRead,
        ] {
            self.set_check(code, DoctorCheckStatus::Pass, Vec::new());
        }
    }

    fn mark_timeout_phase(&mut self, phase: &'static str) {
        let code = match phase {
            "initialize" => DoctorCheckCode::InitializeHandshake,
            "account/read" => {
                self.set_check(
                    DoctorCheckCode::InitializeHandshake,
                    DoctorCheckStatus::Pass,
                    Vec::new(),
                );
                DoctorCheckCode::AccountRead
            }
            "account/rateLimits/read" => {
                self.mark_account_stage_passed();
                DoctorCheckCode::RateLimitsRead
            }
            "account/usage/read" => {
                self.mark_account_stage_passed();
                self.set_check(
                    DoctorCheckCode::RateLimitsRead,
                    DoctorCheckStatus::Pass,
                    Vec::new(),
                );
                DoctorCheckCode::TokenUsage
            }
            _ => DoctorCheckCode::AppServerSpawn,
        };
        self.set_check(
            code,
            DoctorCheckStatus::Fail,
            vec![reason("app_server_timeout")],
        );
    }

    fn mark_method_failure(&mut self, method: &'static str, reason_code: &'static str) {
        let code = match method {
            "initialize" => DoctorCheckCode::InitializeHandshake,
            "account/read" => {
                self.set_check(
                    DoctorCheckCode::InitializeHandshake,
                    DoctorCheckStatus::Pass,
                    Vec::new(),
                );
                DoctorCheckCode::AccountRead
            }
            "account/rateLimits/read" => {
                self.mark_account_stage_passed();
                DoctorCheckCode::RateLimitsRead
            }
            "account/usage/read" => {
                self.mark_account_stage_passed();
                self.set_check(
                    DoctorCheckCode::RateLimitsRead,
                    DoctorCheckStatus::Pass,
                    Vec::new(),
                );
                DoctorCheckCode::TokenUsage
            }
            _ => DoctorCheckCode::ProtocolContract,
        };
        self.set_check(code, DoctorCheckStatus::Fail, vec![reason(reason_code)]);
    }

    fn mark_account_stage_passed(&mut self) {
        for code in [
            DoctorCheckCode::InitializeHandshake,
            DoctorCheckCode::AccountRead,
            DoctorCheckCode::Authentication,
        ] {
            self.set_check(code, DoctorCheckStatus::Pass, Vec::new());
        }
    }

    fn set_check(
        &mut self,
        code: DoctorCheckCode,
        status: DoctorCheckStatus,
        reason_codes: Vec<ReasonCode>,
    ) {
        let check = self
            .checks
            .iter_mut()
            .find(|check| check.code == code)
            .expect("doctor check matrix contains every fixed check");
        check.status = status;
        check.reason_codes = reason_codes;
    }
}

fn safe_architecture(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        value.to_owned()
    } else {
        "unknown".to_owned()
    }
}

fn safe_codex_version(value: &str) -> bool {
    value.strip_prefix("codex-cli ").is_some_and(|version| {
        !version.is_empty()
            && version.len() <= 96
            && version.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_')
            })
    })
}

fn discovery_reason_codes(discovery: &DiscoveryReport) -> Vec<ReasonCode> {
    let mut codes = discovery
        .diagnostics
        .iter()
        .map(|diagnostic| match diagnostic.code.as_str() {
            "canonicalize_failed"
            | "codex_not_found"
            | "codex_version_unverified"
            | "cwd_path_entry_ignored"
            | "executable_metadata_failed"
            | "executable_not_executable"
            | "executable_not_file"
            | "macos_registry_unavailable"
            | "multiple_codex_executables"
            | "path_confirmation_required"
            | "version_failed"
            | "version_timeout" => diagnostic.code.clone(),
            _ => reason("discovery_detail_withheld"),
        })
        .collect::<Vec<_>>();
    codes.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    codes.dedup();
    codes
}

fn has_reason(status: &StatusSnapshot, expected: &str) -> bool {
    status
        .data_status
        .reason_codes
        .iter()
        .any(|code| code.as_str() == expected)
}

fn preferred_unsupported_reason(status: &StatusSnapshot) -> Option<ReasonCode> {
    ["authentication_mode_unsupported", "no_fixed_window"]
        .into_iter()
        .find_map(|expected| {
            status
                .data_status
                .reason_codes
                .iter()
                .find(|code| code.as_str() == expected)
                .cloned()
        })
}

fn outcome_diagnostic(outcome: DoctorOutcome, preferred: Option<ReasonCode>) -> Diagnostic {
    let (default_code, severity, message) = match outcome {
        DoctorOutcome::Ready => (
            "doctor_ready",
            DiagnosticSeverity::Info,
            "All required Technical Preview read-only checks passed.",
        ),
        DoctorOutcome::Partial => (
            "doctor_partial",
            DiagnosticSeverity::Warning,
            "The read-only probe succeeded with partial or stale capability facts.",
        ),
        DoctorOutcome::ExecutableError => (
            "doctor_executable_error",
            DiagnosticSeverity::Error,
            "A single verified Codex executable could not be selected or started.",
        ),
        DoctorOutcome::AuthenticationRequired => (
            "authentication_required",
            DiagnosticSeverity::Error,
            "Codex does not currently expose an authenticated ChatGPT account.",
        ),
        DoctorOutcome::Unsupported => (
            "fixed_window_unsupported",
            DiagnosticSeverity::Warning,
            "The current account does not expose a fixed quota window supported by this preview.",
        ),
        DoctorOutcome::Timeout => (
            "app_server_timeout",
            DiagnosticSeverity::Error,
            "The selected Codex app-server did not answer within a bounded timeout.",
        ),
        DoctorOutcome::ProtocolError => (
            "app_server_protocol_error",
            DiagnosticSeverity::Error,
            "The selected Codex app-server did not satisfy the read-only protocol contract.",
        ),
    };
    Diagnostic {
        code: preferred.unwrap_or_else(|| reason(default_code)),
        severity,
        message: message.to_owned(),
    }
}

fn reason(code: &'static str) -> ReasonCode {
    ReasonCode::new(code).expect("static doctor reason code must be valid")
}

#[cfg(test)]
mod tests {
    use capacity_domain::{Availability, Compatibility, Freshness};
    use codex_runtime::discovery::{CandidateVerification, CodexExecutableCandidate, HostPlatform};

    use super::*;

    fn selected_discovery() -> DiscoveryReport {
        DiscoveryReport {
            schema_version: "1.0".to_owned(),
            platform: HostPlatform::Macos,
            architecture: "aarch64".to_owned(),
            outcome: DiscoveryOutcome::Selected,
            selected_executable_id: Some("safe-id".to_owned()),
            candidates: vec![CodexExecutableCandidate {
                executable_id: "safe-id".to_owned(),
                sources: vec![CandidateSource::Explicit],
                discovered_paths: vec!["/Users/private/secret/codex".to_owned()],
                canonical_path: "/Users/private/secret/codex".to_owned(),
                file_identity: "Bearer secret-cookie".to_owned(),
                identity_strength: FileIdentityStrength::OsFileId,
                version: Some("codex-cli 1.2.3".to_owned()),
                verification: CandidateVerification::Verified,
                requires_confirmation: false,
                reason_codes: Vec::new(),
            }],
            diagnostics: Vec::new(),
        }
    }

    fn fixture_status(name: &str) -> StatusSnapshot {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/app-server")
                .join(name)
                .join("expected-status.json"),
        )
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn ready_report_is_a_path_and_account_allowlist() {
        let status = fixture_status("pro-multi-bucket");
        let report = DoctorReport::live_success(&selected_discovery(), &status);
        let json = serde_json::to_string(&report).unwrap();

        assert_eq!(report.outcome, DoctorOutcome::Ready);
        assert_eq!(report.exit_code(), 0);
        assert_eq!(report.observations.fixed_window_count, Some(3));
        assert!(report.discovery.path_redacted);
        for canary in [
            "/Users/private",
            "secret-cookie",
            "fixture-credit",
            "12.50",
            "plan_type",
            "auth_mode",
            "used_percent",
        ] {
            assert!(!json.contains(canary), "doctor JSON leaked {canary}");
        }
    }

    #[test]
    fn failure_categories_have_stable_exit_codes() {
        let discovery = selected_discovery();
        let timeout = DoctorReport::live_failure(
            &discovery,
            &LiveReadFailure::Runtime(RuntimeError::Timeout {
                phase: "initialize",
            }),
        );
        let protocol = DoctorReport::live_failure(
            &discovery,
            &LiveReadFailure::Runtime(RuntimeError::MalformedJson),
        );
        let account_changed = DoctorReport::live_failure(
            &discovery,
            &LiveReadFailure::Runtime(RuntimeError::AccountChanged),
        );

        assert_eq!(timeout.outcome, DoctorOutcome::Timeout);
        assert_eq!(timeout.exit_code(), 5);
        assert_eq!(protocol.outcome, DoctorOutcome::ProtocolError);
        assert_eq!(protocol.exit_code(), 5);
        assert_eq!(account_changed.outcome, DoctorOutcome::ProtocolError);
        assert_eq!(account_changed.exit_code(), 5);
        assert_eq!(
            account_changed.diagnostics[0].code.as_str(),
            "account_changed_during_read"
        );
    }

    #[test]
    fn unsupported_and_authentication_are_distinct() {
        let discovery = selected_discovery();
        let unsupported =
            DoctorReport::live_success(&discovery, &fixture_status("no-fixed-window"));
        let mut authentication = fixture_status("no-fixed-window");
        authentication.data_status.availability = Availability::Unsupported;
        authentication.data_status.freshness = Freshness::NotApplicable;
        authentication.data_status.compatibility = Compatibility::NotApplicable;
        authentication.data_status.reason_codes = vec![reason("authentication_required")];
        let authentication = DoctorReport::live_success(&discovery, &authentication);

        assert_eq!(unsupported.outcome, DoctorOutcome::Unsupported);
        assert_eq!(unsupported.exit_code(), 2);
        assert_eq!(
            authentication.outcome,
            DoctorOutcome::AuthenticationRequired
        );
        assert_eq!(authentication.exit_code(), 3);
    }

    #[test]
    fn discovery_error_does_not_copy_discovery_messages() {
        let mut discovery = selected_discovery();
        discovery.outcome = DiscoveryOutcome::NotFound;
        discovery.selected_executable_id = None;
        discovery.candidates.clear();
        discovery.diagnostics = vec![
            Diagnostic {
                code: reason("codex_not_found"),
                severity: DiagnosticSeverity::Error,
                message: "Cookie: secret-canary".to_owned(),
            },
            Diagnostic {
                code: reason("bearer_secret_canary"),
                severity: DiagnosticSeverity::Error,
                message: "unknown diagnostic".to_owned(),
            },
        ];
        let report = DoctorReport::discovery_failure(&discovery);
        let json = serde_json::to_string(&report).unwrap();

        assert_eq!(report.outcome, DoctorOutcome::ExecutableError);
        assert_eq!(report.exit_code(), 4);
        assert!(json.contains("codex_not_found"));
        assert!(json.contains("discovery_detail_withheld"));
        assert!(!json.contains("bearer_secret_canary"));
        assert!(!json.contains("secret-canary"));
    }
}
