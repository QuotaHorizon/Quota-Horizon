use super::*;
use crate::DesktopStatusEnvelope;
use capacity_domain::UtcTimestamp;
use capacity_domain::demand::{CapacityDemandInput, DemandAllowance, demand_allowance};
use capacity_store::{CapacityWorkPlan, CapacityWorkPlanUpdate, CapacityWorkPlanUpdateOutcome};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopDemandPlanUpdate {
    pub history_context_id: String,
    pub expected_revision: u32,
    pub enabled: bool,
    pub demand: CapacityDemandInput,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDemandPlanView {
    pub work_plan_id: String,
    pub revision: u32,
    pub demand_id: String,
    pub demand_revision: u32,
    pub enabled: bool,
    pub demand: CapacityDemandInput,
    pub created_at: UtcTimestamp,
}

impl From<CapacityWorkPlan> for DesktopDemandPlanView {
    fn from(plan: CapacityWorkPlan) -> Self {
        Self {
            work_plan_id: plan.work_plan_id,
            revision: plan.revision,
            demand_id: plan.demand_id,
            demand_revision: plan.demand_revision,
            enabled: plan.enabled,
            demand: plan.demand,
            created_at: plan.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDemandAllowance {
    pub limit_id: String,
    pub window_minutes: u64,
    pub remaining_percent: f64,
    pub resets_at: Option<String>,
    pub allocation: DemandAllowance,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDemandPlanEnvelope {
    pub schema_version: &'static str,
    pub history_context_id: String,
    pub status: &'static str,
    pub reason_code: &'static str,
    pub plans: Vec<DesktopDemandPlanView>,
    pub has_other_environment_plans: bool,
    pub allowances: Vec<DesktopDemandAllowance>,
    pub generated_at: UtcTimestamp,
    pub observed_at: Option<String>,
    pub freshness: capacity_domain::Freshness,
    /// This package records intent/allowances, not a calibrated forecast.
    pub decision: &'static str,
}

impl DesktopDemandPlanEnvelope {
    pub(crate) fn unavailable(context: &str, reason: &'static str, now: &UtcTimestamp) -> Self {
        Self {
            schema_version: "1.0",
            history_context_id: context.to_owned(),
            status: "unavailable",
            reason_code: reason,
            plans: vec![],
            has_other_environment_plans: false,
            allowances: vec![],
            generated_at: now.clone(),
            observed_at: None,
            freshness: capacity_domain::Freshness::NotApplicable,
            decision: "not_assessed",
        }
    }
}

impl DesktopPersistence {
    pub(crate) fn demand_plan(
        &mut self,
        context: &str,
        request: Option<DesktopDemandPlanUpdate>,
        status: &DesktopStatusEnvelope,
        now: &UtcTimestamp,
    ) -> DesktopDemandPlanEnvelope {
        let mut result =
            DesktopDemandPlanEnvelope::unavailable(context, "plan_binding_required", now);
        // Do not expose the new context or a different account's plan to a
        // stale WebView. The host, not the frontend, selects the stable scope.
        if context != self.history_context_id
            || status.history_context_id.as_deref() != Some(context)
        {
            result.reason_code = "plan_context_changed";
            return result;
        }
        if let Some(reason) = self.binding_failure_reason {
            result.reason_code = reason;
            return result;
        }
        if self.last_account_status != AccountBindingStatus::Stable {
            return result;
        }
        let AccountBinding::Stable(fingerprint) = &self.binding else {
            return result;
        };
        let Some(environment) = self.environment_id.as_deref() else {
            return result;
        };
        let Some(store) = self.store.as_mut() else {
            result.reason_code = "plan_store_unavailable";
            return result;
        };
        result.status = "available";
        result.reason_code = "plan_available";
        result.has_other_environment_plans =
            match store.has_capacity_work_plan_in_other_environment(fingerprint, environment) {
                Ok(value) => value,
                Err(_) => {
                    result.status = "failed";
                    result.reason_code = "plan_read_failed";
                    return result;
                }
            };
        if let Some(request) = request {
            if request.history_context_id != context {
                result.status = "unavailable";
                result.reason_code = "plan_context_changed";
                return result;
            }
            match store.update_capacity_work_plan(
                fingerprint,
                environment,
                &CapacityWorkPlanUpdate {
                    expected_revision: request.expected_revision,
                    enabled: request.enabled,
                    demand: request.demand,
                },
                now,
            ) {
                Ok(CapacityWorkPlanUpdateOutcome::Updated(_)) => result.status = "updated",
                Ok(CapacityWorkPlanUpdateOutcome::RevisionConflict(_)) => {
                    result.status = "revision_conflict";
                    result.reason_code = "plan_revision_conflict";
                }
                Err(StoreError::DemandValidation(_)) | Err(StoreError::InvalidMetadata(_)) => {
                    result.status = "invalid_request";
                    result.reason_code = "plan_input_invalid";
                }
                Err(_) => {
                    result.status = "failed";
                    result.reason_code = "plan_save_failed";
                }
            }
        }
        result.plans = match store.capacity_work_plans(fingerprint, environment) {
            Ok(plans) => plans.into_iter().map(Into::into).collect(),
            Err(_) => {
                result.status = "failed";
                result.reason_code = "plan_read_failed";
                return result;
            }
        };
        let Some(plan) = result.plans.first().filter(|plan| plan.enabled) else {
            return result;
        };
        let Some(status) = status.status.as_ref() else {
            return result;
        };
        result.observed_at = Some(
            status
                .quota_observed_at
                .as_ref()
                .unwrap_or(&status.captured_at)
                .clone(),
        );
        result.freshness = status
            .quota_freshness
            .unwrap_or(status.data_status.freshness);
        // This is arithmetic on observed facts, not a prediction permission.
        if result.freshness == capacity_domain::Freshness::NotApplicable
            || matches!(
                status.data_status.compatibility,
                capacity_domain::Compatibility::KnownBroken
                    | capacity_domain::Compatibility::Unsupported
            )
            || (status.quota_source.is_none()
                && matches!(
                    status.data_status.availability,
                    capacity_domain::Availability::Failed
                        | capacity_domain::Availability::Unsupported
                ))
        {
            return result;
        }
        let weekly_only = status
            .account
            .as_ref()
            .and_then(|account| account.plan_type.as_deref())
            .is_some_and(|plan| {
                matches!(
                    plan.to_ascii_lowercase().as_str(),
                    "pro" | "prolite" | "chatgptpro" | "chatgptprolite"
                )
            });
        result.allowances = status
            .quota_windows
            .iter()
            .filter_map(|window| {
                if !crate::canonical_codex_limit(&window.limit_id) {
                    return None;
                }
                let minutes = window.window_minutes.filter(|minutes| *minutes > 0)?;
                if weekly_only && minutes <= 1440 {
                    return None;
                }
                if !window.remaining_percent.is_finite()
                    || !(0.0..=100.0).contains(&window.remaining_percent)
                {
                    return None;
                }
                let reset = window
                    .resets_at
                    .as_ref()
                    .and_then(|value| UtcTimestamp::parse(value).ok());
                Some(DesktopDemandAllowance {
                    limit_id: window.limit_id.clone(),
                    window_minutes: minutes,
                    remaining_percent: window.remaining_percent,
                    resets_at: window.resets_at.clone(),
                    allocation: demand_allowance(
                        &plan.demand,
                        window.remaining_percent,
                        reset.as_ref(),
                        now,
                    ),
                })
            })
            .collect();
        result
            .allowances
            .sort_by_key(|allowance| std::cmp::Reverse(allowance.window_minutes));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capacity_domain::{AccountFingerprint, demand::DemandKind};
    fn at(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).unwrap()
    }
    fn fixture() -> (DesktopPersistence, DesktopStatusEnvelope, String) {
        let mut persistence = DesktopPersistence::unavailable();
        persistence.store = Some(CapacityStore::open_in_memory().unwrap());
        persistence.set_stable_binding(AccountFingerprint::parse("hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap());
        // Binding without a persisted observation may be read but cannot invent
        // an account_bindings row for saving. Exercise that failure too.
        persistence.last_account_status = AccountBindingStatus::Stable;
        persistence.environment_id = Some("test-environment".into());
        let context = persistence.history_context_id.clone();
        let mut status = crate::envelope_from_discovery(None, None, None);
        status.history_context_id = Some(context.clone());
        (persistence, status, context)
    }
    #[test]
    fn never_returns_or_writes_another_context_and_does_not_prompt_for_keys() {
        let (mut persistence, status, context) = fixture();
        let now = at("2026-09-26T04:00:00Z");
        let request = DesktopDemandPlanUpdate {
            history_context_id: context.clone(),
            expected_revision: 0,
            enabled: true,
            demand: CapacityDemandInput {
                horizon_end: at("2026-09-27T04:00:00Z"),
                demand_kind: DemandKind::MaintainRecentPace,
                planned_codex_active_hours: None,
            },
        };
        let changed = persistence.demand_plan("old-context", Some(request.clone()), &status, &now);
        assert_eq!(changed.reason_code, "plan_context_changed");
        assert!(changed.plans.is_empty());
        persistence.set_binding_failure("history_keychain_denied");
        let denied = persistence.demand_plan(&context, Some(request.clone()), &status, &now);
        assert_eq!(denied.reason_code, "history_keychain_denied");
        assert!(denied.plans.is_empty());
        persistence.binding_failure_reason = None;
        let no_capture = persistence.demand_plan(&context, Some(request), &status, &now);
        assert_eq!(no_capture.status, "failed");
        assert!(no_capture.plans.is_empty());
        persistence.rotate_ephemeral_binding();
        assert_eq!(
            persistence
                .demand_plan(&context, None, &status, &now)
                .reason_code,
            "plan_context_changed"
        );
    }

    #[test]
    fn saved_intent_and_allowances_use_current_scoped_quota_without_forecasts() {
        let (mut persistence, mut status, _) = fixture();
        let mut capture = super::super::tests::fixture_status("plus-normal");
        capture.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
        assert_eq!(
            persistence.record_snapshot(&capture),
            PersistenceRecordResult::Persisted
        );
        let context = persistence.history_context_id.clone();
        status.history_context_id = Some(context.clone());
        status.status = Some(crate::DesktopStatusView::from(&capture));
        let windows = &mut status.status.as_mut().unwrap().quota_windows;
        windows.clear();
        for (id, minutes, remaining) in [
            ("codex:primary", 10080, 40.0),
            ("codex:secondary", 300, 60.0),
            ("codex_model:primary", 10080, 100.0),
        ] {
            windows.push(crate::DesktopQuotaWindowView {
                limit_id: id.into(),
                label: None,
                window_minutes: Some(minutes),
                used_percent: 100.0 - remaining,
                remaining_percent: remaining,
                resets_at: Some(
                    if minutes == 300 {
                        "2026-09-26T05:00:00Z"
                    } else {
                        "2026-09-29T04:00:00Z"
                    }
                    .into(),
                ),
            });
        }
        let now = at("2026-09-26T04:00:00Z");
        let request = DesktopDemandPlanUpdate {
            history_context_id: context.clone(),
            expected_revision: 0,
            enabled: true,
            demand: CapacityDemandInput {
                horizon_end: at("2026-09-28T04:00:00Z"),
                demand_kind: DemandKind::ActiveHours,
                planned_codex_active_hours: Some(8.0),
            },
        };
        let result = persistence.demand_plan(&context, Some(request.clone()), &status, &now);
        assert_eq!(result.status, "updated");
        assert_eq!(result.decision, "not_assessed");
        assert_eq!(result.allowances.len(), 2);
        assert_eq!(result.allowances[0].allocation.daily_percent, Some(20.0));
        assert_eq!(
            result.allowances[1].allocation.reason_code,
            "reset_before_deadline"
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("hmac-sha256"));
        assert!(!json.contains("accountFingerprint"));
        assert!(!json.contains("test-environment"));
        assert!(!json.contains("codex_model"));
        assert_eq!(
            persistence
                .demand_plan(&context, Some(request), &status, &now)
                .status,
            "revision_conflict"
        );
        status
            .status
            .as_mut()
            .unwrap()
            .account
            .as_mut()
            .unwrap()
            .plan_type = Some("prolite".into());
        assert_eq!(
            persistence
                .demand_plan(&context, None, &status, &now)
                .allowances
                .len(),
            1
        );
        status.status.as_mut().unwrap().data_status.compatibility =
            capacity_domain::Compatibility::KnownBroken;
        let unavailable = persistence.demand_plan(&context, None, &status, &now);
        assert_eq!(unavailable.plans.len(), 1);
        assert!(unavailable.allowances.is_empty());
        persistence.set_binding_failure("history_keychain_denied");
        assert!(
            persistence
                .demand_plan(&context, None, &status, &now)
                .plans
                .is_empty()
        );
    }
}
