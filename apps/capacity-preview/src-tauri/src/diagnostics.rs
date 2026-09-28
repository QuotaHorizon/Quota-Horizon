use capacity_domain::{
    Availability, Clock, Compatibility, Freshness, ResetCreditDetailsStatus, SummaryStatus,
    SystemClock, UtcTimestamp,
};
use capacity_store::MonitorSettings;
use codex_runtime::discovery::{DiscoveryOutcome, DiscoveryReport, FileIdentityStrength};
use serde::Serialize;

use crate::{DesktopLifecycle, DesktopState, DesktopStatusEnvelope, get_status_inner};

const DIAGNOSTICS_SCHEMA_VERSION: &str = "1.0";
const MAX_DIAGNOSTIC_CODES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopDiagnosticsStatus {
    Available,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDiagnosticsEnvelope {
    pub(crate) schema_version: &'static str,
    pub(crate) status: DesktopDiagnosticsStatus,
    pub(crate) reason_code: &'static str,
    pub(crate) preview: Option<DesktopDiagnosticsPreview>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopDiagnosticsPreview {
    generated_at: String,
    product_version: &'static str,
    platform: &'static str,
    architecture: String,
    discovery: DiagnosticsDiscoveryView,
    monitor: DiagnosticsMonitorView,
    last_read: Option<DiagnosticsLastReadView>,
    path_redacted: bool,
    account_redacted: bool,
    quota_values_redacted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsDiscoveryView {
    outcome: DiscoveryOutcome,
    candidate_count: usize,
    selected_source_categories: Vec<&'static str>,
    codex_version: Option<String>,
    identity_strength: Option<FileIdentityStrength>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsMonitorView {
    store_backend_available: bool,
    settings_available: bool,
    auto_refresh_enabled: Option<bool>,
    refresh_interval_seconds: Option<u32>,
    notification_privacy: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsLastReadView {
    lifecycle: DesktopLifecycle,
    captured_at: String,
    availability: Availability,
    freshness: Freshness,
    compatibility: Compatibility,
    quota_window_count: usize,
    reset_credit_summary: SummaryStatus,
    reset_credit_details: ResetCreditDetailsStatus,
    usage_availability: Availability,
    diagnostic_codes: Vec<String>,
}

pub(crate) async fn preview(state: &DesktopState) -> DesktopDiagnosticsEnvelope {
    let discovery = match state.discovery_snapshot() {
        Ok(discovery) => discovery,
        Err(_) => return failed_envelope(),
    };
    let settings = state.monitor_settings().await;
    let status = get_status_inner(state).await;
    build_preview(
        &discovery,
        settings.as_ref(),
        &status,
        state.persistence_backend_available(),
        &SystemClock.now(),
    )
}

fn build_preview(
    discovery: &DiscoveryReport,
    settings: Option<&MonitorSettings>,
    status: &DesktopStatusEnvelope,
    store_backend_available: bool,
    generated_at: &UtcTimestamp,
) -> DesktopDiagnosticsEnvelope {
    let selected = discovery.selected_candidate();
    let last_read = status.status.as_ref().map(|read| DiagnosticsLastReadView {
        lifecycle: status.lifecycle,
        captured_at: read.captured_at.clone(),
        availability: read.data_status.availability,
        freshness: read.data_status.freshness,
        compatibility: read.data_status.compatibility,
        quota_window_count: read.quota_windows.len(),
        reset_credit_summary: read.reset_credits.summary_status,
        reset_credit_details: read.reset_credits.details_status,
        usage_availability: read.usage.availability,
        diagnostic_codes: read
            .diagnostic_codes
            .iter()
            .filter(|code| safe_reason_code(code))
            .take(MAX_DIAGNOSTIC_CODES)
            .cloned()
            .collect(),
    });
    let (diagnostics_status, reason_code) = if settings.is_none() {
        (
            DesktopDiagnosticsStatus::Partial,
            "diagnostics_settings_unavailable",
        )
    } else if last_read.is_none() {
        (
            DesktopDiagnosticsStatus::Partial,
            "diagnostics_snapshot_unavailable",
        )
    } else {
        (DesktopDiagnosticsStatus::Available, "diagnostics_available")
    };
    DesktopDiagnosticsEnvelope {
        schema_version: DIAGNOSTICS_SCHEMA_VERSION,
        status: diagnostics_status,
        reason_code,
        preview: Some(DesktopDiagnosticsPreview {
            generated_at: generated_at.as_str().to_owned(),
            product_version: env!("CARGO_PKG_VERSION"),
            platform: discovery.platform.as_str(),
            architecture: safe_metadata_or_unknown(&discovery.architecture, 64),
            discovery: DiagnosticsDiscoveryView {
                outcome: discovery.outcome,
                candidate_count: discovery.candidates.len(),
                selected_source_categories: selected.map_or_else(Vec::new, |candidate| {
                    candidate
                        .sources
                        .iter()
                        .map(|source| source.as_str())
                        .collect()
                }),
                codex_version: selected
                    .and_then(|candidate| candidate.version.as_deref())
                    .filter(|version| safe_ascii_metadata(version, 128))
                    .map(str::to_owned),
                identity_strength: selected.map(|candidate| candidate.identity_strength),
            },
            monitor: DiagnosticsMonitorView {
                store_backend_available,
                settings_available: settings.is_some(),
                auto_refresh_enabled: settings.map(|settings| settings.auto_refresh_enabled),
                refresh_interval_seconds: settings
                    .map(|settings| settings.refresh_interval_seconds),
                notification_privacy: settings.map(|settings| {
                    if settings.lock_screen_privacy {
                        "generic"
                    } else {
                        "detailed"
                    }
                }),
            },
            last_read,
            path_redacted: true,
            account_redacted: true,
            quota_values_redacted: true,
        }),
    }
}

fn failed_envelope() -> DesktopDiagnosticsEnvelope {
    DesktopDiagnosticsEnvelope {
        schema_version: DIAGNOSTICS_SCHEMA_VERSION,
        status: DesktopDiagnosticsStatus::Failed,
        reason_code: "diagnostics_state_unavailable",
        preview: None,
    }
}

fn safe_metadata_or_unknown(value: &str, maximum_length: usize) -> String {
    if safe_ascii_metadata(value, maximum_length) {
        value.to_owned()
    } else {
        "unknown".to_owned()
    }
}

fn safe_ascii_metadata(value: &str, maximum_length: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum_length
        && value.is_ascii()
        && !value.chars().any(char::is_control)
}

fn safe_reason_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DesktopDataStatusView, DesktopQuotaWindowView, DesktopResetCreditView, DesktopStatusView,
        DesktopUsageView,
    };
    use capacity_store::SettingsLanguage;
    use codex_runtime::discovery::{
        CandidateSource, CandidateVerification, CodexExecutableCandidate, HostPlatform,
    };

    fn discovery() -> DiscoveryReport {
        DiscoveryReport {
            schema_version: "1.0".to_owned(),
            platform: HostPlatform::Macos,
            architecture: "aarch64".to_owned(),
            outcome: DiscoveryOutcome::Selected,
            selected_executable_id: Some("fixture-executable".to_owned()),
            candidates: vec![CodexExecutableCandidate {
                executable_id: "fixture-executable".to_owned(),
                sources: vec![CandidateSource::MacosChatgptBundle],
                discovered_paths: vec!["/Users/secret/Bearer-token/codex".to_owned()],
                canonical_path: "/Users/secret/Bearer-token/codex".to_owned(),
                file_identity: "secret-file-identity".to_owned(),
                identity_strength: FileIdentityStrength::OsFileId,
                version: Some("codex-cli 0.150.0".to_owned()),
                verification: CandidateVerification::Verified,
                requires_confirmation: false,
                reason_codes: Vec::new(),
            }],
            diagnostics: Vec::new(),
        }
    }

    fn settings() -> MonitorSettings {
        MonitorSettings {
            revision: 7,
            auto_refresh_enabled: true,
            refresh_interval_seconds: 300,
            notification_threshold_basis_points: Some(2_000),
            reset_credit_notice_hours: 48,
            quiet_hours_enabled: false,
            quiet_hours_start_minute: None,
            quiet_hours_end_minute: None,
            language: SettingsLanguage::English,
            lock_screen_privacy: true,
            launch_at_login: false,
            history_retention_days: 180,
            updated_at: UtcTimestamp::parse("2026-08-30T00:00:00Z").unwrap(),
        }
    }

    fn status() -> DesktopStatusEnvelope {
        DesktopStatusEnvelope {
            history_context_id: None,
            selected_executable_source: None,
            schema_version: "1.0",
            sequence: 4,
            lifecycle: DesktopLifecycle::Ready,
            status: Some(DesktopStatusView {
                schema_version: "1.0".to_owned(),
                captured_at: "2026-08-30T00:00:00Z".to_owned(),
                quota_observed_at: None,
                quota_freshness: None,
                quota_source: None,
                codex_version: Some("codex-cli 0.150.0".to_owned()),
                account: Some(crate::DesktopAccountView {
                    auth_mode: Some("chatgpt".to_owned()),
                    plan_type: Some("fixture-secret-plan".to_owned()),
                    binding_status: capacity_domain::AccountBindingStatus::Ephemeral,
                }),
                data_status: DesktopDataStatusView {
                    availability: Availability::Complete,
                    freshness: Freshness::Live,
                    compatibility: Compatibility::ExpectedCompatible,
                    reason_codes: vec!["live_app_server".to_owned()],
                },
                quota_windows: vec![DesktopQuotaWindowView {
                    limit_id: "fixture-secret-window".to_owned(),
                    label: Some("fixture-secret-label".to_owned()),
                    window_minutes: Some(10_080),
                    used_percent: 33.0,
                    remaining_percent: 67.0,
                    resets_at: Some("2026-09-05T00:00:00Z".to_owned()),
                }],
                reset_credits: DesktopResetCreditView {
                    summary_status: SummaryStatus::Available,
                    available_count: Some(99),
                    details_status: ResetCreditDetailsStatus::Complete,
                },
                usage: DesktopUsageView {
                    availability: Availability::Complete,
                    has_summary: true,
                    reason_codes: Vec::new(),
                },
                diagnostic_codes: vec![
                    "compatibility_expected".to_owned(),
                    "secret diagnostic payload".to_owned(),
                ],
            }),
            candidates: Vec::new(),
            selected_executable_id: Some("fixture-executable".to_owned()),
            issue: None,
            persistence_enabled: false,
        }
    }

    fn fixture(name: &str) -> serde_json::Value {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../fixtures/desktop/v1")
                .join(name),
        )
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn available_preview_is_an_explicit_redacted_allowlist() {
        let report = build_preview(
            &discovery(),
            Some(&settings()),
            &status(),
            true,
            &UtcTimestamp::parse("2026-08-30T00:05:00Z").unwrap(),
        );
        let value = serde_json::to_value(&report).unwrap();
        let json = serde_json::to_string(&value).unwrap();
        for canary in [
            "/Users/secret",
            "Bearer-token",
            "secret-file-identity",
            "fixture-secret-plan",
            "fixture-secret-window",
            "fixture-secret-label",
            "secret diagnostic payload",
            "99",
            "67",
        ] {
            assert!(!json.contains(canary), "diagnostics leaked {canary}");
        }
        assert_eq!(value, fixture("diagnostics-available-envelope.json"));
    }

    #[test]
    fn missing_settings_and_snapshot_remain_a_useful_partial_preview() {
        let mut status = status();
        status.status = None;
        status.lifecycle = DesktopLifecycle::Idle;
        let report = build_preview(
            &discovery(),
            None,
            &status,
            false,
            &UtcTimestamp::parse("2026-08-30T00:05:00Z").unwrap(),
        );
        assert_eq!(report.status, DesktopDiagnosticsStatus::Partial);
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            fixture("diagnostics-partial-envelope.json")
        );
    }

    #[test]
    fn invalid_host_metadata_is_replaced_not_forwarded() {
        let mut discovery = discovery();
        discovery.architecture = "arm64\n/Users/secret".to_owned();
        let report = build_preview(
            &discovery,
            Some(&settings()),
            &status(),
            true,
            &UtcTimestamp::parse("2026-08-30T00:05:00Z").unwrap(),
        );
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("unknown"));
        assert!(!json.contains("/Users/secret"));
    }
}
