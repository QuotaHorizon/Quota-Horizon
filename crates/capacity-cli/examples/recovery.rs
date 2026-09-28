use std::collections::VecDeque;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use capacity_domain::StatusSnapshot;
use codex_runtime::refresh::{
    DisconnectFuture, RefreshFailure, RefreshFuture, RefreshHandle, RefreshOwnerConfig,
    RefreshOwnerError, RefreshProvenance, RefreshSource,
};
use serde::Serialize;
use tokio::time::sleep;

const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/app-server");

#[derive(Debug, Serialize)]
struct RecoveryReport {
    schema_version: &'static str,
    outcome: &'static str,
    scenario_count: usize,
    passed_scenarios: usize,
    scenarios: Vec<ScenarioReport>,
    path_redacted: bool,
}

#[derive(Debug, Serialize)]
struct ScenarioReport {
    name: &'static str,
    outcome: &'static str,
    checks_total: usize,
    checks_passed: usize,
    checks: Vec<CheckReport>,
}

#[derive(Debug, Serialize)]
struct CheckReport {
    name: &'static str,
    passed: bool,
}

impl ScenarioReport {
    fn from_checks(name: &'static str, checks: Vec<CheckReport>) -> Self {
        let checks_passed = checks.iter().filter(|check| check.passed).count();
        Self {
            name,
            outcome: if checks_passed == checks.len() {
                "pass"
            } else {
                "failed"
            },
            checks_total: checks.len(),
            checks_passed,
            checks,
        }
    }
}

fn check(name: &'static str, passed: bool) -> CheckReport {
    CheckReport { name, passed }
}

struct ScriptedSource {
    results: VecDeque<Result<StatusSnapshot, RefreshFailure>>,
    calls: Arc<AtomicUsize>,
    disconnects: Arc<AtomicUsize>,
}

impl RefreshSource for ScriptedSource {
    fn refresh(&mut self) -> RefreshFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = self
            .results
            .pop_front()
            .unwrap_or(Err(RefreshFailure::ProtocolError));
        Box::pin(async move { result })
    }

    fn disconnect(&mut self) -> DisconnectFuture<'_> {
        self.disconnects.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn scripted(
    results: impl IntoIterator<Item = Result<StatusSnapshot, RefreshFailure>>,
) -> (ScriptedSource, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let disconnects = Arc::new(AtomicUsize::new(0));
    (
        ScriptedSource {
            results: results.into_iter().collect(),
            calls: Arc::clone(&calls),
            disconnects: Arc::clone(&disconnects),
        },
        calls,
        disconnects,
    )
}

fn recovery_config() -> RefreshOwnerConfig {
    RefreshOwnerConfig {
        idle_timeout: Duration::from_millis(25),
        backoff_schedule: vec![Duration::from_millis(25)],
        jitter_percent: 0,
        command_capacity: 16,
    }
}

fn fixture_status() -> StatusSnapshot {
    let bytes = std::fs::read(
        std::path::Path::new(FIXTURE_ROOT)
            .join("plus-normal")
            .join("expected-status.json"),
    )
    .expect("versioned recovery fixture must exist");
    serde_json::from_slice(&bytes).expect("versioned recovery fixture must satisfy status v1")
}

async fn transient_recovery(
    name: &'static str,
    failure: RefreshFailure,
    live: &StatusSnapshot,
) -> ScenarioReport {
    let (source, calls, _) = scripted([Ok(live.clone()), Err(failure), Ok(live.clone())]);
    let handle = match RefreshHandle::start(source, recovery_config()) {
        Ok(handle) => handle,
        Err(_) => {
            return ScenarioReport::from_checks(name, vec![check("owner_started", false)]);
        }
    };

    let initial = handle.refresh().await;
    let stale = handle.refresh().await;
    let blocked = handle.refresh().await;
    sleep(Duration::from_millis(40)).await;
    let recovered = handle.refresh().await;
    let shutdown_clean = handle.shutdown().await.is_ok();

    ScenarioReport::from_checks(
        name,
        vec![
            check(
                "initial_current",
                matches!(
                    initial,
                    Ok(ref value) if value.provenance == RefreshProvenance::CurrentRead
                ),
            ),
            check(
                "stale_fallback",
                matches!(
                    stale,
                    Ok(ref value) if value.provenance == RefreshProvenance::StaleFallback
                ),
            ),
            check(
                "backoff_enforced",
                matches!(blocked, Err(RefreshOwnerError::Backoff { .. })),
            ),
            check(
                "new_current_generation",
                matches!(
                    (&stale, &recovered),
                    (Ok(stale), Ok(recovered))
                        if recovered.provenance == RefreshProvenance::CurrentRead
                            && recovered.generation > stale.generation
                ),
            ),
            check("exact_source_calls", calls.load(Ordering::SeqCst) == 3),
            check("shutdown_clean", shutdown_clean),
        ],
    )
}

async fn boundary_recovery(
    name: &'static str,
    failure: RefreshFailure,
    live: &StatusSnapshot,
) -> ScenarioReport {
    let (source, calls, _) = scripted([Ok(live.clone()), Err(failure), Ok(live.clone())]);
    let handle = match RefreshHandle::start(source, recovery_config()) {
        Ok(handle) => handle,
        Err(_) => {
            return ScenarioReport::from_checks(name, vec![check("owner_started", false)]);
        }
    };

    let initial = handle.refresh().await;
    let boundary = handle.refresh().await;
    let latest_after_boundary = handle.latest().await;
    let blocked = handle.refresh().await;
    sleep(Duration::from_millis(40)).await;
    let recovered = handle.refresh().await;
    let shutdown_clean = handle.shutdown().await.is_ok();

    ScenarioReport::from_checks(
        name,
        vec![
            check(
                "initial_current",
                matches!(
                    initial,
                    Ok(ref value) if value.provenance == RefreshProvenance::CurrentRead
                ),
            ),
            check(
                "boundary_failed_closed",
                matches!(boundary, Err(RefreshOwnerError::Source(value)) if value == failure),
            ),
            check(
                "old_cache_cleared",
                matches!(latest_after_boundary, Ok(None)),
            ),
            check(
                "backoff_enforced",
                matches!(blocked, Err(RefreshOwnerError::Backoff { .. })),
            ),
            check(
                "new_current_generation",
                matches!(
                    recovered,
                    Ok(ref value) if value.provenance == RefreshProvenance::CurrentRead
                ),
            ),
            check("exact_source_calls", calls.load(Ordering::SeqCst) == 3),
            check("shutdown_clean", shutdown_clean),
        ],
    )
}

async fn idle_gap_recovery(live: &StatusSnapshot) -> ScenarioReport {
    let (source, calls, disconnects) = scripted([Ok(live.clone()), Ok(live.clone())]);
    let handle = match RefreshHandle::start(source, recovery_config()) {
        Ok(handle) => handle,
        Err(_) => {
            return ScenarioReport::from_checks("idle_gap", vec![check("owner_started", false)]);
        }
    };

    let initial = handle.refresh().await;
    sleep(Duration::from_millis(50)).await;
    let disconnected_before_refresh = disconnects.load(Ordering::SeqCst) == 1;
    let recovered = handle.refresh().await;
    let shutdown_clean = handle.shutdown().await.is_ok();

    ScenarioReport::from_checks(
        "idle_gap",
        vec![
            check(
                "initial_current",
                matches!(
                    initial,
                    Ok(ref value) if value.provenance == RefreshProvenance::CurrentRead
                ),
            ),
            check("idle_disconnect", disconnected_before_refresh),
            check(
                "new_current_generation",
                matches!(
                    (&initial, &recovered),
                    (Ok(initial), Ok(recovered))
                        if recovered.provenance == RefreshProvenance::CurrentRead
                            && recovered.generation > initial.generation
                ),
            ),
            check("exact_source_calls", calls.load(Ordering::SeqCst) == 2),
            check("shutdown_clean", shutdown_clean),
        ],
    )
}

async fn executable_change_requires_reselection(live: &StatusSnapshot) -> ScenarioReport {
    let (source, calls, _) = scripted([Ok(live.clone()), Err(RefreshFailure::ExecutableChanged)]);
    let handle = match RefreshHandle::start(source, recovery_config()) {
        Ok(handle) => handle,
        Err(_) => {
            return ScenarioReport::from_checks(
                "executable_changed",
                vec![check("owner_started", false)],
            );
        }
    };

    let initial = handle.refresh().await;
    let stale = handle.refresh().await;
    let blocked = handle.refresh().await;
    let shutdown_clean = handle.shutdown().await.is_ok();

    ScenarioReport::from_checks(
        "executable_changed",
        vec![
            check(
                "initial_current",
                matches!(
                    initial,
                    Ok(ref value) if value.provenance == RefreshProvenance::CurrentRead
                ),
            ),
            check(
                "stale_marks_identity_change",
                matches!(
                    stale,
                    Ok(ref value)
                        if value.provenance == RefreshProvenance::StaleFallback
                            && value.snapshot.data_status.reason_codes.iter().any(|code| {
                                code.as_str() == "codex_executable_changed"
                            })
                ),
            ),
            check(
                "replacement_not_retried",
                matches!(blocked, Err(RefreshOwnerError::Backoff { .. }))
                    && calls.load(Ordering::SeqCst) == 2,
            ),
            check("shutdown_clean", shutdown_clean),
        ],
    )
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let live = fixture_status();
    let scenarios = vec![
        transient_recovery("process_crash", RefreshFailure::ProcessFailed, &live).await,
        transient_recovery("network_timeout", RefreshFailure::Timeout, &live).await,
        transient_recovery("protocol_failure", RefreshFailure::ProtocolError, &live).await,
        boundary_recovery("account_change", RefreshFailure::AccountChanged, &live).await,
        boundary_recovery(
            "invalid_account_boundary",
            RefreshFailure::AccountBoundaryInvalid,
            &live,
        )
        .await,
        idle_gap_recovery(&live).await,
        executable_change_requires_reselection(&live).await,
    ];
    let passed_scenarios = scenarios
        .iter()
        .filter(|scenario| scenario.outcome == "pass")
        .count();
    let pass = passed_scenarios == scenarios.len();
    let report = RecoveryReport {
        schema_version: "1.0",
        outcome: if pass { "pass" } else { "failed" },
        scenario_count: scenarios.len(),
        passed_scenarios,
        scenarios,
        path_redacted: true,
    };

    if serde_json::to_writer_pretty(std::io::stdout(), &report).is_err() {
        eprintln!("capacity-recovery: failed to write the redacted report");
        return ExitCode::from(5);
    }
    println!();

    if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(5)
    }
}
