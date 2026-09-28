mod jsonrpc;
mod model;
mod process;

use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncWrite};

use jsonrpc::JsonlSession;
pub use model::{
    AccountInfo, AccountReadResult, CapacityRead, CreditsSnapshot, InitializeResult,
    NotificationSummary, OptionalUsageRead, RateLimitResetCredit, RateLimitResetCreditSummary,
    RateLimitSnapshot, RateLimitWindow, RateLimitsReadResult, TokenUsageSummary, UsageReadResult,
    UsageUnavailableReason,
};
pub use process::AppServerConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub idle_child_timeout: Duration,
    pub max_jsonl_line_bytes: usize,
    pub max_stdout_bytes: usize,
    pub stderr_ring_bytes: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            startup_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(10),
            idle_child_timeout: Duration::from_secs(60),
            max_jsonl_line_bytes: 1024 * 1024,
            max_stdout_bytes: 2 * 1024 * 1024,
            stderr_ring_bytes: 64 * 1024,
        }
    }
}

impl RuntimeConfig {
    fn validate(&self) -> Result<(), RuntimeError> {
        if self.startup_timeout.is_zero()
            || self.request_timeout.is_zero()
            || self.idle_child_timeout.is_zero()
            || self.max_jsonl_line_bytes == 0
            || self.max_stdout_bytes < self.max_jsonl_line_bytes
            || self.stderr_ring_bytes == 0
        {
            return Err(RuntimeError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("app-server runtime configuration is invalid")]
    InvalidConfiguration,
    #[error("the selected Codex app-server could not be started")]
    Spawn,
    #[error("the app-server process did not expose all required stdio pipes")]
    PipeUnavailable,
    #[error("an app-server request could not be encoded")]
    Encode,
    #[error("an app-server request could not be written")]
    Write,
    #[error("app-server stdout could not be read")]
    Read,
    #[error("app-server timed out during {phase}")]
    Timeout { phase: &'static str },
    #[error("an outbound JSONL line exceeded the configured limit")]
    OutboundLineTooLarge,
    #[error("an inbound JSONL line exceeded the configured limit")]
    InboundLineTooLarge,
    #[error("app-server stdout exceeded the configured session limit")]
    StdoutLimitExceeded,
    #[error("app-server closed stdout before sending a response")]
    UnexpectedEof,
    #[error("app-server closed stdout in the middle of a JSONL frame")]
    PartialLineEof,
    #[error("app-server returned malformed JSON")]
    MalformedJson,
    #[error("app-server returned an invalid JSON-RPC message")]
    InvalidMessage,
    #[error("app-server returned a non-numeric response ID")]
    InvalidResponseId,
    #[error("app-server returned a response for a different request ID")]
    MismatchedResponseId,
    #[error("app-server returned a duplicate response ID")]
    DuplicateResponseId,
    #[error("app-server initiated a request that this read-only client does not execute")]
    ServerRequestRejected,
    #[error("app-server returned RPC error {code} for {method}")]
    Rpc { method: &'static str, code: i64 },
    #[error("app-server returned an invalid result shape for {method}")]
    InvalidResult { method: &'static str },
    #[error("the Codex account changed while a snapshot was being read")]
    AccountChanged,
    #[error("the app-server returned a malformed account-change notification")]
    InvalidAccountNotification,
    #[error("the app-server request ID space was exhausted")]
    RequestIdExhausted,
    #[error("the app-server child could not be terminated and reaped")]
    Shutdown,
}

pub async fn read_codex_capacity(
    executable: &Path,
    config: RuntimeConfig,
) -> Result<CapacityRead, RuntimeError> {
    match process::read_codex_process(executable, config.clone()).await {
        Err(RuntimeError::AccountChanged) => process::read_codex_process(executable, config).await,
        result => result,
    }
}

#[cfg(test)]
async fn read_from_session<R, W>(
    session: &mut JsonlSession<R, W>,
    config: &RuntimeConfig,
) -> Result<CapacityRead, RuntimeError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let initialize = initialize_session(session, config).await?;
    read_initialized_session(session, config, initialize).await
}

async fn initialize_session<R, W>(
    session: &mut JsonlSession<R, W>,
    config: &RuntimeConfig,
) -> Result<InitializeResult, RuntimeError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let initialize = request_typed(
        session,
        "initialize",
        Some(json!({
            "clientInfo": {
                "name": "codex_capacity_planner",
                "title": "Codex Capacity Planner",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
        config.startup_timeout,
    )
    .await?;
    session.notify("initialized", json!({})).await?;
    Ok(initialize)
}

async fn read_initialized_session<R, W>(
    session: &mut JsonlSession<R, W>,
    config: &RuntimeConfig,
    initialize: InitializeResult,
) -> Result<CapacityRead, RuntimeError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    session.reset_stdout_budget();

    let mut account: AccountReadResult = request_typed(
        session,
        "account/read",
        Some(json!({ "refreshToken": false })),
        config.request_timeout,
    )
    .await?;
    if session.take_account_change_pending() {
        if session.account_baseline_established {
            return Err(RuntimeError::AccountChanged);
        }
        // Recent Codex versions publish their initial auth state during the
        // first account/read. No quota has been read yet. Discard that account
        // response and establish a quiet baseline on this same child, once.
        // Never infer identity from authMode/planType or ignore a notification
        // after the baseline (including on a later refresh).
        account = request_typed(
            session,
            "account/read",
            Some(json!({ "refreshToken": false })),
            config.request_timeout,
        )
        .await?;
        if session.take_account_change_pending() {
            return Err(RuntimeError::AccountChanged);
        }
    }
    session.account_baseline_established = true;

    if account
        .account
        .as_ref()
        .map(|account| account.account_type.as_str())
        != Some("chatgpt")
    {
        return Ok(CapacityRead {
            initialize,
            account,
            rate_limits: None,
            usage: OptionalUsageRead::Unavailable(UsageUnavailableReason::NotApplicable),
            notifications: session.take_notifications(),
            stderr_captured_bytes: 0,
            stderr_truncated: false,
        });
    }

    let mut rate_limits: RateLimitsReadResult = request_typed(
        session,
        "account/rateLimits/read",
        None,
        config.request_timeout,
    )
    .await?;
    if session.take_account_change_pending() {
        return Err(RuntimeError::AccountChanged);
    }

    let usage = match session
        .request("account/usage/read", None, config.request_timeout)
        .await
    {
        Ok(value) => match parse_result::<UsageReadResult>(value, "account/usage/read") {
            Ok(value) => OptionalUsageRead::Available(value),
            Err(_) => OptionalUsageRead::Unavailable(UsageUnavailableReason::InvalidResponse),
        },
        Err(RuntimeError::Rpc { code: -32601, .. }) => {
            OptionalUsageRead::Unavailable(UsageUnavailableReason::MethodUnavailable)
        }
        Err(
            RuntimeError::Rpc { .. }
            | RuntimeError::Timeout {
                phase: "account/usage/read",
            }
            | RuntimeError::Read
            | RuntimeError::Write
            | RuntimeError::UnexpectedEof
            | RuntimeError::PartialLineEof,
        ) => OptionalUsageRead::Unavailable(UsageUnavailableReason::ReadFailed),
        Err(error) => return Err(error),
    };
    if session.take_account_change_pending() {
        return Err(RuntimeError::AccountChanged);
    }

    if session.take_rate_limit_refresh_pending() {
        rate_limits = request_typed(
            session,
            "account/rateLimits/read",
            None,
            config.request_timeout,
        )
        .await?;
        if session.take_account_change_pending() {
            return Err(RuntimeError::AccountChanged);
        }
    }

    Ok(CapacityRead {
        initialize,
        account,
        rate_limits: Some(rate_limits),
        usage,
        notifications: session.take_notifications(),
        stderr_captured_bytes: 0,
        stderr_truncated: false,
    })
}

async fn request_typed<R, W, T>(
    session: &mut JsonlSession<R, W>,
    method: &'static str,
    params: Option<Value>,
    request_timeout: Duration,
) -> Result<T, RuntimeError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    T: DeserializeOwned,
{
    let value = session.request(method, params, request_timeout).await?;
    parse_result(value, method)
}

fn parse_result<T: DeserializeOwned>(
    value: Value,
    method: &'static str,
) -> Result<T, RuntimeError> {
    serde_json::from_value(value).map_err(|_| RuntimeError::InvalidResult { method })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use fake_app_server::{Scenario, ScenarioAction, ScenarioStep, run_scenario};
    use serde_json::json;
    use tokio::io::{BufReader, duplex, split};

    use super::*;

    const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/app-server");

    fn fixture(name: &str) -> Scenario {
        Scenario::from_directory(Path::new(FIXTURE_ROOT).join(name).as_path()).unwrap()
    }

    pub(crate) async fn read_scenario(
        scenario: Scenario,
        config: RuntimeConfig,
    ) -> Result<CapacityRead, RuntimeError> {
        let (client, server) = duplex(64 * 1024);
        let (client_reader, client_writer) = split(client);
        let (server_reader, mut server_writer) = split(server);
        let server_task = tokio::spawn(async move {
            run_scenario(&scenario, BufReader::new(server_reader), &mut server_writer).await
        });
        let mut session =
            JsonlSession::new(BufReader::new(client_reader), client_writer, config.clone());

        let result = read_from_session(&mut session, &config).await;
        drop(session);
        server_task.abort();
        result
    }

    fn refresh_steps(used_percent: f64, include_unknown_notification: bool) -> Vec<ScenarioStep> {
        let usage_action = if include_unknown_notification {
            ScenarioAction::NotifyThenRespond {
                method: "fixture/unknown".to_owned(),
                params: json!({ "ignored": true }),
                result: json!({ "summary": null }),
            }
        } else {
            ScenarioAction::Respond {
                result: json!({ "summary": null }),
            }
        };

        vec![
            ScenarioStep {
                expect_method: "account/read".to_owned(),
                action: ScenarioAction::Respond {
                    result: json!({
                        "account": { "type": "chatgpt", "planType": "plus" },
                        "requiresOpenaiAuth": true
                    }),
                },
            },
            ScenarioStep {
                expect_method: "account/rateLimits/read".to_owned(),
                action: ScenarioAction::Respond {
                    result: json!({
                        "rateLimits": {
                            "limitId": "codex",
                            "primary": {
                                "usedPercent": used_percent,
                                "windowDurationMins": 300,
                                "resetsAt": 1788066000
                            },
                            "secondary": null
                        },
                        "rateLimitsByLimitId": null,
                        "rateLimitResetCredits": null
                    }),
                },
            },
            ScenarioStep {
                expect_method: "account/usage/read".to_owned(),
                action: usage_action,
            },
        ]
    }

    #[tokio::test]
    async fn initialized_session_supports_multiple_reads_and_per_read_notifications() {
        let mut steps = vec![
            ScenarioStep {
                expect_method: "initialize".to_owned(),
                action: ScenarioAction::Respond {
                    result: json!({
                        "userAgent": "codex-cli/0.150.0-alpha.12.2",
                        "platformFamily": "unix",
                        "platformOs": "macos"
                    }),
                },
            },
            ScenarioStep {
                expect_method: "initialized".to_owned(),
                action: ScenarioAction::NoResponse { delay_ms: 0 },
            },
        ];
        steps.extend(refresh_steps(10.0, true));
        steps.extend(refresh_steps(25.0, false));
        let scenario = Scenario {
            schema_version: "1.0".to_owned(),
            name: "persistent-session".to_owned(),
            steps,
        };
        let config = RuntimeConfig::default();
        let (client, server) = duplex(64 * 1024);
        let (client_reader, client_writer) = split(client);
        let (server_reader, mut server_writer) = split(server);
        let server_task = tokio::spawn(async move {
            run_scenario(&scenario, BufReader::new(server_reader), &mut server_writer).await
        });
        let mut session =
            JsonlSession::new(BufReader::new(client_reader), client_writer, config.clone());

        let initialize = initialize_session(&mut session, &config).await.unwrap();
        let first = read_initialized_session(&mut session, &config, initialize.clone())
            .await
            .unwrap();
        let second = read_initialized_session(&mut session, &config, initialize)
            .await
            .unwrap();

        assert_eq!(
            first
                .rate_limits
                .unwrap()
                .rate_limits
                .primary
                .unwrap()
                .used_percent,
            10.0
        );
        assert_eq!(first.notifications.unknown_notifications, 1);
        assert_eq!(
            second
                .rate_limits
                .unwrap()
                .rate_limits
                .primary
                .unwrap()
                .used_percent,
            25.0
        );
        assert_eq!(second.notifications, NotificationSummary::default());
        drop(session);
        server_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn plus_fixture_completes_the_read_only_sequence() {
        let read = read_scenario(fixture("plus-normal"), RuntimeConfig::default())
            .await
            .unwrap();

        assert_eq!(
            read.account
                .account
                .as_ref()
                .map(|account| account.account_type.as_str()),
            Some("chatgpt")
        );
        assert!(read.rate_limits.is_some());
        assert_eq!(
            read.usage,
            OptionalUsageRead::Unavailable(UsageUnavailableReason::MethodUnavailable)
        );
    }

    #[tokio::test]
    async fn malformed_fixture_fails_without_a_snapshot() {
        let error = read_scenario(fixture("malformed-response"), RuntimeConfig::default())
            .await
            .unwrap_err();

        assert!(matches!(error, RuntimeError::MalformedJson));
    }

    #[tokio::test]
    async fn timeout_fixture_is_bounded() {
        let config = RuntimeConfig {
            startup_timeout: Duration::from_millis(20),
            ..RuntimeConfig::default()
        };

        let error = read_scenario(fixture("process-timeout"), config)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            RuntimeError::Timeout {
                phase: "initialize"
            }
        ));
    }

    #[tokio::test]
    async fn wrong_response_id_fails_closed() {
        let error = read_scenario(fixture("wrong-request-id"), RuntimeConfig::default())
            .await
            .unwrap_err();

        assert!(matches!(error, RuntimeError::MismatchedResponseId));
    }

    #[tokio::test]
    async fn duplicate_response_id_fails_closed() {
        let scenario = Scenario {
            schema_version: "1.0".to_owned(),
            name: "duplicate-id".to_owned(),
            steps: vec![
                ScenarioStep {
                    expect_method: "initialize".to_owned(),
                    action: ScenarioAction::Respond { result: json!({}) },
                },
                ScenarioStep {
                    expect_method: "initialized".to_owned(),
                    action: ScenarioAction::NoResponse { delay_ms: 0 },
                },
                ScenarioStep {
                    expect_method: "account/read".to_owned(),
                    action: ScenarioAction::RawLine {
                        line: "{\"id\":0,\"result\":{}}".to_owned(),
                    },
                },
            ],
        };

        let error = read_scenario(scenario, RuntimeConfig::default())
            .await
            .unwrap_err();

        assert!(matches!(error, RuntimeError::DuplicateResponseId));
    }

    #[tokio::test]
    async fn oversized_line_fails_before_json_parsing() {
        let config = RuntimeConfig {
            max_jsonl_line_bytes: 256,
            max_stdout_bytes: 1024,
            ..RuntimeConfig::default()
        };

        let error = read_scenario(fixture("oversized-line"), config)
            .await
            .unwrap_err();

        assert!(matches!(error, RuntimeError::InboundLineTooLarge));
    }

    #[tokio::test]
    async fn partial_jsonl_frame_at_eof_fails_closed() {
        let error = read_scenario(fixture("early-eof"), RuntimeConfig::default())
            .await
            .unwrap_err();

        assert!(matches!(error, RuntimeError::PartialLineEof));
    }

    #[tokio::test]
    async fn rate_limit_notifications_merge_into_one_refresh() {
        let read = read_scenario(fixture("bucket-disappeared"), RuntimeConfig::default())
            .await
            .unwrap();

        let rate_limits = read.rate_limits.unwrap();
        assert_eq!(rate_limits.rate_limits.primary.unwrap().used_percent, 11.0);
        assert_eq!(rate_limits.rate_limits_by_limit_id.unwrap().len(), 1);
        assert_eq!(read.notifications.rate_limit_updates, 1);
        assert!(matches!(read.usage, OptionalUsageRead::Available(_)));
    }

    #[tokio::test]
    async fn startup_account_notification_rechecks_before_reading_any_quota() {
        let mut scenario = fixture("plus-normal");
        scenario.steps.truncate(2);
        scenario.steps.push(startup_account_step());
        scenario.steps.extend(refresh_steps(18.0, false));
        let read = read_scenario(scenario, RuntimeConfig::default())
            .await
            .unwrap();
        // The first response is deliberately different and must be discarded.
        assert_eq!(
            read.account.account.unwrap().plan_type.as_deref(),
            Some("plus")
        );
        assert_eq!(read.notifications.account_updates, 1);
        assert_eq!(
            read.rate_limits
                .unwrap()
                .rate_limits
                .primary
                .unwrap()
                .used_percent,
            18.0
        );
    }

    fn startup_account_step() -> ScenarioStep {
        ScenarioStep {
            expect_method: "account/read".to_owned(),
            action: ScenarioAction::NotifyThenRespond {
                method: "account/updated".to_owned(),
                params: json!({ "authMode": "chatgpt", "planType": "plus" }),
                result: json!({ "account": { "type": "chatgpt", "planType": "pro" } }),
            },
        }
    }

    #[tokio::test]
    async fn repeated_startup_changes_fail_closed_without_reading_quota() {
        let mut scenario = fixture("plus-normal");
        scenario.steps.truncate(2);
        scenario
            .steps
            .extend([startup_account_step(), startup_account_step()]);
        assert_eq!(
            read_scenario(scenario, RuntimeConfig::default())
                .await
                .unwrap_err(),
            RuntimeError::AccountChanged
        );
    }

    #[tokio::test]
    async fn a_notification_on_a_later_account_read_is_not_startup() {
        let mut scenario = fixture("plus-normal");
        scenario.steps.truncate(2);
        scenario.steps.extend(refresh_steps(10.0, false));
        scenario.steps.push(startup_account_step());
        let config = RuntimeConfig::default();
        let (client, server) = duplex(64 * 1024);
        let (reader, writer) = split(client);
        let (server_reader, mut server_writer) = split(server);
        let task = tokio::spawn(async move {
            run_scenario(&scenario, BufReader::new(server_reader), &mut server_writer).await
        });
        let mut session = JsonlSession::new(BufReader::new(reader), writer, config.clone());
        let initialized = initialize_session(&mut session, &config).await.unwrap();
        read_initialized_session(&mut session, &config, initialized.clone())
            .await
            .unwrap();
        assert_eq!(
            read_initialized_session(&mut session, &config, initialized)
                .await
                .unwrap_err(),
            RuntimeError::AccountChanged
        );
        drop(session);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn account_update_invalidates_the_in_flight_snapshot() {
        let error = read_scenario(
            fixture("account-updated-during-read"),
            RuntimeConfig::default(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, RuntimeError::AccountChanged);
    }

    #[tokio::test]
    async fn malformed_account_update_fails_closed() {
        let scenario = Scenario {
            schema_version: "1.0".to_owned(),
            name: "malformed-account-update".to_owned(),
            steps: vec![
                ScenarioStep {
                    expect_method: "initialize".to_owned(),
                    action: ScenarioAction::Respond { result: json!({}) },
                },
                ScenarioStep {
                    expect_method: "initialized".to_owned(),
                    action: ScenarioAction::NoResponse { delay_ms: 0 },
                },
                ScenarioStep {
                    expect_method: "account/read".to_owned(),
                    action: ScenarioAction::NotifyThenRespond {
                        method: "account/updated".to_owned(),
                        params: json!({ "authMode": ["chatgpt"] }),
                        result: json!({
                            "account": { "type": "chatgpt", "planType": "plus" },
                            "requiresOpenaiAuth": true
                        }),
                    },
                },
            ],
        };

        let error = read_scenario(scenario, RuntimeConfig::default())
            .await
            .unwrap_err();

        assert_eq!(error, RuntimeError::InvalidAccountNotification);
    }

    #[tokio::test]
    async fn malformed_notification_during_optional_usage_is_not_downgraded() {
        let mut scenario = fixture("plus-normal");
        let usage_step = scenario
            .steps
            .iter_mut()
            .find(|step| step.expect_method == "account/usage/read")
            .unwrap();
        usage_step.action = ScenarioAction::NotifyThenRespond {
            method: "account/updated".to_owned(),
            params: json!({ "planType": ["plus"] }),
            result: json!({ "summary": null }),
        };

        let error = read_scenario(scenario, RuntimeConfig::default())
            .await
            .unwrap_err();

        assert_eq!(error, RuntimeError::InvalidAccountNotification);
    }

    #[tokio::test]
    async fn multi_bucket_fixture_preserves_unknown_fields_and_bucket_ids() {
        let read = read_scenario(fixture("pro-multi-bucket"), RuntimeConfig::default())
            .await
            .unwrap();
        let rate_limits = read.rate_limits.unwrap();
        let buckets = rate_limits.rate_limits_by_limit_id.unwrap();

        assert_eq!(buckets.len(), 2);
        assert_eq!(
            buckets["codex_future"]
                .primary
                .as_ref()
                .unwrap()
                .used_percent,
            12.5
        );
        assert_eq!(
            rate_limits
                .rate_limit_reset_credits
                .unwrap()
                .available_count,
            2
        );
        assert!(matches!(read.usage, OptionalUsageRead::Available(_)));
        assert_eq!(read.notifications.unknown_notifications, 1);
    }

    #[tokio::test]
    async fn weekly_only_fixture_does_not_invent_a_second_window() {
        let read = read_scenario(fixture("weekly-only"), RuntimeConfig::default())
            .await
            .unwrap();
        let rate_limits = read.rate_limits.unwrap();

        assert_eq!(
            rate_limits
                .rate_limits
                .primary
                .as_ref()
                .unwrap()
                .window_duration_mins,
            Some(10_080)
        );
        assert!(rate_limits.rate_limits.secondary.is_none());
        assert!(rate_limits.rate_limits_by_limit_id.is_none());
    }

    #[tokio::test]
    async fn count_only_reset_credit_fixture_preserves_authoritative_count() {
        let read = read_scenario(fixture("reset-credit-count-only"), RuntimeConfig::default())
            .await
            .unwrap();
        let summary = read.rate_limits.unwrap().rate_limit_reset_credits.unwrap();

        assert_eq!(summary.available_count, 3);
        assert!(summary.credits.is_none());
    }

    #[tokio::test]
    async fn partial_reset_credit_fixture_does_not_replace_count_with_detail_length() {
        let read = read_scenario(fixture("reset-credit-partial"), RuntimeConfig::default())
            .await
            .unwrap();
        let summary = read.rate_limits.unwrap().rate_limit_reset_credits.unwrap();

        assert_eq!(summary.available_count, 2);
        assert_eq!(summary.credits.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn no_fixed_window_fixture_remains_a_valid_transport_read() {
        let read = read_scenario(fixture("no-fixed-window"), RuntimeConfig::default())
            .await
            .unwrap();
        let rate_limits = read.rate_limits.unwrap();

        assert!(rate_limits.rate_limits.primary.is_none());
        assert!(rate_limits.rate_limits.secondary.is_none());
        assert!(matches!(read.usage, OptionalUsageRead::Available(_)));
    }

    #[tokio::test]
    async fn reset_timestamps_crossing_dst_are_kept_as_utc_epoch_facts() {
        let read = read_scenario(fixture("dst-boundary"), RuntimeConfig::default())
            .await
            .unwrap();
        let rate_limits = read.rate_limits.unwrap().rate_limits;

        assert_eq!(rate_limits.primary.unwrap().resets_at, Some(1_793_511_000));
        assert_eq!(
            rate_limits.secondary.unwrap().resets_at,
            Some(1_794_115_800)
        );
    }

    #[test]
    fn default_limits_match_the_wp2_contract() {
        let config = RuntimeConfig::default();
        assert_eq!(config.startup_timeout, Duration::from_secs(5));
        assert_eq!(config.request_timeout, Duration::from_secs(10));
        assert_eq!(config.idle_child_timeout, Duration::from_secs(60));
        assert_eq!(config.max_jsonl_line_bytes, 1024 * 1024);
        assert_eq!(config.max_stdout_bytes, 2 * 1024 * 1024);
        assert_eq!(config.stderr_ring_bytes, 64 * 1024);
    }
}
