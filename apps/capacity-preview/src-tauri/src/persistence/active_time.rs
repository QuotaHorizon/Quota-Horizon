use super::*;
use crate::DesktopStatusEnvelope;
use capacity_domain::{
    UtcTimestamp,
    active_time::{ActiveTimeAction, ActiveTimeObservation, ActiveTimer},
};
use capacity_store::ActiveTimeWriteOutcome;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopActiveTimeQuotaEnvelope {
    pub schema_version: &'static str,
    pub history_context_id: String,
    pub observation_id: String,
    pub status: &'static str,
    pub reason_code: &'static str,
    pub record: Option<ActiveTimeObservation>,
    pub comparison: Option<capacity_domain::activity_quota::ActivityQuotaComparison>,
    pub generated_at: UtcTimestamp,
}

impl DesktopActiveTimeQuotaEnvelope {
    pub(crate) fn unavailable(
        context: &str,
        id: &str,
        reason: &'static str,
        now: &UtcTimestamp,
    ) -> Self {
        Self {
            schema_version: "1.0",
            history_context_id: context.to_owned(),
            observation_id: id.to_owned(),
            status: "unavailable",
            reason_code: reason,
            record: None,
            comparison: None,
            generated_at: now.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopActiveTimeUpdate {
    pub history_context_id: String,
    pub action: ActiveTimeAction,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopActiveTimeEnvelope {
    pub schema_version: &'static str,
    pub history_context_id: String,
    pub status: &'static str,
    pub reason_code: &'static str,
    pub revision: u64,
    pub timer: Option<ActiveTimer>,
    pub observations: Vec<ActiveTimeObservation>,
    pub included_count_30_days: u64,
    pub included_seconds_30_days: u64,
    pub has_other_environment_records: bool,
    pub generated_at: UtcTimestamp,
}

impl DesktopActiveTimeEnvelope {
    pub(crate) fn unavailable(context: &str, reason: &'static str, now: &UtcTimestamp) -> Self {
        Self {
            schema_version: "1.0",
            history_context_id: context.to_owned(),
            status: "unavailable",
            reason_code: reason,
            revision: 0,
            timer: None,
            observations: vec![],
            included_count_30_days: 0,
            included_seconds_30_days: 0,
            has_other_environment_records: false,
            generated_at: now.clone(),
        }
    }
}

impl DesktopPersistence {
    pub(crate) fn active_time_quota(
        &self,
        context: &str,
        id: &str,
        status: &DesktopStatusEnvelope,
        now: &UtcTimestamp,
    ) -> DesktopActiveTimeQuotaEnvelope {
        let mut result = DesktopActiveTimeQuotaEnvelope::unavailable(
            context,
            id,
            "activity_binding_required",
            now,
        );
        if id.len() != 36 || Uuid::parse_str(id).is_err() {
            result.status = "invalid_request";
            result.reason_code = "activity_input_invalid";
            return result;
        }
        if context != self.history_context_id
            || status.history_context_id.as_deref() != Some(context)
        {
            result.reason_code = "activity_context_changed";
            return result;
        }
        if let Some(reason) = self.binding_failure_reason {
            result.reason_code = reason;
            return result;
        }
        if self.last_account_status != AccountBindingStatus::Stable {
            return result;
        }
        let AccountBinding::Stable(fp) = &self.binding else {
            return result;
        };
        let Some(env) = self.environment_id.as_deref() else {
            return result;
        };
        let Some(store) = self.store.as_ref() else {
            result.reason_code = "activity_store_unavailable";
            return result;
        };
        match store.active_time_quota_evidence(fp, env, id) {
            Ok(Some(value)) => {
                result.status = "available";
                result.reason_code = "activity_evidence_available";
                result.record = Some(value.record);
                result.comparison = Some(value.comparison);
            }
            Ok(None) => {
                result.status = "missing";
                result.reason_code = "activity_record_missing";
            }
            Err(_) => {
                result.status = "failed";
                result.reason_code = "activity_evidence_failed";
            }
        }
        result
    }

    pub(crate) fn interrupt_activity(&mut self) {
        if self
            .store
            .as_mut()
            .is_some_and(|store| store.interrupt_active_time().is_err())
        {
            self.history_context_id = Uuid::new_v4().to_string();
        }
        // On failure, the newly rotated history context still forces recovery
        // on the next successful read; no old draft can resume silently.
    }

    pub(crate) fn active_time(
        &mut self,
        context: &str,
        request: Option<DesktopActiveTimeUpdate>,
        status: &DesktopStatusEnvelope,
        now: &UtcTimestamp,
    ) -> DesktopActiveTimeEnvelope {
        let mut result =
            DesktopActiveTimeEnvelope::unavailable(context, "activity_binding_required", now);
        if context != self.history_context_id
            || status.history_context_id.as_deref() != Some(context)
            || request
                .as_ref()
                .is_some_and(|request| request.history_context_id != context)
        {
            result.reason_code = "activity_context_changed";
            return result;
        }
        if let Some(reason) = self.binding_failure_reason {
            result.reason_code = reason;
            return result;
        }
        if self.last_account_status != AccountBindingStatus::Stable {
            return result;
        }
        let AccountBinding::Stable(fp) = &self.binding else {
            return result;
        };
        let Some(env) = self.environment_id.as_deref() else {
            return result;
        };
        let Some(store) = self.store.as_mut() else {
            result.reason_code = "activity_store_unavailable";
            return result;
        };
        // Always checkpoint/recover before applying a revision-checked action.
        let mut view = match store.active_time(fp, env, context, now) {
            Ok(view) => view,
            Err(_) => {
                result.status = "failed";
                result.reason_code = "activity_read_failed";
                return result;
            }
        };
        result.status = "available";
        result.reason_code = "activity_available";
        if let Some(request) = request {
            let (state, reason) =
                match store.update_active_time(fp, env, context, &request.action, now) {
                    Ok(ActiveTimeWriteOutcome::Updated) => ("updated", "activity_saved"),
                    Ok(ActiveTimeWriteOutcome::RevisionConflict) => {
                        ("revision_conflict", "activity_revision_conflict")
                    }
                    Ok(ActiveTimeWriteOutcome::Overlap) => ("invalid_request", "activity_overlap"),
                    Ok(ActiveTimeWriteOutcome::TimerOpen) => {
                        ("invalid_request", "activity_timer_open")
                    }
                    Err(StoreError::ActiveTimeValidation(_))
                    | Err(StoreError::InvalidMetadata(_)) => {
                        ("invalid_request", "activity_input_invalid")
                    }
                    Err(_) => ("failed", "activity_save_failed"),
                };
            result.status = state;
            result.reason_code = reason;
            match store.active_time(fp, env, context, now) {
                Ok(next) => view = next,
                Err(_) => {
                    result.status = "failed";
                    result.reason_code = "activity_read_failed";
                    return result;
                }
            }
        }
        result.revision = view.revision;
        result.timer = view.timer;
        result.observations = view.observations;
        result.included_count_30_days = view.included_count_30_days;
        result.included_seconds_30_days = view.included_seconds_30_days;
        result.has_other_environment_records = view.has_other_environment_records;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesktopStatusView, persistence::tests::fixture_status};
    use capacity_domain::AccountFingerprint;
    fn fixture() -> (DesktopPersistence, DesktopStatusEnvelope, UtcTimestamp) {
        let mut snapshot = fixture_status("plus-normal");
        snapshot.account.as_mut().unwrap().binding_status = AccountBindingStatus::Stable;
        let fp = AccountFingerprint::parse(format!(
            "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:{}",
            "a".repeat(64)
        ))
        .unwrap();
        let mut p = DesktopPersistence::in_memory(AccountBinding::Stable(fp));
        p.record_snapshot(&snapshot);
        let mut envelope = crate::envelope_from_discovery(None, None, None);
        envelope.status = Some(DesktopStatusView::from(&snapshot));
        envelope.history_context_id = Some(p.history_context_id.clone());
        let now = snapshot.captured_at.clone();
        (p, envelope, now)
    }
    #[test]
    fn stale_context_and_failed_binding_expose_no_records() {
        let (mut p, status, now) = fixture();
        let context = p.history_context_id.clone();
        let request = DesktopActiveTimeUpdate {
            history_context_id: context.clone(),
            action: ActiveTimeAction::Start {
                timer_id: Uuid::new_v4().to_string(),
            },
        };
        let first = p.active_time(&context, Some(request), &status, &now);
        assert_eq!(first.status, "updated");
        let json = serde_json::to_string(&first).unwrap();
        assert!(!json.contains("hmac-sha256"));
        assert!(!json.contains("environmentId"));
        assert!(!json.contains("ownerContextId"));
        let stale = p.active_time("old", None, &status, &now);
        assert!(stale.timer.is_none());
        assert_eq!(stale.history_context_id, "old");
        p.set_binding_failure("history_key_authorization_required");
        let denied = p.active_time(&context, None, &status, &now);
        assert!(denied.timer.is_none());
        assert!(denied.observations.is_empty());
        let mut restored = status;
        restored.history_context_id = Some(p.history_context_id.clone());
        p.binding_failure_reason = None;
        let recovered = p.active_time(&p.history_context_id.clone(), None, &restored, &now);
        assert!(recovered.timer.unwrap().interrupted);
    }
    #[test]
    fn environment_and_account_loss_interrupt_drafts_without_saving_activity() {
        let (mut p, status, now) = fixture();
        let context = p.history_context_id.clone();
        p.active_time(
            &context,
            Some(DesktopActiveTimeUpdate {
                history_context_id: context.clone(),
                action: ActiveTimeAction::Start {
                    timer_id: Uuid::new_v4().to_string(),
                },
            }),
            &status,
            &now,
        );
        p.rotate_ephemeral_binding();
        let result = p.active_time(&context, None, &status, &now);
        assert!(result.timer.is_none());
        assert_eq!(result.status, "unavailable");
    }
    #[test]
    fn evidence_is_context_guarded_and_does_not_create_or_authorize_history() {
        let (mut p, status, now) = fixture();
        let context = p.history_context_id.clone();
        let id = Uuid::new_v4().to_string();
        let missing = p.active_time_quota(&context, &id, &status, &now);
        assert_eq!(missing.status, "missing");
        assert!(missing.record.is_none());
        let changed = p.active_time_quota("old-context", &id, &status, &now);
        assert_eq!(changed.reason_code, "activity_context_changed");
        assert!(changed.comparison.is_none());
        assert_eq!(changed.history_context_id, "old-context");
        p.set_binding_failure("history_keychain_denied");
        let denied = p.active_time_quota(&context, &id, &status, &now);
        assert!(denied.record.is_none());
        assert!(denied.comparison.is_none());
        assert!(
            !serde_json::to_string(&denied)
                .unwrap()
                .contains("hmac-sha256")
        );
    }
}
