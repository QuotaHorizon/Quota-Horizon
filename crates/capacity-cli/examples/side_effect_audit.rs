use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use capacity_cli::side_effect_audit::{
    CategoryDelta, FINGERPRINT_BUDGET_BYTES_PER_CATEGORY, MAX_FINGERPRINT_FILE_BYTES,
    capture_protected_snapshot, compare_snapshots, protected_active_change_count,
    runtime_cache_active_change_count,
};
use capacity_domain::{Availability, Clock, Compatibility, Freshness, SystemClock};
use clap::Parser;
use codex_runtime::app_server::RuntimeConfig;
use codex_runtime::discovery::{DiscoveryOutcome, discover_current};
use codex_runtime::refresh::{CodexStatusSource, RefreshHandle, RefreshOwnerConfig};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "capacity-side-effect-audit")]
#[command(about = "Run the internal redacted Codex file side-effect audit")]
struct Arguments {
    #[arg(long, value_name = "PATH")]
    codex: PathBuf,
    #[arg(long, value_name = "DIRECTORY")]
    codex_home: PathBuf,
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(0..=60))]
    control_seconds: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum AuditOutcome {
    Pass,
    ProtectedChangeObserved,
    ReadFailed,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeOutcome {
    ReadSucceeded,
    ReadFailed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct RuntimeReport {
    outcome: RuntimeOutcome,
    availability: Option<Availability>,
    freshness: Option<Freshness>,
    compatibility: Option<Compatibility>,
    window_count: Option<usize>,
    shutdown_clean: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct AuditReport {
    schema_version: &'static str,
    outcome: AuditOutcome,
    captured_at: String,
    control_seconds: u64,
    runtime: RuntimeReport,
    categories: Vec<CategoryDelta>,
    max_fingerprint_file_bytes: u64,
    fingerprint_budget_bytes_per_category: u64,
    protected_active_change_count: usize,
    runtime_cache_active_change_count: usize,
    writer_attribution: &'static str,
    paths_redacted: bool,
    fingerprints_redacted: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let control_before = match capture_protected_snapshot(&arguments.codex_home) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            eprintln!("capacity-side-effect-audit: the protected snapshot could not be captured");
            return ExitCode::from(5);
        }
    };
    tokio::time::sleep(Duration::from_secs(arguments.control_seconds)).await;
    let active_before = match capture_protected_snapshot(&arguments.codex_home) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            eprintln!("capacity-side-effect-audit: the control snapshot could not be captured");
            return ExitCode::from(5);
        }
    };

    let runtime = run_read(arguments.codex).await;
    let active_after = match capture_protected_snapshot(&arguments.codex_home) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            eprintln!("capacity-side-effect-audit: the final snapshot could not be captured");
            return ExitCode::from(5);
        }
    };
    let categories = compare_snapshots(&control_before, &active_before, &active_after);
    let protected_changes = protected_active_change_count(&categories);
    let runtime_cache_changes = runtime_cache_active_change_count(&categories);
    let read_succeeded =
        matches!(runtime.outcome, RuntimeOutcome::ReadSucceeded) && runtime.shutdown_clean;
    let outcome = if !read_succeeded {
        AuditOutcome::ReadFailed
    } else if protected_changes == 0 {
        AuditOutcome::Pass
    } else {
        AuditOutcome::ProtectedChangeObserved
    };
    let report = AuditReport {
        schema_version: "1.0",
        outcome,
        captured_at: SystemClock.now().as_str().to_owned(),
        control_seconds: arguments.control_seconds,
        runtime,
        categories,
        max_fingerprint_file_bytes: MAX_FINGERPRINT_FILE_BYTES,
        fingerprint_budget_bytes_per_category: FINGERPRINT_BUDGET_BYTES_PER_CATEGORY,
        protected_active_change_count: protected_changes,
        runtime_cache_active_change_count: runtime_cache_changes,
        writer_attribution: "snapshot_diff_does_not_identify_writer",
        paths_redacted: true,
        fingerprints_redacted: true,
    };
    if serde_json::to_writer_pretty(std::io::stdout(), &report).is_err() {
        eprintln!("capacity-side-effect-audit: failed to write the redacted report");
        return ExitCode::from(5);
    }
    println!();

    match outcome {
        AuditOutcome::Pass => ExitCode::SUCCESS,
        AuditOutcome::ProtectedChangeObserved => ExitCode::from(6),
        AuditOutcome::ReadFailed => ExitCode::from(5),
    }
}

async fn run_read(codex: PathBuf) -> RuntimeReport {
    let discovery = discover_current(Some(codex));
    if discovery.outcome != DiscoveryOutcome::Selected {
        return failed_runtime_report();
    }
    let source = match CodexStatusSource::from_discovery(&discovery, RuntimeConfig::default()) {
        Ok(source) => source,
        Err(_) => return failed_runtime_report(),
    };
    let owner = match RefreshHandle::start(source, RefreshOwnerConfig::default()) {
        Ok(owner) => owner,
        Err(_) => return failed_runtime_report(),
    };
    let refresh = owner.refresh().await;
    let shutdown_clean = owner.shutdown().await.is_ok();
    match refresh {
        Ok(snapshot) => RuntimeReport {
            outcome: RuntimeOutcome::ReadSucceeded,
            availability: Some(snapshot.snapshot.data_status.availability),
            freshness: Some(snapshot.snapshot.data_status.freshness),
            compatibility: Some(snapshot.snapshot.data_status.compatibility),
            window_count: Some(snapshot.snapshot.quota.windows.len()),
            shutdown_clean,
        },
        Err(_) => RuntimeReport {
            shutdown_clean,
            ..failed_runtime_report()
        },
    }
}

const fn failed_runtime_report() -> RuntimeReport {
    RuntimeReport {
        outcome: RuntimeOutcome::ReadFailed,
        availability: None,
        freshness: None,
        compatibility: None,
        window_count: None,
        shutdown_clean: false,
    }
}
