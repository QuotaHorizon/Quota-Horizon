use capacity_domain::{
    AccountBindingStatus, Availability, Diagnostic, DiagnosticSeverity, Freshness, ReasonCode,
    StatusSnapshot,
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleFallbackReason {
    ProcessFailed,
    Timeout,
    ProtocolError,
    ExecutableChanged,
}

impl StaleFallbackReason {
    const fn reason_code(self) -> &'static str {
        match self {
            Self::ProcessFailed => "app_server_process_failed",
            Self::Timeout => "app_server_timeout",
            Self::ProtocolError => "app_server_protocol_error",
            Self::ExecutableChanged => "codex_executable_changed",
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::ProcessFailed => {
                "The live app-server process failed; this is the last successful session snapshot."
            }
            Self::Timeout => {
                "The live app-server read timed out; this is the last successful session snapshot."
            }
            Self::ProtocolError => {
                "The live app-server response failed validation; this is the last successful session snapshot."
            }
            Self::ExecutableChanged => {
                "The selected Codex executable changed; this is the last successful session snapshot and the replacement must be reviewed."
            }
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SessionCacheError {
    #[error("the snapshot did not satisfy status JSON v1")]
    InvalidStatus,
}

/// A process-local cache for one explicitly managed account session.
///
/// This type intentionally has no serialization or storage API. Its owner must
/// discard it when `account/updated` is observed or when the executable/session
/// boundary changes.
#[derive(Debug, Default)]
pub struct SessionSnapshotCache {
    last_success: Option<StatusSnapshot>,
}

impl SessionSnapshotCache {
    pub const fn new() -> Self {
        Self { last_success: None }
    }

    pub fn observe(&mut self, snapshot: StatusSnapshot) -> Result<bool, SessionCacheError> {
        snapshot
            .validate()
            .map_err(|_| SessionCacheError::InvalidStatus)?;

        let cacheable = matches!(
            snapshot.data_status.availability,
            Availability::Complete | Availability::Partial
        ) && snapshot.data_status.freshness == Freshness::Live
            && !snapshot.quota.windows.is_empty()
            && snapshot.codex_executable.is_some()
            && snapshot.account.as_ref().is_some_and(|account| {
                matches!(
                    account.binding_status,
                    AccountBindingStatus::Stable | AccountBindingStatus::Ephemeral
                )
            });

        if cacheable {
            self.last_success = Some(snapshot);
        } else {
            self.last_success = None;
        }
        Ok(cacheable)
    }

    pub fn invalidate_account_change(&mut self) {
        self.last_success = None;
    }

    pub fn stale_after_failure(&self, failure: StaleFallbackReason) -> Option<StatusSnapshot> {
        let mut snapshot = self.last_success.clone()?;
        snapshot.data_status.freshness = Freshness::Stale;
        push_reason(&mut snapshot.data_status.reason_codes, "stale_source");
        push_reason(
            &mut snapshot.data_status.reason_codes,
            failure.reason_code(),
        );
        snapshot.diagnostics.push(Diagnostic {
            code: reason(failure.reason_code()),
            severity: DiagnosticSeverity::Warning,
            message: failure.message().to_owned(),
        });
        snapshot.validate().ok()?;
        Some(snapshot)
    }

    pub const fn has_snapshot(&self) -> bool {
        self.last_success.is_some()
    }
}

fn push_reason(codes: &mut Vec<ReasonCode>, code: &'static str) {
    if !codes.iter().any(|existing| existing.as_str() == code) {
        codes.push(reason(code));
    }
}

fn reason(code: &'static str) -> ReasonCode {
    ReasonCode::new(code).expect("static session-cache reason code must be valid")
}

#[cfg(test)]
mod tests {
    use capacity_domain::StatusSnapshot;

    use super::*;

    fn fixture_status(name: &str) -> StatusSnapshot {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/app-server")
                .join(name)
                .join("expected-status.json"),
        )
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn transient_failure_returns_original_values_as_stale() {
        let live = fixture_status("plus-normal");
        let original_capture = live.captured_at.clone();
        let original_windows = live.quota.windows.clone();
        let mut cache = SessionSnapshotCache::new();

        assert!(cache.observe(live).unwrap());
        let stale = cache
            .stale_after_failure(StaleFallbackReason::Timeout)
            .unwrap();

        assert_eq!(stale.captured_at, original_capture);
        assert_eq!(stale.quota.windows, original_windows);
        assert_eq!(stale.data_status.freshness, Freshness::Stale);
        assert!(
            stale
                .data_status
                .reason_codes
                .iter()
                .any(|code| code.as_str() == "stale_source")
        );
        assert!(
            stale
                .data_status
                .reason_codes
                .iter()
                .any(|code| code.as_str() == "app_server_timeout")
        );
    }

    #[test]
    fn account_update_immediately_removes_the_fallback() {
        let mut cache = SessionSnapshotCache::new();
        assert!(cache.observe(fixture_status("plus-normal")).unwrap());

        cache.invalidate_account_change();

        assert!(!cache.has_snapshot());
        assert!(
            cache
                .stale_after_failure(StaleFallbackReason::ProcessFailed)
                .is_none()
        );
    }

    #[test]
    fn unsupported_or_failed_status_is_never_a_last_success() {
        let mut cache = SessionSnapshotCache::new();
        assert!(cache.observe(fixture_status("plus-normal")).unwrap());

        assert!(!cache.observe(fixture_status("no-fixed-window")).unwrap());
        assert!(!cache.has_snapshot());
        assert!(!cache.observe(fixture_status("malformed-response")).unwrap());
        assert!(!cache.has_snapshot());
    }

    #[test]
    fn stale_status_is_never_accepted_as_a_new_success() {
        let mut stale = fixture_status("plus-normal");
        stale.data_status.freshness = Freshness::Stale;
        stale.data_status.reason_codes.push(reason("stale_source"));
        let mut cache = SessionSnapshotCache::new();

        assert!(!cache.observe(stale).unwrap());
        assert!(!cache.has_snapshot());
    }

    #[test]
    fn every_fallback_reason_uses_a_fixed_safe_diagnostic() {
        let mut cache = SessionSnapshotCache::new();
        assert!(cache.observe(fixture_status("plus-normal")).unwrap());

        for failure in [
            StaleFallbackReason::ProcessFailed,
            StaleFallbackReason::Timeout,
            StaleFallbackReason::ProtocolError,
            StaleFallbackReason::ExecutableChanged,
        ] {
            let stale = cache.stale_after_failure(failure).unwrap();
            let diagnostic = stale.diagnostics.last().unwrap();
            assert_eq!(diagnostic.code.as_str(), failure.reason_code());
            assert!(!diagnostic.message.contains('/'));
            assert!(!diagnostic.message.contains("Bearer"));
        }
    }
}
