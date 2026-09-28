use super::*;
use crate::DesktopStatusEnvelope;
use capacity_domain::UtcTimestamp;
use capacity_store::{PlanningArchiveQuery, PlanningArchiveResult};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPlanningArchiveEnvelope {
    pub history_context_id: String,
    pub query: PlanningArchiveQuery,
    pub status: &'static str,
    pub reason_code: &'static str,
    pub result: Option<PlanningArchiveResult>,
    pub generated_at: UtcTimestamp,
}
impl DesktopPlanningArchiveEnvelope {
    pub(crate) fn unavailable(
        context: &str,
        query: PlanningArchiveQuery,
        reason: &'static str,
        now: &UtcTimestamp,
    ) -> Self {
        Self {
            history_context_id: context.to_owned(),
            query,
            status: "unavailable",
            reason_code: reason,
            result: None,
            generated_at: now.clone(),
        }
    }
}
impl DesktopPersistence {
    pub(crate) fn planning_archive(
        &self,
        context: &str,
        query: PlanningArchiveQuery,
        status: &DesktopStatusEnvelope,
        now: &UtcTimestamp,
    ) -> DesktopPlanningArchiveEnvelope {
        let mut result = DesktopPlanningArchiveEnvelope::unavailable(
            context,
            query,
            "plan_binding_required",
            now,
        );
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
        let AccountBinding::Stable(fp) = &self.binding else {
            return result;
        };
        let Some(env) = self.environment_id.as_deref() else {
            return result;
        };
        let Some(store) = self.store.as_ref() else {
            result.reason_code = "plan_store_unavailable";
            return result;
        };
        match store.planning_archive(fp, env, &result.query) {
            Ok(Some(value)) => {
                result.status = "available";
                result.reason_code = "archive_available";
                result.result = Some(value);
            }
            Ok(None) => {
                result.status = "missing";
                result.reason_code = "archive_missing";
            }
            Err(StoreError::InvalidMetadata(_)) => {
                result.status = "invalid_request";
                result.reason_code = "archive_invalid_request";
            }
            Err(_) => {
                result.status = "failed";
                result.reason_code = "archive_read_failed";
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesktopStatusView, persistence::tests::fixture_status};
    use capacity_domain::AccountFingerprint;
    #[test]
    fn archive_requires_live_context_and_stable_binding_without_authorization_prompts() {
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
        let query = PlanningArchiveQuery::List { offset: 0 };
        let available = p.planning_archive(&context, query.clone(), &status, &snapshot.captured_at);
        assert_eq!(available.status, "available");
        let stale =
            p.planning_archive("old-context", query.clone(), &status, &snapshot.captured_at);
        assert_eq!(stale.history_context_id, "old-context");
        assert!(stale.result.is_none());
        p.set_binding_failure("history_keychain_denied");
        let denied = p.planning_archive(&context, query.clone(), &status, &snapshot.captured_at);
        assert!(denied.result.is_none());
        p.rotate_ephemeral_binding();
        status.history_context_id = Some(p.history_context_id.clone());
        let ephemeral =
            p.planning_archive(&p.history_context_id, query, &status, &snapshot.captured_at);
        assert!(ephemeral.result.is_none());
        for value in [available, stale, denied, ephemeral] {
            let json = serde_json::to_string(&value).unwrap();
            assert!(!json.contains("environmentId"));
            assert!(!json.contains("hmac-sha256"));
        }
    }
}
