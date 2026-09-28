mod doctor;
pub mod side_effect_audit;
pub mod soak;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use capacity_domain::{
    AppError, AppErrorKind, Availability, Clock, CodexExecutableSnapshot, Compatibility,
    DataStatus, Diagnostic, DiagnosticSeverity, EnvironmentSnapshot, Freshness, QuotaSnapshot,
    ReasonCode, ResetCreditDetailsStatus, ResetCreditSummary, STATUS_SCHEMA_VERSION,
    StatusSnapshot, SummaryStatus, SystemClock, UsageCreditSummary, UsageSnapshot, UtcTimestamp,
};
use capacity_migration::{
    ImportConflictKind, LegacyArtifactKind, LegacyArtifactState, LegacyDryRunStatus,
    dry_run_quotaviewer,
};
use clap::{Parser, Subcommand};
use codex_runtime::discovery::{
    CandidateSource, DiscoveryOutcome, DiscoveryReport, HostPlatform, discover_current,
};
use codex_runtime::normalize::{LiveStatusContext, normalize_capacity_read};
use codex_runtime::{
    app_server::{RuntimeConfig, RuntimeError, read_codex_capacity},
    discovery::CodexExecutableCandidate,
};
use doctor::DoctorReport;

const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/app-server");

#[derive(Debug, Parser)]
#[command(name = "capacity-cli")]
#[command(version)]
#[command(about = "Read and diagnose local Codex capacity data")]
pub struct Arguments {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Doctor {
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "PATH")]
        codex: Option<PathBuf>,
    },
    Status {
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "PATH", conflicts_with = "fixture")]
        codex: Option<PathBuf>,
        #[arg(long, value_name = "SCENARIO", conflicts_with = "codex")]
        fixture: Option<String>,
    },
    Migrate {
        #[command(subcommand)]
        command: MigrationCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum MigrationCommand {
    DryRun {
        #[arg(long, value_name = "DIRECTORY")]
        source: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(arguments: Arguments, stdout: &mut impl Write, stderr: &mut impl Write) -> u8 {
    match arguments.command {
        Command::Doctor { json, codex } => run_doctor(json, codex, stdout, stderr),
        Command::Status {
            json,
            codex,
            fixture,
        } => run_status(json, codex, fixture, stdout, stderr),
        Command::Migrate {
            command: MigrationCommand::DryRun { source, json },
        } => run_migration_dry_run(&source, json, stdout, stderr),
    }
}

fn run_migration_dry_run(
    source: &Path,
    json: bool,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> u8 {
    let report = dry_run_quotaviewer(source);
    let exit_code = report.exit_code();
    let write_result = if json {
        write_json(stdout, &report)
    } else {
        write_migration_text(stdout, &report)
    };
    if write_result.is_err() {
        let _ = writeln!(stderr, "capacity-cli: failed to write migration report");
        return 5;
    }
    exit_code
}

pub fn load_fixture_status(name: &str) -> Result<StatusSnapshot, AppError> {
    load_fixture_status_from(Path::new(FIXTURE_ROOT), name)
}

fn load_fixture_status_from(root: &Path, name: &str) -> Result<StatusSnapshot, AppError> {
    if !valid_fixture_name(name) {
        return Err(AppError::new(
            AppErrorKind::InvalidFixture,
            "invalid_fixture_name",
            "Fixture names may contain only lowercase ASCII letters, digits, '-' and '_'.",
        ));
    }

    let bytes = fs::read(root.join(name).join("expected-status.json")).map_err(|_| {
        AppError::new(
            AppErrorKind::InvalidFixture,
            "fixture_not_found",
            "The requested fixture was not found.",
        )
    })?;
    let status: StatusSnapshot = serde_json::from_slice(&bytes).map_err(|_| {
        AppError::new(
            AppErrorKind::InvalidFixture,
            "fixture_json_invalid",
            "The fixture status JSON is invalid.",
        )
    })?;
    status.validate().map_err(|_| {
        AppError::new(
            AppErrorKind::InvalidFixture,
            "fixture_contract_invalid",
            "The fixture does not satisfy status JSON v1.",
        )
    })?;
    Ok(status)
}

fn run_doctor(
    json: bool,
    codex: Option<PathBuf>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> u8 {
    let discovery = discover_current(codex);
    let report = if discovery.outcome == DiscoveryOutcome::Selected {
        match read_live_status(&discovery, &SystemClock) {
            Ok(status) => DoctorReport::live_success(&discovery, &status),
            Err(error) => DoctorReport::live_failure(&discovery, &error),
        }
    } else {
        DoctorReport::discovery_failure(&discovery)
    };
    let exit_code = report.exit_code();
    let write_result = if json {
        write_json(stdout, &report)
    } else {
        write_doctor_text(stdout, &report)
    };
    if write_result.is_err() {
        let _ = writeln!(stderr, "capacity-cli: failed to write diagnostic output");
        return 5;
    }
    exit_code
}

fn run_status(
    json: bool,
    codex: Option<PathBuf>,
    fixture: Option<String>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> u8 {
    if let Some(fixture) = fixture {
        return match load_fixture_status(&fixture) {
            Ok(status) => {
                let write_result = if json {
                    write_json(stdout, &status)
                } else {
                    write_status_text(stdout, &status)
                };
                if write_result.is_err() {
                    let _ = writeln!(stderr, "capacity-cli: failed to write status output");
                    return 5;
                }
                status_exit_code(&status)
            }
            Err(error) => {
                let _ = writeln!(stderr, "capacity-cli: {error}");
                error.exit_code()
            }
        };
    }

    let discovery = discover_current(codex);
    let (status, exit_code) = if discovery.outcome == DiscoveryOutcome::Selected {
        match read_live_status(&discovery, &SystemClock) {
            Ok(status) => {
                let exit_code = status_exit_code(&status);
                (status, exit_code)
            }
            Err(error) => {
                let (reason_code, message) = live_failure_diagnostic(&error);
                let exit_code = live_failure_exit_code(&error);
                (
                    failed_live_status(&discovery, &SystemClock, reason_code, message),
                    exit_code,
                )
            }
        }
    } else {
        (
            unavailable_live_status(&discovery, &SystemClock),
            discovery.exit_code(),
        )
    };
    let write_result = if json {
        write_json(stdout, &status)
    } else {
        write_status_text(stdout, &status)
    };
    if write_result.is_err() {
        let _ = writeln!(stderr, "capacity-cli: failed to write status output");
        return 5;
    }

    exit_code
}

fn live_failure_exit_code(error: &LiveReadFailure) -> u8 {
    match error {
        LiveReadFailure::Runtime(RuntimeError::Spawn | RuntimeError::PipeUnavailable)
        | LiveReadFailure::MissingSelectedCandidate => AppErrorKind::Discovery.exit_code(),
        LiveReadFailure::Runtime(_)
        | LiveReadFailure::Normalize
        | LiveReadFailure::RuntimeInitialization => AppErrorKind::Protocol.exit_code(),
    }
}

#[derive(Debug)]
enum LiveReadFailure {
    Runtime(RuntimeError),
    Normalize,
    RuntimeInitialization,
    MissingSelectedCandidate,
}

fn read_live_status(
    discovery: &DiscoveryReport,
    clock: &dyn Clock,
) -> Result<StatusSnapshot, LiveReadFailure> {
    let candidate = discovery
        .selected_candidate()
        .ok_or(LiveReadFailure::MissingSelectedCandidate)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| LiveReadFailure::RuntimeInitialization)?;
    let read = runtime
        .block_on(read_codex_capacity(
            Path::new(&candidate.canonical_path),
            RuntimeConfig::default(),
        ))
        .map_err(LiveReadFailure::Runtime)?;
    let context = LiveStatusContext {
        captured_at: clock.now(),
        environment: EnvironmentSnapshot {
            environment_id: format!(
                "{}:local:{}",
                discovery.platform.as_str(),
                candidate.executable_id
            ),
            platform: discovery.platform.as_str().to_owned(),
            architecture: discovery.architecture.clone(),
            boundary: if discovery.platform == HostPlatform::Unknown {
                "unknown".to_owned()
            } else {
                "native".to_owned()
            },
        },
        codex_executable: executable_snapshot(candidate),
    };
    normalize_capacity_read(read, context).map_err(|_| LiveReadFailure::Normalize)
}

fn live_failure_diagnostic(error: &LiveReadFailure) -> (&'static str, &'static str) {
    match error {
        LiveReadFailure::Runtime(RuntimeError::AccountChanged) => (
            "account_changed_during_read",
            "Codex changed account state during the read; no mixed-account snapshot was accepted.",
        ),
        LiveReadFailure::Runtime(RuntimeError::Timeout { .. }) => (
            "app_server_timeout",
            "The selected Codex app-server did not answer within the bounded timeout.",
        ),
        LiveReadFailure::Runtime(
            RuntimeError::Spawn
            | RuntimeError::PipeUnavailable
            | RuntimeError::Read
            | RuntimeError::Write
            | RuntimeError::UnexpectedEof
            | RuntimeError::PartialLineEof
            | RuntimeError::Shutdown,
        ) => (
            "app_server_process_failed",
            "The selected Codex app-server process could not complete a managed read.",
        ),
        LiveReadFailure::Runtime(_) | LiveReadFailure::Normalize => (
            "app_server_protocol_error",
            "The selected Codex app-server returned data that did not satisfy the read-only quota contract.",
        ),
        LiveReadFailure::RuntimeInitialization => (
            "local_runtime_failed",
            "The local asynchronous runtime could not be initialized.",
        ),
        LiveReadFailure::MissingSelectedCandidate => (
            "codex_discovery_failed",
            "A verified Codex executable was not available for the live read.",
        ),
    }
}

fn unavailable_live_status(discovery: &DiscoveryReport, clock: &dyn Clock) -> StatusSnapshot {
    let selected = discovery.selected_candidate();
    let captured_at = clock.now();
    let reason_code = "codex_discovery_failed";
    let message = "A Codex executable must be selected before live status can be read.";
    let mut diagnostics = discovery.diagnostics.clone();
    diagnostics.push(Diagnostic {
        code: reason(reason_code),
        severity: DiagnosticSeverity::Error,
        message: message.to_owned(),
    });

    StatusSnapshot {
        schema_version: STATUS_SCHEMA_VERSION.to_owned(),
        captured_at: captured_at.clone(),
        environment: EnvironmentSnapshot {
            environment_id: format!("{}:local:unbound", discovery.platform.as_str()),
            platform: discovery.platform.as_str().to_owned(),
            architecture: discovery.architecture.clone(),
            boundary: if discovery.platform == HostPlatform::Unknown {
                "unknown".to_owned()
            } else {
                "native".to_owned()
            },
        },
        codex_executable: selected.map(executable_snapshot),
        account: None,
        quota: QuotaSnapshot {
            windows: Vec::new(),
            reset_credit_summary: unavailable_reset_credit_summary(),
            usage_credit_summary: unavailable_usage_credit_summary(captured_at),
        },
        usage: UsageSnapshot {
            availability: Availability::Failed,
            summary: None,
            reason_codes: vec![reason(reason_code)],
        },
        data_status: DataStatus {
            availability: Availability::Failed,
            freshness: Freshness::NotApplicable,
            compatibility: Compatibility::NotTested,
            reason_codes: vec![reason(reason_code), reason("compatibility_not_tested")],
        },
        diagnostics,
    }
}

fn failed_live_status(
    discovery: &DiscoveryReport,
    clock: &dyn Clock,
    reason_code: &'static str,
    message: &'static str,
) -> StatusSnapshot {
    let selected = discovery.selected_candidate();
    let captured_at = clock.now();
    StatusSnapshot {
        schema_version: STATUS_SCHEMA_VERSION.to_owned(),
        captured_at: captured_at.clone(),
        environment: EnvironmentSnapshot {
            environment_id: format!("{}:local:unbound", discovery.platform.as_str()),
            platform: discovery.platform.as_str().to_owned(),
            architecture: discovery.architecture.clone(),
            boundary: if discovery.platform == HostPlatform::Unknown {
                "unknown".to_owned()
            } else {
                "native".to_owned()
            },
        },
        codex_executable: selected.map(executable_snapshot),
        account: None,
        quota: QuotaSnapshot {
            windows: Vec::new(),
            reset_credit_summary: unavailable_reset_credit_summary(),
            usage_credit_summary: unavailable_usage_credit_summary(captured_at),
        },
        usage: UsageSnapshot {
            availability: Availability::Failed,
            summary: None,
            reason_codes: vec![reason(reason_code)],
        },
        data_status: DataStatus {
            availability: Availability::Failed,
            freshness: Freshness::NotApplicable,
            compatibility: Compatibility::NotTested,
            reason_codes: vec![reason(reason_code), reason("compatibility_not_tested")],
        },
        diagnostics: vec![Diagnostic {
            code: reason(reason_code),
            severity: DiagnosticSeverity::Error,
            message: message.to_owned(),
        }],
    }
}

fn unavailable_reset_credit_summary() -> ResetCreditSummary {
    ResetCreditSummary {
        summary_status: SummaryStatus::Unavailable,
        available_count: None,
        details_status: ResetCreditDetailsStatus::Unavailable,
        credits: None,
    }
}

fn unavailable_usage_credit_summary(captured_at: UtcTimestamp) -> UsageCreditSummary {
    UsageCreditSummary {
        summary_status: SummaryStatus::Unavailable,
        entries: Vec::new(),
        captured_at,
        upstream_schema_fingerprint: None,
    }
}

fn executable_snapshot(candidate: &CodexExecutableCandidate) -> CodexExecutableSnapshot {
    CodexExecutableSnapshot {
        executable_id: candidate.executable_id.clone(),
        source: candidate
            .sources
            .first()
            .copied()
            .unwrap_or(CandidateSource::Path)
            .as_str()
            .to_owned(),
        canonical_path: Some(candidate.canonical_path.clone()),
        file_identity: Some(candidate.file_identity.clone()),
        version: candidate.version.clone(),
    }
}

fn status_exit_code(status: &StatusSnapshot) -> u8 {
    if status
        .data_status
        .reason_codes
        .iter()
        .any(|code| code.as_str() == "authentication_required")
    {
        return AppErrorKind::AuthenticationRequired.exit_code();
    }
    match status.data_status.availability {
        Availability::Complete | Availability::Partial => 0,
        Availability::Unsupported => 2,
        Availability::Failed => 5,
    }
}

fn write_json(writer: &mut impl Write, value: &impl serde::Serialize) -> std::io::Result<()> {
    serde_json::to_writer_pretty(&mut *writer, value).map_err(std::io::Error::other)?;
    writeln!(writer)
}

fn write_doctor_text(writer: &mut impl Write, report: &DoctorReport) -> std::io::Result<()> {
    writeln!(writer, "Doctor outcome: {}", report.outcome.as_str())?;
    writeln!(
        writer,
        "Codex: {} candidate(s), {}",
        report.discovery.candidate_count,
        report
            .discovery
            .codex_version
            .as_deref()
            .unwrap_or("version unavailable")
    )?;
    for check in &report.checks {
        writeln!(
            writer,
            "- {}: {}{}",
            check.code.as_str(),
            check.status.as_str(),
            check
                .reason_codes
                .first()
                .map(|code| format!(" ({})", code.as_str()))
                .unwrap_or_default()
        )?;
    }
    for item in &report.diagnostics {
        writeln!(writer, "{}: {}", item.code.as_str(), item.message)?;
    }
    Ok(())
}

fn write_status_text(writer: &mut impl Write, status: &StatusSnapshot) -> std::io::Result<()> {
    writeln!(
        writer,
        "Status: {:?} / {:?} / {:?}",
        status.data_status.availability,
        status.data_status.freshness,
        status.data_status.compatibility
    )?;
    for window in &status.quota.windows {
        writeln!(
            writer,
            "- {}: {:.1}% remaining",
            window.label.as_deref().unwrap_or(&window.limit_id),
            window.remaining_percent
        )?;
    }
    for item in &status.diagnostics {
        writeln!(writer, "{}: {}", item.code.as_str(), item.message)?;
    }
    Ok(())
}

fn write_migration_text(
    writer: &mut impl Write,
    report: &capacity_migration::LegacyDryRunReport,
) -> std::io::Result<()> {
    writeln!(
        writer,
        "QuotaViewer migration dry run: {}",
        migration_status_name(report.status)
    )?;
    for artifact in &report.inventory {
        writeln!(
            writer,
            "- {}: {} ({} file(s), {} record(s), {} byte(s))",
            artifact_kind_name(artifact.kind),
            artifact_state_name(artifact.state),
            artifact.file_count,
            artifact.record_count,
            artifact.source_bytes
        )?;
    }
    for conflict in &report.conflicts {
        writeln!(
            writer,
            "conflict: {} x{} ({})",
            conflict_kind_name(conflict.kind),
            conflict.count,
            conflict.reason_code
        )?;
    }
    for reason in &report.reason_codes {
        writeln!(writer, "reason: {reason}")?;
    }
    Ok(())
}

fn migration_status_name(status: LegacyDryRunStatus) -> &'static str {
    match status {
        LegacyDryRunStatus::Ready => "ready",
        LegacyDryRunStatus::Partial => "partial",
        LegacyDryRunStatus::Unsupported => "unsupported",
        LegacyDryRunStatus::Failed => "failed",
    }
}

fn artifact_kind_name(kind: LegacyArtifactKind) -> &'static str {
    match kind {
        LegacyArtifactKind::Settings => "settings",
        LegacyArtifactKind::AccountIndex => "account_index",
        LegacyArtifactKind::AccountRecords => "account_records",
        LegacyArtifactKind::QuotaCache => "quota_cache",
        LegacyArtifactKind::SessionManagerSettings => "session_manager_settings",
        LegacyArtifactKind::ProviderMode => "provider_mode",
        LegacyArtifactKind::RestorePoints => "restore_points",
    }
}

fn artifact_state_name(state: LegacyArtifactState) -> &'static str {
    match state {
        LegacyArtifactState::Present => "present",
        LegacyArtifactState::Missing => "missing",
        LegacyArtifactState::Invalid => "invalid",
        LegacyArtifactState::Unsafe => "unsafe",
        LegacyArtifactState::LimitExceeded => "limit_exceeded",
    }
}

fn conflict_kind_name(kind: ImportConflictKind) -> &'static str {
    match kind {
        ImportConflictKind::DuplicateAccountId => "duplicate_account_id",
        ImportConflictKind::MissingAccountRecord => "missing_account_record",
        ImportConflictKind::OrphanAccountRecord => "orphan_account_record",
        ImportConflictKind::InvalidAccountRecord => "invalid_account_record",
        ImportConflictKind::InvalidRestorePoint => "invalid_restore_point",
        ImportConflictKind::ProviderRestorePointMissing => "provider_restore_point_missing",
    }
}

fn valid_fixture_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn reason(code: &'static str) -> ReasonCode {
    ReasonCode::new(code).expect("static reason code must be valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_fixture_has_stable_pretty_json() {
        let status = load_fixture_status("plus-normal").unwrap();
        let actual = serde_json::to_string_pretty(&status).unwrap();
        let expected =
            include_str!("../../../fixtures/app-server/plus-normal/expected-status.json");

        assert_eq!(actual, expected.trim_end());
    }

    #[test]
    fn failed_fixture_maps_to_protocol_exit_code() {
        let status = load_fixture_status("malformed-response").unwrap();
        assert_eq!(status_exit_code(&status), 5);
    }

    #[test]
    fn every_versioned_fixture_satisfies_status_v1_and_expected_exit_semantics() {
        for (name, expected_exit) in [
            ("account-updated-during-read", 5),
            ("bucket-disappeared", 0),
            ("dst-boundary", 0),
            ("early-eof", 5),
            ("malformed-response", 5),
            ("no-fixed-window", 2),
            ("oversized-line", 5),
            ("plus-normal", 0),
            ("pro-multi-bucket", 0),
            ("process-timeout", 5),
            ("reset-credit-count-only", 0),
            ("reset-credit-partial", 0),
            ("weekly-only", 0),
            ("wrong-request-id", 5),
        ] {
            let status = load_fixture_status(name).unwrap();
            assert_eq!(status_exit_code(&status), expected_exit, "fixture {name}");
        }
    }

    #[test]
    fn fixture_name_cannot_escape_the_fixture_root() {
        let error = load_fixture_status("../private-data").unwrap_err();
        assert_eq!(error.reason_code.as_str(), "invalid_fixture_name");
    }

    #[test]
    fn command_surface_accepts_the_frozen_wp0_examples() {
        assert!(Arguments::try_parse_from(["capacity-cli", "doctor", "--json"]).is_ok());
        assert!(
            Arguments::try_parse_from([
                "capacity-cli",
                "status",
                "--fixture",
                "plus-normal",
                "--json"
            ])
            .is_ok()
        );
        assert!(
            Arguments::try_parse_from([
                "capacity-cli",
                "migrate",
                "dry-run",
                "--source",
                "/tmp/quotaviewer-fixture",
                "--json",
            ])
            .is_ok()
        );
    }

    #[test]
    fn migration_cli_emits_redacted_json_and_preserves_partial_exit_code() {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/migration/source/partial");
        let arguments = Arguments::try_parse_from([
            "capacity-cli",
            "migrate",
            "dry-run",
            "--source",
            source.to_str().unwrap(),
            "--json",
        ])
        .unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        assert_eq!(run(arguments, &mut stdout, &mut stderr), 2);
        let output = String::from_utf8(stdout).unwrap();
        assert!(stderr.is_empty());
        assert!(output.contains("\"status\": \"partial\""));
        for canary in [
            source.to_string_lossy().as_ref(),
            "missing-account",
            "orphan-account",
            "private@example.com",
        ] {
            assert!(!output.contains(canary));
        }
    }

    #[test]
    fn migration_text_is_aggregate_only() {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/migration/source/ready");
        let arguments = Arguments::try_parse_from([
            "capacity-cli",
            "migrate",
            "dry-run",
            "--source",
            source.to_str().unwrap(),
        ])
        .unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        assert_eq!(run(arguments, &mut stdout, &mut stderr), 0);
        let output = String::from_utf8(stdout).unwrap();
        assert!(stderr.is_empty());
        assert!(output.contains("QuotaViewer migration dry run: ready"));
        assert!(output.contains("- account_records: present"));
        assert!(!output.contains(source.to_string_lossy().as_ref()));
        assert!(!output.contains("person@example.com"));
    }

    #[test]
    fn live_failure_exit_codes_separate_spawn_from_protocol_failures() {
        assert_eq!(
            live_failure_exit_code(&LiveReadFailure::Runtime(RuntimeError::Spawn)),
            4
        );
        assert_eq!(
            live_failure_exit_code(&LiveReadFailure::Runtime(RuntimeError::Timeout {
                phase: "initialize",
            })),
            5
        );
        assert_eq!(
            live_failure_exit_code(&LiveReadFailure::Runtime(RuntimeError::AccountChanged)),
            5
        );
    }
}
