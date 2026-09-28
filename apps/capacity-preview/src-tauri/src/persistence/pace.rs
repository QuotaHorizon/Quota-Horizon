use super::*;
use crate::DesktopStatusEnvelope;
use capacity_domain::{
    UtcTimestamp,
    pace::{PaceCurrent, PaceEstimate},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPaceEstimate {
    pub pace_estimate_id: String,
    #[serde(flatten)]
    pub estimate: PaceEstimate,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPaceEnvelope {
    pub schema_version: &'static str,
    pub history_context_id: String,
    pub status: &'static str,
    pub reason_code: &'static str,
    pub estimates: Vec<DesktopPaceEstimate>,
    pub generated_at: UtcTimestamp,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPaceTrialsEnvelope {
    pub history_context_id: String,
    pub status: &'static str,
    pub reason_code: &'static str,
    pub offset: u32,
    pub has_more: bool,
    pub recording_failed: bool,
    pub trials: Vec<capacity_domain::pace_trial::PaceTrialView>,
    pub summary: Option<capacity_domain::pace_evidence::PaceEvidenceSummary>,
}
impl DesktopPaceTrialsEnvelope {
    pub(crate) fn unavailable(context: &str, offset: u32, reason: &'static str) -> Self {
        Self {
            history_context_id: context.into(),
            status: "unavailable",
            reason_code: reason,
            offset,
            has_more: false,
            recording_failed: false,
            trials: vec![],
            summary: None,
        }
    }
}
impl DesktopPaceEnvelope {
    pub(crate) fn unavailable(context: &str, reason: &'static str, now: &UtcTimestamp) -> Self {
        Self {
            schema_version: "1.0",
            history_context_id: context.into(),
            status: "unavailable",
            reason_code: reason,
            estimates: vec![],
            generated_at: now.clone(),
        }
    }
}
impl DesktopPersistence {
    pub(crate) fn pace_trials(
        &self,
        context: &str,
        status: &DesktopStatusEnvelope,
        offset: u32,
    ) -> DesktopPaceTrialsEnvelope {
        let mut out =
            DesktopPaceTrialsEnvelope::unavailable(context, offset, "plan_binding_required");
        if context != self.history_context_id
            || status.history_context_id.as_deref() != Some(context)
        {
            out.reason_code = "plan_context_changed";
            return out;
        }
        if let Some(reason) = self.binding_failure_reason {
            out.reason_code = reason;
            return out;
        }
        if self.last_account_status != AccountBindingStatus::Stable {
            return out;
        }
        let AccountBinding::Stable(fp) = &self.binding else {
            return out;
        };
        let Some(env) = self.environment_id.as_deref() else {
            return out;
        };
        let Some(store) = self.store.as_ref() else {
            out.reason_code = "plan_store_unavailable";
            return out;
        };
        match store.pace_trial_report(fp, env, offset, &SystemClock.now()) {
            Ok((mut trials, summary)) => {
                out.has_more = trials.len() > 20;
                trials.truncate(20);
                out.trials = trials;
                out.summary = Some(summary);
                out.status = "available";
                out.reason_code = "trials_available";
                out.recording_failed = self.pace_trials_failed;
            }
            Err(_) => {
                out.status = "failed";
                out.reason_code = "pace_read_failed";
            }
        }
        out
    }

    pub(crate) fn pace(
        &self,
        context: &str,
        status: &DesktopStatusEnvelope,
        now: &UtcTimestamp,
    ) -> DesktopPaceEnvelope {
        let mut out = DesktopPaceEnvelope::unavailable(context, "plan_binding_required", now);
        if context != self.history_context_id
            || status.history_context_id.as_deref() != Some(context)
        {
            out.reason_code = "plan_context_changed";
            return out;
        }
        if let Some(reason) = self.binding_failure_reason {
            out.reason_code = reason;
            return out;
        }
        if self.last_account_status != AccountBindingStatus::Stable {
            return out;
        }
        let AccountBinding::Stable(fp) = &self.binding else {
            return out;
        };
        let Some(env) = self.environment_id.as_deref() else {
            return out;
        };
        let Some(store) = self.store.as_ref() else {
            out.reason_code = "plan_store_unavailable";
            return out;
        };
        let Some(status) = status.status.as_ref() else {
            out.reason_code = "source_unavailable";
            return out;
        };
        let Some(observed_at) = UtcTimestamp::parse(
            status
                .quota_observed_at
                .as_ref()
                .unwrap_or(&status.captured_at),
        )
        .ok() else {
            out.reason_code = "source_unavailable";
            return out;
        };
        let plan = status
            .account
            .as_ref()
            .and_then(|account| account.plan_type.clone());
        let pro = capacity_domain::pace::normalize_plan_type(plan.as_deref())
            .is_some_and(|plan| matches!(plan.as_str(), "pro" | "prolite"));
        let windows: Vec<_> = status
            .quota_windows
            .iter()
            .filter(|w| {
                crate::canonical_codex_limit(&w.limit_id)
                    && !(pro && w.window_minutes.is_some_and(|m| m <= 1440))
            })
            .collect();
        if windows.len() > 8 {
            out.reason_code = "unsupported_window";
            return out;
        }
        out.status = "available";
        out.reason_code = "pace_available";
        for window in windows {
            let current = PaceCurrent {
                limit_id: window.limit_id.clone(),
                window_minutes: window.window_minutes,
                remaining_percent: window.remaining_percent,
                resets_at: window
                    .resets_at
                    .as_ref()
                    .and_then(|r| UtcTimestamp::parse(r).ok()),
                observed_at: observed_at.clone(),
                account_plan_type: plan.clone(),
                availability: status.data_status.availability,
                freshness: status
                    .quota_freshness
                    .unwrap_or(status.data_status.freshness),
                compatibility: status.data_status.compatibility,
            };
            match store.pace_estimate(fp, env, &current, now) {
                Ok(estimate) => out.estimates.push(DesktopPaceEstimate {
                    pace_estimate_id: Uuid::new_v4().to_string(),
                    estimate,
                }),
                Err(_) => {
                    out.status = "failed";
                    out.reason_code = "pace_read_failed";
                    out.estimates.clear();
                    return out;
                }
            }
        }
        out.estimates
            .sort_by_key(|v| std::cmp::Reverse(v.estimate.window_minutes));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesktopStatusView, persistence::tests::fixture_status};
    use capacity_domain::AccountFingerprint;
    #[test]
    fn guards_binding_and_treats_new_context_as_missing_evidence_not_zero() {
        let mut snapshot = fixture_status("plus-normal");
        snapshot.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
        let fp = AccountFingerprint::parse(format!(
            "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:{}",
            "a".repeat(64)
        ))
        .unwrap();
        let mut p = DesktopPersistence::in_memory(AccountBinding::Stable(fp));
        p.record_snapshot(&snapshot);
        let mut status = crate::envelope_from_discovery(None, None, None);
        status.status = Some(DesktopStatusView::from(&snapshot));
        let context = p.history_context_id.clone();
        status.history_context_id = Some(context.clone());
        let value = p.pace(&context, &status, &snapshot.captured_at);
        assert_eq!(value.status, "available");
        assert!(!value.estimates.is_empty());
        assert!(
            value
                .estimates
                .iter()
                .all(|v| v.estimate.state == "abstained"
                    && v.estimate.balance_at_horizon.lower.is_none())
        );
        let json = serde_json::to_string(&value).unwrap();
        assert!(!json.contains("hmac-sha256"));
        assert!(!json.contains("environmentId"));
        assert!(
            p.pace("old-context", &status, &snapshot.captured_at)
                .estimates
                .is_empty()
        );
        p.set_binding_failure("history_keychain_denied");
        assert!(
            p.pace(&context, &status, &snapshot.captured_at)
                .estimates
                .is_empty()
        );
    }
    #[test]
    fn pro_never_gets_a_synthetic_short_window_estimate() {
        let mut snapshot = fixture_status("plus-normal");
        snapshot.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
        snapshot.account.as_mut().unwrap().plan_type = Some("pro".into());
        let fp = AccountFingerprint::parse(format!(
            "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:{}",
            "a".repeat(64)
        ))
        .unwrap();
        let mut p = DesktopPersistence::in_memory(AccountBinding::Stable(fp));
        p.record_snapshot(&snapshot);
        let mut status = crate::envelope_from_discovery(None, None, None);
        status.status = Some(DesktopStatusView::from(&snapshot));
        status.history_context_id = Some(p.history_context_id.clone());
        let value = p.pace(&p.history_context_id, &status, &snapshot.captured_at);
        assert!(
            value
                .estimates
                .iter()
                .all(|e| e.estimate.window_minutes.is_none_or(|m| m > 1440))
        );
    }

    #[test]
    fn trial_reads_guard_context_and_expose_independent_bookkeeping_failures() {
        let mut snapshot = fixture_status("plus-normal");
        snapshot.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
        let fp = AccountFingerprint::parse(format!(
            "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:{}",
            "a".repeat(64)
        ))
        .unwrap();
        let mut p = DesktopPersistence::in_memory(AccountBinding::Stable(fp));
        assert_eq!(
            p.record_snapshot(&snapshot),
            PersistenceRecordResult::Persisted
        );
        let mut status = crate::envelope_from_discovery(None, None, None);
        status.status = Some(DesktopStatusView::from(&snapshot));
        status.history_context_id = Some(p.history_context_id.clone());
        let view = p.pace_trials(&p.history_context_id, &status, 0);
        assert_eq!(view.status, "available");
        assert!(view.trials.is_empty());
        assert_eq!(view.summary.as_ref().unwrap().counts.total, 0);
        assert!(p.pace_trials("old-context", &status, 0).summary.is_none());
        assert_eq!(
            p.pace_trials("old-context", &status, 0).status,
            "unavailable"
        );
        p.set_binding_failure("history_keychain_denied");
        assert!(
            p.pace_trials(&p.history_context_id, &status, 0)
                .trials
                .is_empty()
        );
        p.binding_failure_reason = None;
        p.pace_trials_failed = true;
        assert!(
            p.pace_trials(&p.history_context_id, &status, 0)
                .recording_failed
        );
        p.rotate_ephemeral_binding();
        assert!(!p.pace_trials_failed);
        assert!(
            p.pace_trials(&p.history_context_id, &status, 0)
                .trials
                .is_empty()
        );
    }
}
