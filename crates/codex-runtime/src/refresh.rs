use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use capacity_domain::{
    Clock, CodexExecutableSnapshot, EnvironmentSnapshot, StatusSnapshot, SystemClock,
};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

use crate::app_server::{AppServerConnection, RuntimeConfig, RuntimeError};
use crate::discovery::{
    CandidateSource, CodexExecutableCandidate, DiscoveryOutcome, DiscoveryReport, HostPlatform,
    verify_executable_identity,
};
use crate::normalize::{LiveStatusContext, normalize_capacity_read};
use crate::{SessionSnapshotCache, StaleFallbackReason};

pub type RefreshFuture<'a> =
    Pin<Box<dyn Future<Output = Result<StatusSnapshot, RefreshFailure>> + Send + 'a>>;
pub type DisconnectFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), RefreshFailure>> + Send + 'a>>;

pub trait RefreshSource: Send + 'static {
    fn refresh(&mut self) -> RefreshFuture<'_>;
    fn disconnect(&mut self) -> DisconnectFuture<'_>;

    fn diagnostics(&self) -> RefreshSourceDiagnostics {
        RefreshSourceDiagnostics::default()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RefreshSourceDiagnostics {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshFailure {
    ProcessFailed,
    Timeout,
    ProtocolError,
    ExecutableChanged,
    AccountChanged,
    AccountBoundaryInvalid,
}

impl RefreshFailure {
    fn stale_fallback(self) -> Option<StaleFallbackReason> {
        match self {
            Self::ProcessFailed => Some(StaleFallbackReason::ProcessFailed),
            Self::Timeout => Some(StaleFallbackReason::Timeout),
            Self::ProtocolError => Some(StaleFallbackReason::ProtocolError),
            Self::ExecutableChanged => Some(StaleFallbackReason::ExecutableChanged),
            Self::AccountChanged | Self::AccountBoundaryInvalid => None,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RefreshSourceBuildError {
    #[error("discovery did not select a verified Codex executable")]
    DiscoveryNotSelected,
}

pub struct CodexStatusSource {
    executable: PathBuf,
    verified_executable: CodexExecutableCandidate,
    runtime_config: RuntimeConfig,
    environment: EnvironmentSnapshot,
    codex_executable: CodexExecutableSnapshot,
    clock: Arc<dyn Clock>,
    connection: Option<AppServerConnection>,
    diagnostics: RefreshSourceDiagnostics,
}

impl CodexStatusSource {
    pub fn from_discovery(
        discovery: &DiscoveryReport,
        runtime_config: RuntimeConfig,
    ) -> Result<Self, RefreshSourceBuildError> {
        Self::from_discovery_with_clock(discovery, runtime_config, Arc::new(SystemClock))
    }

    pub fn from_discovery_with_clock(
        discovery: &DiscoveryReport,
        runtime_config: RuntimeConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, RefreshSourceBuildError> {
        if discovery.outcome != DiscoveryOutcome::Selected {
            return Err(RefreshSourceBuildError::DiscoveryNotSelected);
        }
        let candidate = discovery
            .selected_candidate()
            .ok_or(RefreshSourceBuildError::DiscoveryNotSelected)?;
        let platform = discovery.platform.as_str();

        Ok(Self {
            executable: PathBuf::from(&candidate.canonical_path),
            verified_executable: candidate.clone(),
            runtime_config,
            environment: EnvironmentSnapshot {
                environment_id: format!("{platform}:local:{}", candidate.executable_id),
                platform: platform.to_owned(),
                architecture: discovery.architecture.clone(),
                boundary: if discovery.platform == HostPlatform::Unknown {
                    "unknown".to_owned()
                } else {
                    "native".to_owned()
                },
            },
            codex_executable: CodexExecutableSnapshot {
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
            },
            clock,
            connection: None,
            diagnostics: RefreshSourceDiagnostics::default(),
        })
    }

    async fn refresh_status(&mut self) -> Result<StatusSnapshot, RefreshFailure> {
        // Order observations by request start, not arrival. A slow app-server
        // response must not supersede a newer managed-account quota read.
        let captured_at = self.clock.now();
        self.diagnostics.refresh_attempts = self.diagnostics.refresh_attempts.saturating_add(1);
        if self.connection.is_none() {
            if verify_executable_identity(&self.verified_executable).is_err() {
                self.diagnostics.identity_verification_failures = self
                    .diagnostics
                    .identity_verification_failures
                    .saturating_add(1);
                return Err(RefreshFailure::ExecutableChanged);
            }
            self.diagnostics.connection_start_attempts =
                self.diagnostics.connection_start_attempts.saturating_add(1);
            match AppServerConnection::connect(&self.executable, self.runtime_config.clone()).await
            {
                Ok(connection) => {
                    self.diagnostics.connections_started =
                        self.diagnostics.connections_started.saturating_add(1);
                    self.connection = Some(connection);
                }
                Err(error) => {
                    self.diagnostics.connection_start_failures =
                        self.diagnostics.connection_start_failures.saturating_add(1);
                    return Err(classify_runtime_failure(error));
                }
            }
        }

        let read_result = self
            .connection
            .as_mut()
            .expect("connection was established above")
            .read_capacity()
            .await;
        match read_result {
            Ok(read) => {
                let context = LiveStatusContext {
                    captured_at,
                    environment: self.environment.clone(),
                    codex_executable: self.codex_executable.clone(),
                };
                match normalize_capacity_read(read, context) {
                    Ok(status) => Ok(status),
                    Err(_) => {
                        let _ = self.disconnect_status().await;
                        Err(RefreshFailure::ProtocolError)
                    }
                }
            }
            Err(error) => {
                let failure = classify_runtime_failure(error);
                let _ = self.disconnect_status().await;
                Err(failure)
            }
        }
    }

    async fn disconnect_status(&mut self) -> Result<(), RefreshFailure> {
        let Some(connection) = self.connection.take() else {
            return Ok(());
        };
        self.diagnostics.disconnect_attempts =
            self.diagnostics.disconnect_attempts.saturating_add(1);
        match connection.shutdown().await {
            Ok(()) => {
                self.diagnostics.disconnects_completed =
                    self.diagnostics.disconnects_completed.saturating_add(1);
                Ok(())
            }
            Err(error) => {
                self.diagnostics.disconnect_failures =
                    self.diagnostics.disconnect_failures.saturating_add(1);
                Err(classify_runtime_failure(error))
            }
        }
    }
}

impl RefreshSource for CodexStatusSource {
    fn refresh(&mut self) -> RefreshFuture<'_> {
        Box::pin(self.refresh_status())
    }

    fn disconnect(&mut self) -> DisconnectFuture<'_> {
        Box::pin(self.disconnect_status())
    }

    fn diagnostics(&self) -> RefreshSourceDiagnostics {
        let mut diagnostics = self.diagnostics.clone();
        diagnostics.connection_handle_active = self.connection.is_some();
        diagnostics
    }
}

fn classify_runtime_failure(error: RuntimeError) -> RefreshFailure {
    match error {
        RuntimeError::Timeout { .. } => RefreshFailure::Timeout,
        RuntimeError::AccountChanged => RefreshFailure::AccountChanged,
        RuntimeError::InvalidAccountNotification => RefreshFailure::AccountBoundaryInvalid,
        RuntimeError::Spawn
        | RuntimeError::PipeUnavailable
        | RuntimeError::Read
        | RuntimeError::Write
        | RuntimeError::UnexpectedEof
        | RuntimeError::PartialLineEof
        | RuntimeError::Shutdown => RefreshFailure::ProcessFailed,
        RuntimeError::InvalidConfiguration
        | RuntimeError::Encode
        | RuntimeError::OutboundLineTooLarge
        | RuntimeError::InboundLineTooLarge
        | RuntimeError::StdoutLimitExceeded
        | RuntimeError::MalformedJson
        | RuntimeError::InvalidMessage
        | RuntimeError::InvalidResponseId
        | RuntimeError::MismatchedResponseId
        | RuntimeError::DuplicateResponseId
        | RuntimeError::ServerRequestRejected
        | RuntimeError::Rpc { .. }
        | RuntimeError::InvalidResult { .. }
        | RuntimeError::RequestIdExhausted => RefreshFailure::ProtocolError,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshProvenance {
    CurrentRead,
    StaleFallback,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshSnapshot {
    pub generation: u64,
    pub provenance: RefreshProvenance,
    pub failure: Option<RefreshFailure>,
    pub snapshot: StatusSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshShutdownReport {
    pub failure: Option<RefreshFailure>,
    pub source: RefreshSourceDiagnostics,
}

impl RefreshShutdownReport {
    pub fn is_clean(&self) -> bool {
        self.failure.is_none()
            && self.source.disconnect_failures == 0
            && !self.source.connection_handle_active
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshOwnerConfig {
    pub idle_timeout: Duration,
    pub backoff_schedule: Vec<Duration>,
    pub jitter_percent: u8,
    pub command_capacity: usize,
}

impl Default for RefreshOwnerConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(60),
            backoff_schedule: vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(5),
                Duration::from_secs(15),
                Duration::from_secs(60),
            ],
            jitter_percent: 20,
            command_capacity: 32,
        }
    }
}

impl RefreshOwnerConfig {
    fn validate(&self) -> Result<(), RefreshOwnerError> {
        let schedule_valid = !self.backoff_schedule.is_empty()
            && self
                .backoff_schedule
                .iter()
                .all(|duration| !duration.is_zero())
            && self
                .backoff_schedule
                .windows(2)
                .all(|pair| pair[0] <= pair[1])
            && self
                .backoff_schedule
                .last()
                .is_some_and(|duration| *duration <= Duration::from_secs(60));
        if self.idle_timeout.is_zero()
            || !schedule_valid
            || self.jitter_percent > 50
            || self.command_capacity == 0
            || self.command_capacity > 1024
        {
            return Err(RefreshOwnerError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RefreshOwnerError {
    #[error("refresh owner configuration is invalid")]
    InvalidConfiguration,
    #[error("a Tokio runtime is required to start the refresh owner")]
    RuntimeUnavailable,
    #[error("the refresh owner has stopped")]
    Stopped,
    #[error("refresh source failed: {0:?}")]
    Source(RefreshFailure),
    #[error("refresh is in crash-loop backoff for another {retry_after_ms} ms")]
    Backoff { retry_after_ms: u64 },
}

#[derive(Clone)]
pub struct RefreshHandle {
    sender: mpsc::Sender<RefreshCommand>,
}

impl RefreshHandle {
    pub fn start<S>(source: S, config: RefreshOwnerConfig) -> Result<Self, RefreshOwnerError>
    where
        S: RefreshSource,
    {
        config.validate()?;
        tokio::runtime::Handle::try_current().map_err(|_| RefreshOwnerError::RuntimeUnavailable)?;
        let (sender, receiver) = mpsc::channel(config.command_capacity);
        tokio::spawn(run_owner(source, config, receiver));
        Ok(Self { sender })
    }

    pub async fn refresh(&self) -> Result<RefreshSnapshot, RefreshOwnerError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RefreshCommand::Refresh { reply })
            .await
            .map_err(|_| RefreshOwnerError::Stopped)?;
        result.await.map_err(|_| RefreshOwnerError::Stopped)?
    }

    pub async fn latest(&self) -> Result<Option<RefreshSnapshot>, RefreshOwnerError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RefreshCommand::Latest { reply })
            .await
            .map_err(|_| RefreshOwnerError::Stopped)?;
        result.await.map_err(|_| RefreshOwnerError::Stopped)
    }

    pub async fn shutdown(&self) -> Result<(), RefreshOwnerError> {
        let report = self.shutdown_with_diagnostics().await?;
        match report.failure {
            Some(failure) => Err(RefreshOwnerError::Source(failure)),
            None => Ok(()),
        }
    }

    pub async fn shutdown_with_diagnostics(
        &self,
    ) -> Result<RefreshShutdownReport, RefreshOwnerError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RefreshCommand::Shutdown { reply })
            .await
            .map_err(|_| RefreshOwnerError::Stopped)?;
        result.await.map_err(|_| RefreshOwnerError::Stopped)
    }
}

enum RefreshCommand {
    Refresh {
        reply: oneshot::Sender<Result<RefreshSnapshot, RefreshOwnerError>>,
    },
    Latest {
        reply: oneshot::Sender<Option<RefreshSnapshot>>,
    },
    Shutdown {
        reply: oneshot::Sender<RefreshShutdownReport>,
    },
}

struct RefreshState {
    cache: SessionSnapshotCache,
    generation: u64,
    latest: Option<RefreshSnapshot>,
    consecutive_failures: usize,
    next_attempt_at: Option<Instant>,
    jitter_seed: u64,
}

impl RefreshState {
    fn new() -> Self {
        let jitter_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u64)
            ^ u64::from(std::process::id());
        Self {
            cache: SessionSnapshotCache::new(),
            generation: 0,
            latest: None,
            consecutive_failures: 0,
            next_attempt_at: None,
            jitter_seed,
        }
    }

    fn backoff_error(&self, now: Instant) -> Option<RefreshOwnerError> {
        let deadline = self.next_attempt_at?;
        if deadline <= now {
            return None;
        }
        let millis = deadline.duration_since(now).as_millis().max(1);
        Some(RefreshOwnerError::Backoff {
            retry_after_ms: u64::try_from(millis).unwrap_or(u64::MAX),
        })
    }

    fn apply_result(
        &mut self,
        result: Result<StatusSnapshot, RefreshFailure>,
        config: &RefreshOwnerConfig,
    ) -> Result<RefreshSnapshot, RefreshOwnerError> {
        self.generation = self.generation.saturating_add(1);
        match result {
            Ok(snapshot) => {
                if self.cache.observe(snapshot.clone()).is_err() {
                    return self.apply_failure(RefreshFailure::ProtocolError, config);
                }
                self.consecutive_failures = 0;
                self.next_attempt_at = None;
                let outcome = RefreshSnapshot {
                    generation: self.generation,
                    provenance: RefreshProvenance::CurrentRead,
                    failure: None,
                    snapshot,
                };
                self.latest = Some(outcome.clone());
                Ok(outcome)
            }
            Err(failure) => self.apply_failure(failure, config),
        }
    }

    fn apply_failure(
        &mut self,
        failure: RefreshFailure,
        config: &RefreshOwnerConfig,
    ) -> Result<RefreshSnapshot, RefreshOwnerError> {
        if matches!(
            failure,
            RefreshFailure::AccountChanged | RefreshFailure::AccountBoundaryInvalid
        ) {
            self.cache.invalidate_account_change();
            self.latest = None;
        }
        self.schedule_backoff(config);
        if let Some(stale_reason) = failure.stale_fallback()
            && let Some(snapshot) = self.cache.stale_after_failure(stale_reason)
        {
            let outcome = RefreshSnapshot {
                generation: self.generation,
                provenance: RefreshProvenance::StaleFallback,
                failure: Some(failure),
                snapshot,
            };
            self.latest = Some(outcome.clone());
            return Ok(outcome);
        }
        Err(RefreshOwnerError::Source(failure))
    }

    fn schedule_backoff(&mut self, config: &RefreshOwnerConfig) {
        let index = self
            .consecutive_failures
            .min(config.backoff_schedule.len().saturating_sub(1));
        let base = config.backoff_schedule[index];
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.jitter_seed = xorshift(self.jitter_seed);
        let delay = jitter_down(base, config.jitter_percent, self.jitter_seed);
        self.next_attempt_at = Some(Instant::now() + delay);
    }
}

async fn run_owner<S>(
    mut source: S,
    config: RefreshOwnerConfig,
    mut receiver: mpsc::Receiver<RefreshCommand>,
) where
    S: RefreshSource,
{
    let mut state = RefreshState::new();
    let mut idle_deadline = None;

    loop {
        let idle = sleep_until(idle_deadline.unwrap_or_else(far_future));
        tokio::pin!(idle);
        tokio::select! {
            command = receiver.recv() => {
                let Some(command) = command else {
                    let _ = source.disconnect().await;
                    return;
                };
                match command {
                    RefreshCommand::Refresh { reply } => {
                        if let Some(error) = state.backoff_error(Instant::now()) {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                        let disposition = perform_coalesced_refresh(
                            &mut source,
                            &config,
                            &mut receiver,
                            &mut state,
                            reply,
                        ).await;
                        match disposition {
                            CoalescedDisposition::Continue => {
                                idle_deadline = Some(Instant::now() + config.idle_timeout);
                            }
                            CoalescedDisposition::ChannelClosed => {
                                let _ = source.disconnect().await;
                                return;
                            }
                            CoalescedDisposition::ShutdownHandled => return,
                        }
                    }
                    RefreshCommand::Latest { reply } => {
                        let _ = reply.send(state.latest.clone());
                    }
                    RefreshCommand::Shutdown { reply } => {
                        let report = disconnect_with_diagnostics(&mut source).await;
                        let _ = reply.send(report);
                        return;
                    }
                }
            }
            _ = &mut idle, if idle_deadline.is_some() => {
                idle_deadline = None;
                if let Err(failure) = source.disconnect().await {
                    let _ = state.apply_result(Err(failure), &config);
                }
            }
        }
    }
}

async fn perform_coalesced_refresh<S>(
    source: &mut S,
    config: &RefreshOwnerConfig,
    receiver: &mut mpsc::Receiver<RefreshCommand>,
    state: &mut RefreshState,
    first_reply: oneshot::Sender<Result<RefreshSnapshot, RefreshOwnerError>>,
) -> CoalescedDisposition
where
    S: RefreshSource,
{
    let mut waiters = VecDeque::from([first_reply]);
    let mut latest_waiters = Vec::new();
    let mut shutdown_replies = Vec::new();
    let mut channel_closed = false;
    let result = {
        let refresh = source.refresh();
        tokio::pin!(refresh);
        loop {
            tokio::select! {
                result = &mut refresh => break result,
                command = receiver.recv(), if !channel_closed => {
                    match command {
                        Some(RefreshCommand::Refresh { reply }) => waiters.push_back(reply),
                        Some(RefreshCommand::Latest { reply }) => {
                            latest_waiters.push(reply);
                        }
                        Some(RefreshCommand::Shutdown { reply }) => {
                            shutdown_replies.push(reply);
                        }
                        None => {
                            channel_closed = true;
                        }
                    }
                }
            }
        }
    };
    let outcome = state.apply_result(result, config);
    for waiter in waiters {
        let _ = waiter.send(outcome.clone());
    }
    for waiter in latest_waiters {
        let _ = waiter.send(state.latest.clone());
    }

    if !shutdown_replies.is_empty() {
        let report = disconnect_with_diagnostics(source).await;
        for reply in shutdown_replies {
            let _ = reply.send(report.clone());
        }
        return CoalescedDisposition::ShutdownHandled;
    }
    if channel_closed {
        CoalescedDisposition::ChannelClosed
    } else {
        CoalescedDisposition::Continue
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoalescedDisposition {
    Continue,
    ChannelClosed,
    ShutdownHandled,
}

async fn disconnect_with_diagnostics<S>(source: &mut S) -> RefreshShutdownReport
where
    S: RefreshSource,
{
    let failure = source.disconnect().await.err();
    RefreshShutdownReport {
        failure,
        source: source.diagnostics(),
    }
}

fn far_future() -> Instant {
    Instant::now() + Duration::from_secs(365 * 24 * 60 * 60)
}

fn xorshift(mut value: u64) -> u64 {
    if value == 0 {
        value = 0x9e37_79b9_7f4a_7c15;
    }
    value ^= value << 13;
    value ^= value >> 7;
    value ^ (value << 17)
}

fn jitter_down(base: Duration, percent: u8, seed: u64) -> Duration {
    if percent == 0 {
        return base;
    }
    let maximum_nanos = base.as_nanos().saturating_mul(u128::from(percent)) / 100;
    let jitter_nanos = u128::from(seed) % maximum_nanos.saturating_add(1);
    let nanos = base.as_nanos().saturating_sub(jitter_nanos);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Notify;
    use tokio::time::sleep;

    use super::*;

    const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/app-server");

    fn fixture_status(name: &str) -> StatusSnapshot {
        let bytes = std::fs::read(
            std::path::Path::new(FIXTURE_ROOT)
                .join(name)
                .join("expected-status.json"),
        )
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn test_config() -> RefreshOwnerConfig {
        RefreshOwnerConfig {
            idle_timeout: Duration::from_secs(1),
            backoff_schedule: vec![Duration::from_millis(100)],
            jitter_percent: 0,
            command_capacity: 16,
        }
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

        fn diagnostics(&self) -> RefreshSourceDiagnostics {
            let refresh_attempts =
                u64::try_from(self.calls.load(Ordering::SeqCst)).expect("test counter fits in u64");
            let disconnects = u64::try_from(self.disconnects.load(Ordering::SeqCst))
                .expect("test counter fits in u64");
            RefreshSourceDiagnostics {
                refresh_attempts,
                disconnect_attempts: disconnects,
                disconnects_completed: disconnects,
                ..RefreshSourceDiagnostics::default()
            }
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

    #[tokio::test]
    async fn transient_failure_returns_cached_snapshot_then_enforces_backoff() {
        let live = fixture_status("plus-normal");
        let original_windows = live.quota.windows.clone();
        let (source, calls, _) = scripted([Ok(live), Err(RefreshFailure::Timeout)]);
        let handle = RefreshHandle::start(source, test_config()).unwrap();

        let first = handle.refresh().await.unwrap();
        let stale = handle.refresh().await.unwrap();
        let blocked = handle.refresh().await.unwrap_err();

        assert_eq!(first.provenance, RefreshProvenance::CurrentRead);
        assert_eq!(first.failure, None);
        assert_eq!(stale.provenance, RefreshProvenance::StaleFallback);
        assert_eq!(stale.failure, Some(RefreshFailure::Timeout));
        assert_eq!(stale.snapshot.quota.windows, original_windows);
        assert!(matches!(blocked, RefreshOwnerError::Backoff { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn transient_failures_recover_with_a_new_current_generation() {
        for failure in [
            RefreshFailure::ProcessFailed,
            RefreshFailure::Timeout,
            RefreshFailure::ProtocolError,
        ] {
            let live = fixture_status("plus-normal");
            let (source, calls, _) = scripted([Ok(live.clone()), Err(failure), Ok(live)]);
            let handle = RefreshHandle::start(source, test_config()).unwrap();

            let initial = handle.refresh().await.unwrap();
            let stale = handle.refresh().await.unwrap();
            let blocked = handle.refresh().await.unwrap_err();
            sleep(Duration::from_millis(120)).await;
            let recovered = handle.refresh().await.unwrap();

            assert_eq!(initial.provenance, RefreshProvenance::CurrentRead);
            assert_eq!(initial.failure, None);
            assert_eq!(stale.provenance, RefreshProvenance::StaleFallback);
            assert_eq!(stale.failure, Some(failure));
            assert!(matches!(blocked, RefreshOwnerError::Backoff { .. }));
            assert_eq!(recovered.provenance, RefreshProvenance::CurrentRead);
            assert!(recovered.generation > stale.generation);
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            handle.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn executable_change_keeps_only_a_stale_snapshot_until_reselection() {
        let live = fixture_status("plus-normal");
        let (source, calls, _) = scripted([Ok(live), Err(RefreshFailure::ExecutableChanged)]);
        let handle = RefreshHandle::start(source, test_config()).unwrap();

        handle.refresh().await.unwrap();
        let stale = handle.refresh().await.unwrap();

        assert_eq!(stale.provenance, RefreshProvenance::StaleFallback);
        assert!(
            stale
                .snapshot
                .data_status
                .reason_codes
                .iter()
                .any(|code| { code.as_str() == "codex_executable_changed" })
        );
        assert!(matches!(
            handle.refresh().await.unwrap_err(),
            RefreshOwnerError::Backoff { .. }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn account_boundary_failures_remove_cached_and_latest_snapshots() {
        for failure in [
            RefreshFailure::AccountChanged,
            RefreshFailure::AccountBoundaryInvalid,
        ] {
            let (source, _, _) = scripted([Ok(fixture_status("plus-normal")), Err(failure)]);
            let handle = RefreshHandle::start(source, test_config()).unwrap();

            handle.refresh().await.unwrap();
            let error = handle.refresh().await.unwrap_err();

            assert_eq!(error, RefreshOwnerError::Source(failure));
            assert!(handle.latest().await.unwrap().is_none());
            handle.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn account_boundary_recovers_only_from_a_new_live_snapshot() {
        for failure in [
            RefreshFailure::AccountChanged,
            RefreshFailure::AccountBoundaryInvalid,
        ] {
            let live = fixture_status("plus-normal");
            let (source, calls, _) = scripted([Ok(live.clone()), Err(failure), Ok(live)]);
            let handle = RefreshHandle::start(source, test_config()).unwrap();

            handle.refresh().await.unwrap();
            assert_eq!(
                handle.refresh().await.unwrap_err(),
                RefreshOwnerError::Source(failure)
            );
            assert!(handle.latest().await.unwrap().is_none());
            assert!(matches!(
                handle.refresh().await.unwrap_err(),
                RefreshOwnerError::Backoff { .. }
            ));
            sleep(Duration::from_millis(120)).await;
            let recovered = handle.refresh().await.unwrap();

            assert_eq!(recovered.provenance, RefreshProvenance::CurrentRead);
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            handle.shutdown().await.unwrap();
        }
    }

    struct BlockingSource {
        status: StatusSnapshot,
        calls: Arc<AtomicUsize>,
        disconnects: Arc<AtomicUsize>,
        started: Option<oneshot::Sender<()>>,
        release: Arc<Notify>,
    }

    impl RefreshSource for BlockingSource {
        fn refresh(&mut self) -> RefreshFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self.started.take() {
                let _ = started.send(());
            }
            let release = Arc::clone(&self.release);
            let status = self.status.clone();
            Box::pin(async move {
                release.notified().await;
                Ok(status)
            })
        }

        fn disconnect(&mut self) -> DisconnectFuture<'_> {
            self.disconnects.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn simultaneous_refreshes_share_one_source_read() {
        let calls = Arc::new(AtomicUsize::new(0));
        let disconnects = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let (started, did_start) = oneshot::channel();
        let source = BlockingSource {
            status: fixture_status("plus-normal"),
            calls: Arc::clone(&calls),
            disconnects: Arc::clone(&disconnects),
            started: Some(started),
            release: Arc::clone(&release),
        };
        let handle = RefreshHandle::start(source, test_config()).unwrap();
        let first_handle = handle.clone();
        let first = tokio::spawn(async move { first_handle.refresh().await.unwrap() });
        did_start.await.unwrap();
        let second_handle = handle.clone();
        let second = tokio::spawn(async move { second_handle.refresh().await.unwrap() });
        sleep(Duration::from_millis(10)).await;
        release.notify_one();

        let first = first.await.unwrap();
        let second = second.await.unwrap();
        assert_eq!(first.generation, second.generation);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        handle.shutdown().await.unwrap();
        assert_eq!(disconnects.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn shutdown_during_refresh_reports_diagnostics_and_disconnects_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let disconnects = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let (started, did_start) = oneshot::channel();
        let source = BlockingSource {
            status: fixture_status("plus-normal"),
            calls: Arc::clone(&calls),
            disconnects: Arc::clone(&disconnects),
            started: Some(started),
            release: Arc::clone(&release),
        };
        let handle = RefreshHandle::start(source, test_config()).unwrap();
        let refresh_handle = handle.clone();
        let refresh = tokio::spawn(async move { refresh_handle.refresh().await });
        did_start.await.unwrap();
        let shutdown_handle = handle.clone();
        let shutdown =
            tokio::spawn(async move { shutdown_handle.shutdown_with_diagnostics().await });
        sleep(Duration::from_millis(10)).await;

        release.notify_one();
        assert!(refresh.await.unwrap().is_ok());
        let report = shutdown.await.unwrap().unwrap();

        assert!(report.is_clean());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(disconnects.load(Ordering::SeqCst), 1);
    }

    struct BoundarySource {
        live: Option<StatusSnapshot>,
        started: Option<oneshot::Sender<()>>,
        release: Arc<Notify>,
    }

    impl RefreshSource for BoundarySource {
        fn refresh(&mut self) -> RefreshFuture<'_> {
            if let Some(live) = self.live.take() {
                return Box::pin(async move { Ok(live) });
            }
            if let Some(started) = self.started.take() {
                let _ = started.send(());
            }
            let release = Arc::clone(&self.release);
            Box::pin(async move {
                release.notified().await;
                Err(RefreshFailure::AccountChanged)
            })
        }

        fn disconnect(&mut self) -> DisconnectFuture<'_> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn latest_waits_for_in_flight_account_boundary_resolution() {
        let release = Arc::new(Notify::new());
        let (started, did_start) = oneshot::channel();
        let source = BoundarySource {
            live: Some(fixture_status("plus-normal")),
            started: Some(started),
            release: Arc::clone(&release),
        };
        let handle = RefreshHandle::start(source, test_config()).unwrap();
        handle.refresh().await.unwrap();
        let refresh_handle = handle.clone();
        let refresh = tokio::spawn(async move { refresh_handle.refresh().await });
        did_start.await.unwrap();
        let latest_handle = handle.clone();
        let latest = tokio::spawn(async move { latest_handle.latest().await.unwrap() });
        sleep(Duration::from_millis(10)).await;

        assert!(!latest.is_finished());
        release.notify_one();
        assert_eq!(
            refresh.await.unwrap().unwrap_err(),
            RefreshOwnerError::Source(RefreshFailure::AccountChanged)
        );
        assert!(latest.await.unwrap().is_none());
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn idle_deadline_disconnects_the_source() {
        let (source, _, disconnects) = scripted([Ok(fixture_status("plus-normal"))]);
        let config = RefreshOwnerConfig {
            idle_timeout: Duration::from_millis(20),
            ..test_config()
        };
        let handle = RefreshHandle::start(source, config).unwrap();

        handle.refresh().await.unwrap();
        sleep(Duration::from_millis(50)).await;

        assert_eq!(disconnects.load(Ordering::SeqCst), 1);
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn refresh_after_an_idle_gap_reconnects_and_returns_current_data() {
        let live = fixture_status("plus-normal");
        let (source, calls, disconnects) = scripted([Ok(live.clone()), Ok(live)]);
        let config = RefreshOwnerConfig {
            idle_timeout: Duration::from_millis(20),
            ..test_config()
        };
        let handle = RefreshHandle::start(source, config).unwrap();

        let initial = handle.refresh().await.unwrap();
        sleep(Duration::from_millis(50)).await;
        let recovered = handle.refresh().await.unwrap();

        assert_eq!(initial.provenance, RefreshProvenance::CurrentRead);
        assert_eq!(recovered.provenance, RefreshProvenance::CurrentRead);
        assert!(recovered.generation > initial.generation);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(disconnects.load(Ordering::SeqCst), 1);
        handle.shutdown().await.unwrap();
    }

    #[test]
    fn default_backoff_uses_the_frozen_bounded_schedule() {
        let config = RefreshOwnerConfig::default();
        assert_eq!(
            config.backoff_schedule,
            [1, 2, 5, 15, 60].map(Duration::from_secs)
        );
        assert_eq!(config.jitter_percent, 20);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn jitter_never_exceeds_the_base_or_configured_range() {
        let base = Duration::from_secs(10);
        for seed in [0, 1, u64::MAX / 2, u64::MAX] {
            let delay = jitter_down(base, 20, seed);
            assert!(delay <= base);
            assert!(delay >= Duration::from_secs(8));
        }
    }
}
