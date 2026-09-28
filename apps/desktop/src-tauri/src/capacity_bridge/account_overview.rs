use capacity_desktop_service::{DesktopWorkPlanEnvelope, DesktopWorkPlanQuotaObservation};
use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::AppHandle;

use crate::models::{AccountSummary, UsageSummary};

/// The popover needs quota and plan data, not passwords, tokens, 2FA or paths.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountOverview {
    id: String,
    email: String,
    note: String,
    plan: String,
    active: bool,
    usage: UsageSummary,
    work_plan: Option<DesktopWorkPlanEnvelope>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageWindow;

    fn account(id: &str, active: bool) -> AccountSummary {
        AccountSummary {
            id: id.into(),
            active,
            email: "fixture@example.com".into(),
            note: String::new(),
            plan: "pro".into(),
            expires_at: String::new(),
            private_details: Default::default(),
            account_id: Some("private-upstream-id".into()),
            auto_switch_enabled: false,
            auto_switch_priority: 0,
            auto_switch_threshold: 0.0,
            local_proxy_compatible: true,
            direct_switch_compatible: true,
            agent_identity: false,
            official: false,
            metadata_editable: true,
            usage: UsageSummary {
                primary: Some(UsageWindow {
                    remaining_percent: 15.0,
                    used_percent: 85.0,
                    window_minutes: Some(10080),
                    resets_at: Some(1789257600),
                }),
                fetched_at: Some("2026-09-06T10:00:00Z".into()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn observed_runtime_identity_overrides_stale_manager_selection_without_switching() {
        let mut rows = vec![
            account("old-selection", true),
            account("actual-login", false),
        ];
        mark_observed_account(&mut rows, Some("actual-login"));
        assert!(!rows[0].active);
        assert!(rows[1].active);
        mark_observed_account(&mut rows, None);
        assert!(rows.iter().all(|row| !row.active));
    }

    #[test]
    fn weekly_only_pro_uses_its_primary_window_and_rejects_unknown_or_invalid_inputs() {
        let mut row = account("pro", true);
        assert!(weekly_observation(&row).is_some());
        row.usage.primary.as_mut().unwrap().window_minutes = None;
        assert!(weekly_observation(&row).is_none());
        row.usage.primary.as_mut().unwrap().window_minutes = Some(10080);
        row.usage.primary.as_mut().unwrap().remaining_percent = f64::NAN;
        assert!(weekly_observation(&row).is_none());
        row.usage.primary.as_mut().unwrap().remaining_percent = 15.0;
        row.usage.fetched_at = None;
        assert!(weekly_observation(&row).is_none());
    }

    #[test]
    fn overview_dto_has_no_private_details_or_upstream_identity() {
        let row = account("pro", true);
        let minimal = AccountOverview {
            id: row.id,
            email: row.email,
            note: row.note,
            plan: row.plan,
            active: row.active,
            usage: row.usage,
            work_plan: None,
        };
        let value = serde_json::to_value(minimal).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 7);
        assert!(value.get("privateDetails").is_none());
        assert!(value.get("accountId").is_none());
        assert!(!value.to_string().contains("private-upstream-id"));
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountOverviewEnvelope {
    schema_version: &'static str,
    privacy_mode: bool,
    accounts: Vec<AccountOverview>,
}

fn weekly_observation(account: &AccountSummary) -> Option<DesktopWorkPlanQuotaObservation> {
    let (_, weekly) = crate::system_tray::account_usage_windows(&account.usage, &account.plan);
    let weekly = weekly?;
    DesktopWorkPlanQuotaObservation::from_managed_account_weekly(
        account.usage.fetched_at.clone()?,
        u64::try_from(weekly.window_minutes?).ok()?,
        weekly.remaining_percent,
        DateTime::<Utc>::from_timestamp(weekly.resets_at?, 0)?.to_rfc3339(),
    )
}

fn mark_observed_account(accounts: &mut [AccountSummary], observed_id: Option<&str>) {
    for account in accounts {
        // Manager selection is an operation setting, not proof of which login
        // an external Codex client currently uses. This is a read-only badge.
        account.active = observed_id == Some(account.id.as_str());
    }
}

#[tauri::command]
pub(crate) async fn capacity_get_account_overviews(
    app_handle: AppHandle,
) -> Result<AccountOverviewEnvelope, String> {
    let worker = app_handle.clone();
    let (accounts, privacy_mode) = tauri::async_runtime::spawn_blocking(move || {
        let mut accounts = crate::commands::list_accounts_blocking(worker.clone())?;
        let paths = crate::storage::resolve_paths(&worker)?;
        let observed_id = super::capacity_account_digest_for_paths(&paths)
            .ok()
            .flatten();
        mark_observed_account(&mut accounts, observed_id.as_deref());
        let settings = crate::storage::read_app_settings(&worker)?;
        Ok::<_, String>((accounts, settings.privacy_mode))
    })
    .await
    .map_err(|_| "Unable to read the saved-account overview".to_string())??;
    let observations = accounts
        .iter()
        .map(|account| (account.id.clone(), weekly_observation(account)))
        .collect();
    let plans = capacity_desktop_service::work_plans_for_saved_accounts(app_handle, observations)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    let mut plans = plans;
    Ok(AccountOverviewEnvelope {
        schema_version: "1.0",
        privacy_mode,
        accounts: accounts
            .into_iter()
            .map(|account| AccountOverview {
                work_plan: plans.remove(&account.id),
                id: account.id,
                email: account.email,
                note: account.note,
                plan: account.plan,
                active: account.active,
                usage: account.usage,
            })
            .collect(),
    })
}
