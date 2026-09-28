use serde::{Deserialize, Serialize};
use tauri::{App, AppHandle, Runtime};

use crate::{commands, models::AccountSummary};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityEnvelope {
    lifecycle: String,
    status: Option<CapacityStatus>,
    issue: Option<CapacityIssue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityStatus {
    captured_at: String,
    quota_observed_at: Option<String>,
    quota_freshness: Option<String>,
    account: Option<CapacityAccount>,
    data_status: CapacityDataStatus,
    quota_windows: Vec<CapacityWindow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityAccount {
    plan_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityDataStatus {
    availability: String,
    freshness: String,
    compatibility: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityWindow {
    #[serde(default)]
    limit_id: String,
    window_minutes: Option<u64>,
    remaining_percent: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityIssue {
    code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapacityPresentation {
    title: String,
    tooltip: String,
}

pub(super) fn setup(app: &mut App) {
    update_from_active_account(app.handle());
    // The host coordinator updates this tray directly under its quota gate.
    // Replaying asynchronous renderer events here could roll back a newer read.
}

pub(crate) fn update_from_serializable<R: Runtime, T: Serialize>(app: &AppHandle<R>, envelope: &T) {
    let Ok(payload) = serde_json::to_string(envelope) else {
        return;
    };
    update_from_payload(app, &payload);
}

fn update_from_payload<R: Runtime>(app: &AppHandle<R>, payload: &str) {
    let Ok(envelope) = serde_json::from_str::<CapacityEnvelope>(payload) else {
        return;
    };
    let presentation = capacity_presentation(&envelope);
    apply_presentation(app, &presentation);
}

pub(super) fn update_from_active_account<R: Runtime>(app: &AppHandle<R>) {
    let Some(presentation) = active_account(app)
        .as_ref()
        .and_then(stored_account_presentation)
    else {
        return;
    };
    apply_presentation(app, &presentation);
}

fn active_account<R: Runtime>(app: &AppHandle<R>) -> Option<AccountSummary> {
    commands::list_accounts_blocking(app.clone())
        .ok()?
        .into_iter()
        .find(|account| account.active)
}

fn apply_presentation<R: Runtime>(app: &AppHandle<R>, presentation: &CapacityPresentation) {
    let presentation = presentation.clone();
    if let Err(error) =
        capacity_desktop_service::with_tray_on_main_thread(app, super::TRAY_ID, move |tray| {
            let _ = tray.set_title(Some(&presentation.title));
            let _ = tray.set_tooltip(Some(&presentation.tooltip));
        })
    {
        eprintln!("failed to dispatch tray presentation: {error}");
    }
}

fn stored_account_presentation(account: &AccountSummary) -> Option<CapacityPresentation> {
    if account.usage.primary.is_none() && account.usage.secondary.is_none() {
        return None;
    }
    let (short, weekly) = super::account_usage_windows(&account.usage, &account.plan);
    let compact_remaining =
        |window: &crate::models::UsageWindow| super::quota_percent_label(window.remaining_percent);
    let mut title_parts = Vec::with_capacity(2);
    if let Some(window) = weekly {
        title_parts.push(format!("W {}", compact_remaining(window)));
    }
    if let Some(window) = short {
        title_parts.push(format!("5h {}", compact_remaining(window)));
    }
    let mut title = title_parts.join(" · ");
    let refresh_failed = account
        .usage
        .error
        .as_deref()
        .is_some_and(|error| !error.trim().is_empty());
    if refresh_failed {
        title.push_str(" !");
    } else if account.usage.fetched_at.is_none() {
        title.push_str(" ?");
    }
    let freshness = if refresh_failed {
        "refresh failed; cached quota"
    } else if account.usage.fetched_at.is_some() {
        "saved account quota"
    } else {
        "not refreshed"
    };
    let updated = account.usage.fetched_at.as_deref().unwrap_or("unknown");
    Some(CapacityPresentation {
        title,
        tooltip: format!(
            "QuotaHorizon · {} · {freshness} · updated {updated}",
            account.plan
        ),
    })
}

fn capacity_presentation(envelope: &CapacityEnvelope) -> CapacityPresentation {
    let Some(status) = envelope.status.as_ref() else {
        let (title, detail) = match envelope.lifecycle.as_str() {
            "selection_required" => ("Quota ?", "Choose the trusted Codex installation"),
            "error" => ("Quota !", "Capacity unavailable"),
            "idle" => ("Quota —", "Waiting for the first capacity refresh"),
            _ => ("Quota —", "No capacity snapshot"),
        };
        let detail = envelope
            .issue
            .as_ref()
            .map(|issue| issue.code.as_str())
            .unwrap_or(detail);
        return CapacityPresentation {
            title: title.to_owned(),
            tooltip: format!("QuotaHorizon · {detail}"),
        };
    };

    let no_short_window = status
        .account
        .as_ref()
        .and_then(|account| account.plan_type.as_deref())
        .is_some_and(super::plan_has_no_short_quota_window);
    let visible_window = |window: &&CapacityWindow| {
        (window.limit_id.is_empty()
            || window.limit_id == "codex"
            || window.limit_id.starts_with("codex:"))
            && !(no_short_window
                && window
                    .window_minutes
                    .is_some_and(|minutes| minutes <= 1_440))
    };
    let primary = status
        .quota_windows
        .iter()
        .filter(visible_window)
        .max_by_key(|window| window.window_minutes.unwrap_or(0));
    let secondary = status
        .quota_windows
        .iter()
        .filter(visible_window)
        .filter(|window| primary.is_none_or(|primary| !std::ptr::eq(*window, primary)))
        .min_by_key(|window| window.window_minutes.unwrap_or(u64::MAX));
    let mut title_parts = Vec::with_capacity(2);
    if let Some(window) = primary {
        title_parts.push(compact_window(window, true));
    }
    if let Some(window) = secondary {
        title_parts.push(compact_window(window, false));
    }
    let mut title = if title_parts.is_empty() {
        "Quota —".to_owned()
    } else {
        title_parts.join(" · ")
    };
    let degraded = status.data_status.availability != "complete"
        || !matches!(
            status.data_status.compatibility.as_str(),
            "tested" | "expected_compatible"
        );
    if degraded {
        title.push_str(" !");
    } else if status
        .quota_freshness
        .as_deref()
        .unwrap_or(&status.data_status.freshness)
        == "stale"
    {
        title.push_str(" ~");
    }

    let plan = status
        .account
        .as_ref()
        .and_then(|account| account.plan_type.as_deref())
        .filter(|plan| !plan.trim().is_empty())
        .unwrap_or("ChatGPT");
    let freshness = if status
        .quota_freshness
        .as_deref()
        .unwrap_or(&status.data_status.freshness)
        == "live"
    {
        "live"
    } else {
        status
            .quota_freshness
            .as_deref()
            .unwrap_or(&status.data_status.freshness)
    };
    CapacityPresentation {
        tooltip: format!(
            "QuotaHorizon · {plan} · {freshness} · updated {}",
            status
                .quota_observed_at
                .as_deref()
                .unwrap_or(&status.captured_at)
        ),
        title,
    }
}

fn compact_window(window: &CapacityWindow, primary: bool) -> String {
    let label = match window.window_minutes {
        Some(10_080) => "W".to_owned(),
        Some(minutes) if minutes % 10_080 == 0 => format!("{}w", minutes / 10_080),
        Some(minutes) if minutes % 60 == 0 => format!("{}h", minutes / 60),
        Some(minutes) => format!("{}m", minutes),
        None if primary => "Q".to_owned(),
        None => "S".to_owned(),
    };
    format!(
        "{label} {}",
        super::quota_percent_label(window.remaining_percent)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccountPrivateDetails, UsageSummary, UsageWindow};

    fn envelope(payload: &str) -> CapacityEnvelope {
        serde_json::from_str(payload).expect("valid tray fixture")
    }

    #[test]
    fn live_capacity_shows_week_and_short_window() {
        let presentation = capacity_presentation(&envelope(
            r#"{
                "lifecycle":"ready",
                "status":{
                    "capturedAt":"2026-09-02T07:00:00Z",
                    "account":{"planType":"plus"},
                    "dataStatus":{"availability":"complete","freshness":"live","compatibility":"tested"},
                    "quotaWindows":[
                        {"windowMinutes":300,"remainingPercent":94.4},
                        {"windowMinutes":10080,"remainingPercent":71.6}
                    ]
                },
                "issue":null
            }"#,
        ));

        assert_eq!(presentation.title, "W 71.6% · 5h 94.4%");
        assert!(presentation.tooltip.contains("plus · live"));
    }

    #[test]
    fn newer_quota_metadata_and_canonical_pro_window_drive_the_title() {
        let presentation = capacity_presentation(&envelope(
            r#"{
                "lifecycle":"ready",
                "status":{
                    "capturedAt":"2026-09-05T00:00:00Z",
                    "quotaObservedAt":"2026-09-05T00:01:00Z",
                    "quotaFreshness":"stale",
                    "account":{"planType":"pro"},
                    "dataStatus":{"availability":"complete","freshness":"live","compatibility":"tested"},
                    "quotaWindows":[
                        {"limitId":"codex_bengalfox:primary","windowMinutes":300,"remainingPercent":100},
                        {"limitId":"codex_bengalfox:secondary","windowMinutes":10080,"remainingPercent":100},
                        {"limitId":"codex:primary","windowMinutes":300,"remainingPercent":100},
                        {"limitId":"codex:secondary","windowMinutes":10080,"remainingPercent":28}
                    ]
                },
                "issue":null
            }"#,
        ));
        assert_eq!(presentation.title, "W 28% ~");
        assert!(presentation.tooltip.contains("pro · stale"));
        assert!(presentation.tooltip.ends_with("2026-09-05T00:01:00Z"));
    }

    #[test]
    fn stale_or_partial_capacity_never_looks_live() {
        let presentation = capacity_presentation(&envelope(
            r#"{
                "lifecycle":"stale",
                "status":{
                    "capturedAt":"2026-09-02T07:00:00Z",
                    "account":null,
                    "dataStatus":{"availability":"partial","freshness":"stale","compatibility":"not_tested"},
                    "quotaWindows":[{"windowMinutes":10080,"remainingPercent":40}]
                },
                "issue":{"code":"app_server_timeout"}
            }"#,
        ));

        assert_eq!(presentation.title, "W 40% !");
    }

    #[test]
    fn unavailable_capacity_uses_actionable_compact_states() {
        let selecting = capacity_presentation(&envelope(
            r#"{"lifecycle":"selection_required","status":null,"issue":null}"#,
        ));
        let failed = capacity_presentation(&envelope(
            r#"{"lifecycle":"error","status":null,"issue":{"code":"authentication_required"}}"#,
        ));

        assert_eq!(selecting.title, "Quota ?");
        assert_eq!(failed.title, "Quota !");
        assert!(failed.tooltip.contains("authentication_required"));
    }

    #[test]
    fn stored_pro_account_overrides_synthetic_capacity_with_weekly_only_usage() {
        let account = AccountSummary {
            id: "account".to_string(),
            email: "masked@example.com".to_string(),
            note: String::new(),
            expires_at: String::new(),
            private_details: AccountPrivateDetails::default(),
            plan: "prolite".to_string(),
            account_id: None,
            active: true,
            auto_switch_enabled: true,
            auto_switch_priority: 0,
            auto_switch_threshold: 0.0,
            local_proxy_compatible: true,
            direct_switch_compatible: true,
            agent_identity: false,
            official: true,
            metadata_editable: false,
            usage: UsageSummary {
                primary: Some(UsageWindow {
                    used_percent: 51.0,
                    remaining_percent: 49.0,
                    resets_at: Some(1_788_753_157),
                    window_minutes: Some(10_080),
                }),
                plan: Some("prolite".to_string()),
                fetched_at: Some("2026-09-03T18:27:20Z".to_string()),
                ..UsageSummary::default()
            },
        };

        let presentation = stored_account_presentation(&account).unwrap();
        assert_eq!(presentation.title, "W 49%");
        assert!(presentation.tooltip.contains("saved account quota"));
    }
}
