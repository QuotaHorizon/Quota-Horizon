use std::collections::HashSet;

use capacity_domain::{
    AccountBindingStatus, AccountSnapshot, Availability, CodexExecutableSnapshot, Compatibility,
    DataStatus, Diagnostic, DiagnosticSeverity, EnvironmentSnapshot, Freshness, QuotaSnapshot,
    QuotaWindow, ReasonCode, ResetCredit, ResetCreditDetailsStatus, ResetCreditSummary,
    STATUS_SCHEMA_VERSION, StatusSnapshot, SummaryStatus,
    TokenUsageSummary as DomainTokenUsageSummary, UsageCreditEntry, UsageCreditSummary,
    UsageSnapshot, UtcTimestamp,
};
use chrono::{DateTime, SecondsFormat, Utc};
use thiserror::Error;

use crate::app_server::{
    CapacityRead, OptionalUsageRead, RateLimitResetCreditSummary, RateLimitSnapshot,
    RateLimitWindow, UsageUnavailableReason,
};

#[derive(Debug, Clone)]
pub struct LiveStatusContext {
    pub captured_at: UtcTimestamp,
    pub environment: EnvironmentSnapshot,
    pub codex_executable: CodexExecutableSnapshot,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum NormalizationError {
    #[error("the ChatGPT account response did not include rate limits")]
    MissingRateLimits,
    #[error("a quota percentage was outside the supported range")]
    InvalidPercentage,
    #[error("a quota window included an invalid reset timestamp")]
    InvalidResetTimestamp,
    #[error("a rate-limit bucket identifier was invalid")]
    InvalidLimitId,
    #[error("account metadata was invalid")]
    InvalidAccountMetadata,
    #[error("reset-credit details were structurally invalid")]
    InvalidResetCredit,
    #[error("usage-credit details were structurally invalid")]
    InvalidUsageCredit,
    #[error("the normalized status violated status JSON v1")]
    InvalidStatusContract,
}

struct WindowNormalizationState {
    captured_at_epoch: i64,
    partial: bool,
    reset_time_elapsed: bool,
}

pub fn normalize_capacity_read(
    read: CapacityRead,
    context: LiveStatusContext,
) -> Result<StatusSnapshot, NormalizationError> {
    let mut diagnostics = Vec::new();
    let initialize_shape_complete = read.initialize.user_agent.is_some()
        && read.initialize.platform_family.is_some()
        && read.initialize.platform_os.is_some();
    let compatibility = if initialize_shape_complete {
        compatibility_for_version(context.codex_executable.version.as_deref())
    } else {
        Compatibility::NotTested
    };
    let compatibility_reason = if compatibility == Compatibility::ExpectedCompatible {
        "compatibility_expected"
    } else {
        "compatibility_not_tested"
    };
    let mut reason_codes = vec![reason("live_app_server"), reason(compatibility_reason)];

    if !initialize_shape_complete {
        push_reason(&mut reason_codes, "initialize_shape_partial");
        diagnostics.push(diagnostic(
            "initialize_shape_partial",
            DiagnosticSeverity::Warning,
            "The app-server initialize response omitted compatibility metadata.",
        ));
    }

    let Some(account) = read.account.account else {
        return Ok(non_capacity_status(
            context,
            None,
            "authentication_required",
            "Codex does not currently expose an authenticated account.",
        ));
    };

    let account_type = safe_account_metadata(&account.account_type)?.to_owned();
    let account_plan_type = account
        .plan_type
        .as_deref()
        .map(safe_account_metadata)
        .transpose()?
        .map(str::to_owned);
    let account_snapshot = AccountSnapshot {
        auth_mode: Some(account_type.clone()),
        plan_type: account_plan_type,
        binding_status: AccountBindingStatus::Ephemeral,
    };

    if account_type != "chatgpt" {
        return Ok(non_capacity_status(
            context,
            Some(account_snapshot),
            "authentication_mode_unsupported",
            "The current authentication mode does not expose supported ChatGPT quota windows.",
        ));
    }

    let rate_limits = read
        .rate_limits
        .ok_or(NormalizationError::MissingRateLimits)?;
    let (buckets, used_single_bucket_fallback) = collect_buckets(&rate_limits)?;
    let plan_type = account_snapshot.plan_type.clone().or_else(|| {
        buckets
            .iter()
            .find_map(|(_, bucket)| bucket.plan_type.clone())
    });
    let plan_type = plan_type
        .as_deref()
        .map(safe_account_metadata)
        .transpose()?
        .map(str::to_owned);
    let account_snapshot = AccountSnapshot {
        plan_type,
        ..account_snapshot
    };
    let mut windows = Vec::new();
    let mut window_state = WindowNormalizationState {
        captured_at_epoch: timestamp_epoch(&context.captured_at)?,
        partial: used_single_bucket_fallback,
        reset_time_elapsed: false,
    };

    if used_single_bucket_fallback {
        push_reason(&mut reason_codes, "single_bucket_fallback");
        diagnostics.push(diagnostic(
            "single_bucket_fallback",
            DiagnosticSeverity::Warning,
            "The app-server returned only the backward-compatible single-bucket quota view.",
        ));
    }

    for (bucket_id, bucket) in &buckets {
        if bucket
            .limit_id
            .as_deref()
            .is_some_and(|limit_id| limit_id != bucket_id)
        {
            window_state.partial = true;
            push_reason(&mut reason_codes, "limit_id_mismatch");
        }
        append_window(
            &mut windows,
            bucket_id,
            bucket.limit_name.as_deref(),
            "primary",
            bucket.primary.as_ref(),
            &mut window_state,
            &mut reason_codes,
        )?;
        append_window(
            &mut windows,
            bucket_id,
            bucket.limit_name.as_deref(),
            "secondary",
            bucket.secondary.as_ref(),
            &mut window_state,
            &mut reason_codes,
        )?;
    }

    let WindowNormalizationState {
        mut partial,
        reset_time_elapsed,
        ..
    } = window_state;

    let reset_credit_summary = normalize_reset_credits(
        rate_limits.rate_limit_reset_credits,
        &mut partial,
        &mut reason_codes,
        &mut diagnostics,
    )?;
    let usage_credit_summary = normalize_usage_credits(
        &buckets,
        context.captured_at.clone(),
        &mut partial,
        &mut reason_codes,
    )?;
    let usage = normalize_usage(read.usage);

    if read.notifications.unknown_notifications > 0 {
        diagnostics.push(diagnostic(
            "unknown_notifications_ignored",
            DiagnosticSeverity::Info,
            "Unknown app-server notifications were ignored without recording their payloads.",
        ));
        push_reason(&mut reason_codes, "unknown_notifications_ignored");
    }
    if read.stderr_truncated {
        diagnostics.push(diagnostic(
            "stderr_capture_truncated",
            DiagnosticSeverity::Info,
            "The bounded in-memory app-server stderr diagnostic buffer was truncated.",
        ));
    }

    if windows.is_empty() {
        reason_codes.retain(|code| code.as_str() != compatibility_reason);
        push_reason(&mut reason_codes, "no_fixed_window");
        diagnostics.push(diagnostic(
            "no_fixed_window",
            DiagnosticSeverity::Warning,
            "The account response did not include a fixed quota window.",
        ));
    }
    if reset_time_elapsed {
        push_reason(&mut reason_codes, "quota_reset_time_elapsed");
        diagnostics.push(diagnostic(
            "quota_reset_time_elapsed",
            DiagnosticSeverity::Warning,
            "A returned quota window has passed its reset time; the original value was preserved as stale.",
        ));
    }

    let has_fixed_window = !windows.is_empty();
    let status = StatusSnapshot {
        schema_version: STATUS_SCHEMA_VERSION.to_owned(),
        captured_at: context.captured_at,
        environment: context.environment,
        codex_executable: Some(context.codex_executable),
        account: Some(account_snapshot),
        quota: QuotaSnapshot {
            windows,
            reset_credit_summary,
            usage_credit_summary,
        },
        usage,
        data_status: DataStatus {
            availability: if !has_fixed_window {
                Availability::Unsupported
            } else if partial {
                Availability::Partial
            } else {
                Availability::Complete
            },
            freshness: if !has_fixed_window {
                Freshness::NotApplicable
            } else if reset_time_elapsed {
                Freshness::Stale
            } else {
                Freshness::Live
            },
            compatibility: if has_fixed_window {
                compatibility
            } else {
                Compatibility::NotApplicable
            },
            reason_codes,
        },
        diagnostics,
    };
    status
        .validate()
        .map_err(|_| NormalizationError::InvalidStatusContract)?;
    Ok(status)
}

fn compatibility_for_version(version: Option<&str>) -> Compatibility {
    match version {
        Some("codex-cli 0.150.0-alpha.12.2" | "codex-cli 0.133.0") => {
            Compatibility::ExpectedCompatible
        }
        _ => Compatibility::NotTested,
    }
}

fn collect_buckets(
    rate_limits: &crate::app_server::RateLimitsReadResult,
) -> Result<(Vec<(String, RateLimitSnapshot)>, bool), NormalizationError> {
    if let Some(by_id) = rate_limits
        .rate_limits_by_limit_id
        .as_ref()
        .filter(|by_id| !by_id.is_empty())
    {
        let mut buckets = Vec::with_capacity(by_id.len());
        for (key, bucket) in by_id {
            buckets.push((safe_limit_id(key)?.to_owned(), bucket.clone()));
        }
        return Ok((buckets, false));
    }

    let fallback_id = rate_limits
        .rate_limits
        .limit_id
        .as_deref()
        .unwrap_or("codex");
    Ok((
        vec![(
            safe_limit_id(fallback_id)?.to_owned(),
            rate_limits.rate_limits.clone(),
        )],
        true,
    ))
}

fn append_window(
    output: &mut Vec<QuotaWindow>,
    bucket_id: &str,
    bucket_label: Option<&str>,
    kind: &'static str,
    window: Option<&RateLimitWindow>,
    state: &mut WindowNormalizationState,
    reason_codes: &mut Vec<ReasonCode>,
) -> Result<(), NormalizationError> {
    let Some(window) = window else {
        return Ok(());
    };
    if !window.used_percent.is_finite() || !(0.0..=100.0).contains(&window.used_percent) {
        return Err(NormalizationError::InvalidPercentage);
    }
    if window.window_duration_mins.is_none() || window.resets_at.is_none() {
        state.partial = true;
        push_reason(reason_codes, "quota_window_incomplete");
    }
    if window
        .resets_at
        .is_some_and(|resets_at| resets_at <= state.captured_at_epoch)
    {
        state.reset_time_elapsed = true;
    }

    output.push(QuotaWindow {
        limit_id: format!("{bucket_id}:{kind}"),
        label: window_label(bucket_label, kind, window.window_duration_mins),
        window_minutes: window.window_duration_mins,
        used_percent: window.used_percent,
        remaining_percent: 100.0 - window.used_percent,
        resets_at: window.resets_at.map(timestamp_from_epoch).transpose()?,
    });
    Ok(())
}

fn window_label(
    bucket_label: Option<&str>,
    kind: &'static str,
    duration_minutes: Option<u64>,
) -> Option<String> {
    match duration_minutes {
        Some(300) => Some("5-hour".to_owned()),
        Some(10_080) => Some("weekly".to_owned()),
        _ => bucket_label
            .filter(|label| {
                !label.is_empty()
                    && label.len() <= 128
                    && !label.chars().any(|character| character.is_control())
            })
            .map(|label| format!("{label} {kind}"))
            .or_else(|| Some(kind.to_owned())),
    }
}

fn normalize_reset_credits(
    summary: Option<RateLimitResetCreditSummary>,
    partial: &mut bool,
    reason_codes: &mut Vec<ReasonCode>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<ResetCreditSummary, NormalizationError> {
    let Some(summary) = summary else {
        *partial = true;
        push_reason(reason_codes, "reset_credit_summary_unavailable");
        return Ok(unavailable_reset_credit_summary());
    };

    let Some(details) = summary.credits else {
        *partial = true;
        push_reason(reason_codes, "reset_credit_details_unavailable");
        return Ok(ResetCreditSummary {
            summary_status: SummaryStatus::Available,
            available_count: Some(summary.available_count),
            details_status: ResetCreditDetailsStatus::Unavailable,
            credits: None,
        });
    };

    if details.len() as u64 > summary.available_count {
        *partial = true;
        push_reason(reason_codes, "reset_credit_detail_conflict");
        diagnostics.push(diagnostic(
            "reset_credit_detail_conflict",
            DiagnosticSeverity::Warning,
            "Reset-credit details exceeded the authoritative available count and were withheld.",
        ));
        return Ok(ResetCreditSummary {
            summary_status: SummaryStatus::Available,
            available_count: Some(summary.available_count),
            details_status: ResetCreditDetailsStatus::Unavailable,
            credits: None,
        });
    }

    let mut detail_fields_partial = false;
    let mut seen_ids = HashSet::new();
    let credits = details
        .into_iter()
        .map(|credit| {
            let opaque_id = credit.id.filter(|value| is_safe_text(value, 512));
            let reset_type = credit.reset_type.filter(|value| is_safe_text(value, 64));
            let status = credit.status.filter(|value| is_safe_text(value, 64));
            let title = credit
                .title
                .map(|value| {
                    if is_safe_text(&value, 512) {
                        Ok(value)
                    } else {
                        Err(NormalizationError::InvalidResetCredit)
                    }
                })
                .transpose()?;
            let description = credit
                .description
                .map(|value| {
                    if is_safe_text(&value, 512) {
                        Ok(value)
                    } else {
                        Err(NormalizationError::InvalidResetCredit)
                    }
                })
                .transpose()?;
            let (Some(opaque_id), Some(reset_type), Some(status)) = (opaque_id, reset_type, status)
            else {
                return Err(NormalizationError::InvalidResetCredit);
            };
            if !seen_ids.insert(opaque_id.clone()) {
                return Err(NormalizationError::InvalidResetCredit);
            }
            if credit.granted_at.is_none()
                || reset_type != "codexRateLimits"
                || status != "available"
            {
                detail_fields_partial = true;
            }
            Ok(ResetCredit {
                opaque_id,
                reset_type,
                status,
                granted_at: credit.granted_at.map(timestamp_from_epoch).transpose()?,
                expires_at: credit.expires_at.map(timestamp_from_epoch).transpose()?,
                title,
                description,
            })
        })
        .collect::<Result<Vec<_>, NormalizationError>>();
    let credits = match credits {
        Ok(credits) => credits,
        Err(_) => {
            *partial = true;
            push_reason(reason_codes, "reset_credit_details_invalid");
            diagnostics.push(diagnostic(
                "reset_credit_details_invalid",
                DiagnosticSeverity::Warning,
                "Reset-credit details were structurally invalid and were withheld.",
            ));
            return Ok(ResetCreditSummary {
                summary_status: SummaryStatus::Available,
                available_count: Some(summary.available_count),
                details_status: ResetCreditDetailsStatus::Unavailable,
                credits: None,
            });
        }
    };
    let details_status =
        if credits.len() as u64 == summary.available_count && !detail_fields_partial {
            ResetCreditDetailsStatus::Complete
        } else {
            *partial = true;
            push_reason(reason_codes, "reset_credit_details_partial");
            ResetCreditDetailsStatus::Partial
        };

    Ok(ResetCreditSummary {
        summary_status: SummaryStatus::Available,
        available_count: Some(summary.available_count),
        details_status,
        credits: Some(credits),
    })
}

fn normalize_usage_credits(
    buckets: &[(String, RateLimitSnapshot)],
    captured_at: UtcTimestamp,
    partial: &mut bool,
    reason_codes: &mut Vec<ReasonCode>,
) -> Result<UsageCreditSummary, NormalizationError> {
    let mut entries = Vec::new();
    let mut summary_partial = false;
    for (limit_id, bucket) in buckets {
        let Some(credits) = &bucket.credits else {
            continue;
        };
        let balance = credits
            .balance
            .as_deref()
            .map(|value| {
                if is_safe_text(value, 128) {
                    Ok(value.to_owned())
                } else {
                    Err(NormalizationError::InvalidUsageCredit)
                }
            })
            .transpose()?;
        if credits.has_credits && !credits.unlimited && balance.is_none() {
            summary_partial = true;
        }
        entries.push(UsageCreditEntry {
            limit_id: limit_id.clone(),
            has_credits: credits.has_credits,
            unlimited: credits.unlimited,
            balance,
        });
    }

    let summary_status = if entries.is_empty() {
        SummaryStatus::Unavailable
    } else if summary_partial {
        *partial = true;
        push_reason(reason_codes, "usage_credit_details_partial");
        SummaryStatus::Partial
    } else {
        SummaryStatus::Available
    };
    Ok(UsageCreditSummary {
        summary_status,
        entries,
        captured_at,
        upstream_schema_fingerprint: None,
    })
}

fn normalize_usage(usage: OptionalUsageRead) -> UsageSnapshot {
    match usage {
        OptionalUsageRead::Available(value) => match value.summary {
            Some(summary) => {
                let summary = DomainTokenUsageSummary {
                    lifetime_tokens: summary.lifetime_tokens,
                    peak_daily_tokens: summary.peak_daily_tokens,
                    longest_running_turn_sec: summary.longest_running_turn_sec,
                    current_streak_days: summary.current_streak_days,
                    longest_streak_days: summary.longest_streak_days,
                };
                let has_observed_value = summary.lifetime_tokens.is_some()
                    || summary.peak_daily_tokens.is_some()
                    || summary.longest_running_turn_sec.is_some()
                    || summary.current_streak_days.is_some()
                    || summary.longest_streak_days.is_some();
                UsageSnapshot {
                    availability: if has_observed_value {
                        Availability::Complete
                    } else {
                        Availability::Partial
                    },
                    summary: Some(summary),
                    reason_codes: vec![reason(if has_observed_value {
                        "usage_summary_available"
                    } else {
                        "usage_summary_empty"
                    })],
                }
            }
            None => UsageSnapshot {
                availability: Availability::Partial,
                summary: None,
                reason_codes: vec![reason("usage_summary_unavailable")],
            },
        },
        OptionalUsageRead::Unavailable(UsageUnavailableReason::MethodUnavailable) => {
            unavailable_usage(Availability::Unsupported, "usage_capability_unavailable")
        }
        OptionalUsageRead::Unavailable(UsageUnavailableReason::ReadFailed) => {
            unavailable_usage(Availability::Failed, "usage_read_failed")
        }
        OptionalUsageRead::Unavailable(UsageUnavailableReason::InvalidResponse) => {
            unavailable_usage(Availability::Failed, "usage_response_invalid")
        }
        OptionalUsageRead::Unavailable(UsageUnavailableReason::NotApplicable) => {
            unavailable_usage(Availability::Unsupported, "usage_not_applicable")
        }
    }
}

fn unavailable_usage(availability: Availability, code: &'static str) -> UsageSnapshot {
    UsageSnapshot {
        availability,
        summary: None,
        reason_codes: vec![reason(code)],
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

fn non_capacity_status(
    context: LiveStatusContext,
    account: Option<AccountSnapshot>,
    code: &'static str,
    message: &'static str,
) -> StatusSnapshot {
    let captured_at = context.captured_at;
    StatusSnapshot {
        schema_version: STATUS_SCHEMA_VERSION.to_owned(),
        captured_at: captured_at.clone(),
        environment: context.environment,
        codex_executable: Some(context.codex_executable),
        account,
        quota: QuotaSnapshot {
            windows: Vec::new(),
            reset_credit_summary: unavailable_reset_credit_summary(),
            usage_credit_summary: unavailable_usage_credit_summary(captured_at),
        },
        usage: UsageSnapshot {
            availability: Availability::Unsupported,
            summary: None,
            reason_codes: vec![reason("usage_not_applicable")],
        },
        data_status: DataStatus {
            availability: Availability::Unsupported,
            freshness: Freshness::NotApplicable,
            compatibility: Compatibility::NotApplicable,
            reason_codes: vec![reason(code)],
        },
        diagnostics: vec![diagnostic(code, DiagnosticSeverity::Error, message)],
    }
}

fn timestamp_from_epoch(seconds: i64) -> Result<UtcTimestamp, NormalizationError> {
    let timestamp: DateTime<Utc> =
        DateTime::from_timestamp(seconds, 0).ok_or(NormalizationError::InvalidResetTimestamp)?;
    UtcTimestamp::parse(timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
        .map_err(|_| NormalizationError::InvalidResetTimestamp)
}

fn timestamp_epoch(timestamp: &UtcTimestamp) -> Result<i64, NormalizationError> {
    DateTime::parse_from_rfc3339(timestamp.as_str())
        .map(|value| value.timestamp())
        .map_err(|_| NormalizationError::InvalidResetTimestamp)
}

fn is_safe_text(value: &str, maximum_length: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum_length
        && !value.chars().any(char::is_control)
}

fn safe_limit_id(value: &str) -> Result<&str, NormalizationError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| character.is_control())
    {
        Err(NormalizationError::InvalidLimitId)
    } else {
        Ok(value)
    }
}

fn safe_account_metadata(value: &str) -> Result<&str, NormalizationError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| character.is_control())
    {
        Err(NormalizationError::InvalidAccountMetadata)
    } else {
        Ok(value)
    }
}

fn push_reason(reason_codes: &mut Vec<ReasonCode>, code: &'static str) {
    if !reason_codes.iter().any(|reason| reason.as_str() == code) {
        reason_codes.push(reason(code));
    }
}

fn diagnostic(
    code: &'static str,
    severity: DiagnosticSeverity,
    message: &'static str,
) -> Diagnostic {
    Diagnostic {
        code: reason(code),
        severity,
        message: message.to_owned(),
    }
}

fn reason(code: &'static str) -> ReasonCode {
    ReasonCode::new(code).expect("static reason code must be valid")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::app_server::tests::read_scenario;
    use crate::app_server::{
        AccountInfo, AccountReadResult, CreditsSnapshot, InitializeResult, NotificationSummary,
        RateLimitResetCredit, RateLimitWindow, RateLimitsReadResult,
        TokenUsageSummary as RuntimeTokenUsageSummary, UsageReadResult,
    };
    use fake_app_server::Scenario;

    const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/app-server");

    fn context() -> LiveStatusContext {
        LiveStatusContext {
            captured_at: UtcTimestamp::parse("2026-08-30T00:00:00Z").unwrap(),
            environment: EnvironmentSnapshot {
                environment_id: "fixture:normalizer".to_owned(),
                platform: "fixture".to_owned(),
                architecture: "fixture".to_owned(),
                boundary: "fixture".to_owned(),
            },
            codex_executable: CodexExecutableSnapshot {
                executable_id: "fixture-codex".to_owned(),
                source: "fixture".to_owned(),
                canonical_path: None,
                file_identity: None,
                version: Some("codex-cli fixture".to_owned()),
            },
        }
    }

    fn complete_read() -> CapacityRead {
        let bucket = RateLimitSnapshot {
            limit_id: Some("codex".to_owned()),
            limit_name: Some("Codex".to_owned()),
            plan_type: Some("plus".to_owned()),
            primary: Some(RateLimitWindow {
                used_percent: 35.0,
                window_duration_mins: Some(300),
                resets_at: Some(1_788_066_000),
            }),
            secondary: Some(RateLimitWindow {
                used_percent: 58.0,
                window_duration_mins: Some(10_080),
                resets_at: Some(1_788_566_400),
            }),
            credits: None,
        };
        CapacityRead {
            initialize: InitializeResult {
                user_agent: Some("codex-cli fixture".to_owned()),
                platform_family: Some("unix".to_owned()),
                platform_os: Some("macos".to_owned()),
            },
            account: AccountReadResult {
                account: Some(AccountInfo {
                    account_type: "chatgpt".to_owned(),
                    plan_type: Some("plus".to_owned()),
                }),
                requires_openai_auth: Some(true),
            },
            rate_limits: Some(RateLimitsReadResult {
                rate_limits: bucket.clone(),
                rate_limits_by_limit_id: Some(BTreeMap::from([("codex".to_owned(), bucket)])),
                rate_limit_reset_credits: Some(RateLimitResetCreditSummary {
                    available_count: 1,
                    credits: Some(vec![RateLimitResetCredit {
                        id: Some("credit-1".to_owned()),
                        granted_at: Some(1_787_961_600),
                        expires_at: Some(1_790_726_400),
                        reset_type: Some("codexRateLimits".to_owned()),
                        status: Some("available".to_owned()),
                        title: Some("Reset Codex limits".to_owned()),
                        description: None,
                    }]),
                }),
            }),
            usage: OptionalUsageRead::Unavailable(UsageUnavailableReason::MethodUnavailable),
            notifications: NotificationSummary::default(),
            stderr_captured_bytes: 0,
            stderr_truncated: false,
        }
    }

    #[test]
    fn normalizes_all_windows_and_keeps_status_axes_independent() {
        let status = normalize_capacity_read(complete_read(), context()).unwrap();

        assert_eq!(status.quota.windows.len(), 2);
        assert_eq!(status.quota.windows[0].remaining_percent, 65.0);
        assert_eq!(status.data_status.availability, Availability::Complete);
        assert_eq!(status.data_status.freshness, Freshness::Live);
        assert_eq!(status.data_status.compatibility, Compatibility::NotTested);
        assert_eq!(status.quota.reset_credit_summary.available_count, Some(1));
    }

    #[test]
    fn null_reset_credit_summary_is_not_converted_to_zero() {
        let mut read = complete_read();
        read.rate_limits.as_mut().unwrap().rate_limit_reset_credits = None;

        let status = normalize_capacity_read(read, context()).unwrap();

        assert_eq!(
            status.quota.reset_credit_summary.summary_status,
            SummaryStatus::Unavailable
        );
        assert_eq!(status.quota.reset_credit_summary.available_count, None);
        assert_eq!(status.data_status.availability, Availability::Partial);
        assert!(
            status
                .data_status
                .reason_codes
                .iter()
                .any(|code| code.as_str() == "reset_credit_summary_unavailable")
        );
    }

    #[test]
    fn missing_account_is_authentication_required_not_zero_quota() {
        let mut read = complete_read();
        read.account.account = None;
        read.rate_limits = None;

        let status = normalize_capacity_read(read, context()).unwrap();

        assert_eq!(status.data_status.availability, Availability::Unsupported);
        assert_eq!(status.data_status.freshness, Freshness::NotApplicable);
        assert!(status.quota.windows.is_empty());
        assert_eq!(
            status.data_status.reason_codes[0].as_str(),
            "authentication_required"
        );
    }

    #[test]
    fn partial_reset_credit_details_preserve_authoritative_count() {
        let mut read = complete_read();
        read.rate_limits
            .as_mut()
            .unwrap()
            .rate_limit_reset_credits
            .as_mut()
            .unwrap()
            .available_count = 2;

        let status = normalize_capacity_read(read, context()).unwrap();
        let summary = status.quota.reset_credit_summary;

        assert_eq!(summary.available_count, Some(2));
        assert_eq!(summary.details_status, ResetCreditDetailsStatus::Partial);
        assert_eq!(status.data_status.availability, Availability::Partial);
    }

    #[test]
    fn no_fixed_window_is_not_reported_as_a_protocol_failure() {
        let mut read = complete_read();
        let rate_limits = read.rate_limits.as_mut().unwrap();
        rate_limits.rate_limits.primary = None;
        rate_limits.rate_limits.secondary = None;
        let bucket = rate_limits
            .rate_limits_by_limit_id
            .as_mut()
            .unwrap()
            .get_mut("codex")
            .unwrap();
        bucket.primary = None;
        bucket.secondary = None;

        let status = normalize_capacity_read(read, context()).unwrap();

        assert!(status.quota.windows.is_empty());
        assert_eq!(status.data_status.availability, Availability::Unsupported);
        assert_eq!(status.data_status.freshness, Freshness::NotApplicable);
        assert_eq!(
            status.data_status.compatibility,
            Compatibility::NotApplicable
        );
        assert!(
            status
                .data_status
                .reason_codes
                .iter()
                .any(|code| code.as_str() == "no_fixed_window")
        );
    }

    #[test]
    fn elapsed_reset_time_preserves_percentage_and_marks_snapshot_stale() {
        let mut context = context();
        context.captured_at = UtcTimestamp::parse("2026-09-06T00:00:00Z").unwrap();

        let status = normalize_capacity_read(complete_read(), context).unwrap();

        assert_eq!(status.quota.windows[0].remaining_percent, 65.0);
        assert_eq!(status.data_status.freshness, Freshness::Stale);
        assert!(
            status
                .data_status
                .reason_codes
                .iter()
                .any(|code| code.as_str() == "quota_reset_time_elapsed")
        );
    }

    #[test]
    fn token_usage_summary_is_preserved_without_quota_conversion() {
        let mut read = complete_read();
        read.usage = OptionalUsageRead::Available(UsageReadResult {
            summary: Some(RuntimeTokenUsageSummary {
                lifetime_tokens: Some(123_456),
                peak_daily_tokens: Some(4_567),
                longest_running_turn_sec: None,
                current_streak_days: Some(5),
                longest_streak_days: Some(12),
            }),
        });

        let status = normalize_capacity_read(read, context()).unwrap();

        assert_eq!(status.usage.availability, Availability::Complete);
        assert_eq!(status.usage.summary.unwrap().lifetime_tokens, Some(123_456));
        assert_eq!(status.quota.windows[0].remaining_percent, 65.0);
    }

    #[test]
    fn workspace_credit_balance_stays_an_unconverted_string_fact() {
        let mut read = complete_read();
        let rate_limits = read.rate_limits.as_mut().unwrap();
        rate_limits
            .rate_limits_by_limit_id
            .as_mut()
            .unwrap()
            .get_mut("codex")
            .unwrap()
            .credits = Some(CreditsSnapshot {
            has_credits: true,
            unlimited: false,
            balance: Some("12.50".to_owned()),
        });

        let status = normalize_capacity_read(read, context()).unwrap();
        let summary = status.quota.usage_credit_summary;

        assert_eq!(summary.summary_status, SummaryStatus::Available);
        assert_eq!(summary.entries[0].balance.as_deref(), Some("12.50"));
    }

    #[test]
    fn observed_codex_versions_are_expected_compatible_not_tested() {
        let mut context = context();
        context.codex_executable.version = Some("codex-cli 0.150.0-alpha.12.2".to_owned());

        let status = normalize_capacity_read(complete_read(), context).unwrap();

        assert_eq!(
            status.data_status.compatibility,
            Compatibility::ExpectedCompatible
        );
        assert!(
            status
                .data_status
                .reason_codes
                .iter()
                .any(|code| code.as_str() == "compatibility_expected")
        );
    }

    #[tokio::test]
    async fn normalized_fixture_statuses_match_exact_expected_json() {
        for (name, captured_at) in [
            ("pro-multi-bucket", "2026-08-30T00:00:00Z"),
            ("weekly-only", "2026-08-30T00:00:00Z"),
            ("reset-credit-count-only", "2026-08-30T00:00:00Z"),
            ("reset-credit-partial", "2026-08-30T00:00:00Z"),
            ("no-fixed-window", "2026-08-30T00:00:00Z"),
            ("bucket-disappeared", "2026-08-30T00:00:00Z"),
            ("dst-boundary", "2026-11-01T04:00:00Z"),
        ] {
            let scenario =
                Scenario::from_directory(Path::new(FIXTURE_ROOT).join(name).as_path()).unwrap();
            let read = read_scenario(scenario, crate::app_server::RuntimeConfig::default())
                .await
                .unwrap();
            let mut context = context();
            context.captured_at = UtcTimestamp::parse(captured_at).unwrap();
            context.environment.environment_id = format!("fixture:{name}");
            context.codex_executable.version = Some("codex-cli 0.150.0-alpha.12.2".to_owned());
            let status = normalize_capacity_read(read, context).unwrap();
            let actual = serde_json::to_string_pretty(&status).unwrap();
            let expected = fs::read_to_string(
                Path::new(FIXTURE_ROOT)
                    .join(name)
                    .join("expected-status.json"),
            )
            .unwrap();

            assert_eq!(
                actual,
                expected.trim_end(),
                "normalized status changed for fixture {name}"
            );
        }
    }
}
