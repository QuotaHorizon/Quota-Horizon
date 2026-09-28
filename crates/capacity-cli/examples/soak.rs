use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use capacity_cli::soak::{SoakAccumulator, SoakExecutableProvenance, SoakRequestResult};
use clap::Parser;
use codex_runtime::app_server::RuntimeConfig;
use codex_runtime::discovery::{DiscoveryOutcome, discover_current};
use codex_runtime::refresh::{CodexStatusSource, RefreshHandle, RefreshOwnerConfig};
use tokio::time::{Instant, sleep_until};

#[derive(Debug, Parser)]
#[command(name = "capacity-soak")]
#[command(about = "Run the internal long-lived refresh acceptance harness")]
struct Arguments {
    #[arg(long, value_name = "PATH")]
    codex: PathBuf,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=28_800))]
    duration_seconds: u64,
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=3_600))]
    interval_seconds: u64,
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u64).range(1..=16))]
    concurrent_requests: u64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let discovery = discover_current(Some(arguments.codex));
    if discovery.outcome != DiscoveryOutcome::Selected {
        eprintln!(
            "capacity-soak: Codex executable discovery did not select one verified candidate"
        );
        return ExitCode::from(4);
    }
    let executable = match SoakExecutableProvenance::from_discovery(&discovery) {
        Some(executable) => executable,
        None => {
            eprintln!("capacity-soak: executable provenance was incomplete");
            return ExitCode::from(4);
        }
    };
    let source = match CodexStatusSource::from_discovery(&discovery, RuntimeConfig::default()) {
        Ok(source) => source,
        Err(_) => {
            eprintln!(
                "capacity-soak: the selected executable could not initialize a status source"
            );
            return ExitCode::from(4);
        }
    };
    let owner = match RefreshHandle::start(source, RefreshOwnerConfig::default()) {
        Ok(owner) => owner,
        Err(_) => {
            eprintln!("capacity-soak: the refresh owner could not start");
            return ExitCode::from(5);
        }
    };

    let started = Instant::now();
    let deadline = started + Duration::from_secs(arguments.duration_seconds);
    let interval = Duration::from_secs(arguments.interval_seconds);
    let mut next_round = started;
    let mut accumulator = SoakAccumulator::new(
        arguments.duration_seconds,
        arguments.interval_seconds,
        arguments.concurrent_requests,
    );

    loop {
        let round_started = Instant::now();
        let mut requests = Vec::with_capacity(
            usize::try_from(arguments.concurrent_requests)
                .expect("concurrency is bounded to sixteen"),
        );
        for _ in 0..arguments.concurrent_requests {
            let owner = owner.clone();
            requests.push(tokio::spawn(async move { owner.refresh().await }));
        }
        let mut responses = Vec::with_capacity(requests.len());
        for request in requests {
            match request.await {
                Ok(Ok(snapshot)) => responses.push(snapshot.into()),
                Ok(Err(error)) => responses.push(SoakRequestResult::OwnerError(error)),
                Err(_) => responses.push(SoakRequestResult::JoinError),
            }
        }
        accumulator.observe_round(
            started.elapsed().as_secs(),
            u64::try_from(round_started.elapsed().as_millis()).unwrap_or(u64::MAX),
            responses,
        );

        next_round += interval;
        if next_round > deadline {
            break;
        }
        sleep_until(next_round).await;
    }

    let latest = owner.latest().await.ok().flatten();
    let shutdown = owner.shutdown_with_diagnostics().await;
    let elapsed_milliseconds = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let report = accumulator.finish(elapsed_milliseconds, latest.as_ref(), shutdown, executable);
    if serde_json::to_writer_pretty(std::io::stdout(), &report).is_err() {
        eprintln!("capacity-soak: failed to write the redacted report");
        return ExitCode::from(5);
    }
    println!();

    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(5)
    }
}
