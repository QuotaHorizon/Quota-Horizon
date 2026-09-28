// QuotaHorizon: one cancellable login attempt; no network work holds the commit gate.
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::models::{LoginPhase, LoginStatus};

pub(super) const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

pub(super) struct LoginAttempt {
    pub(super) id: String,
    started: Instant,
    status: Mutex<LoginStatus>,
}

impl LoginAttempt {
    pub(super) fn new(id: String) -> Self {
        Self {
            status: Mutex::new(LoginStatus {
                ok: false,
                message: "Waiting for authorization".into(),
                account_id: None,
                login_id: Some(id.clone()),
                phase: Some(LoginPhase::Waiting),
                reason: None,
            }),
            id,
            started: Instant::now(),
        }
    }

    pub(super) fn snapshot(&self) -> LoginStatus {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(super) fn active(&self) -> bool {
        self.snapshot().phase.is_some_and(LoginPhase::active)
    }

    pub(super) fn expired(&self) -> bool {
        self.started.elapsed() >= LOGIN_TIMEOUT
    }

    pub(super) fn window_label(&self) -> String {
        format!("codex-login-{}", self.id)
    }

    pub(super) fn transition(
        &self,
        phase: LoginPhase,
        reason: Option<&str>,
    ) -> Option<LoginStatus> {
        let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        if !status.phase.is_some_and(LoginPhase::active) {
            return None;
        }
        if status.phase == Some(LoginPhase::Exchanging)
            && matches!(phase, LoginPhase::Waiting | LoginPhase::BrowserFallback)
        {
            return None;
        }
        status.phase = Some(phase);
        status.ok = phase == LoginPhase::Succeeded;
        status.reason = reason.map(str::to_owned);
        status.message = format!("Login {}", phase.name());
        Some(status.clone())
    }

    /// Cancellation and local persistence are linearized. If cancellation won,
    /// even a successful late token exchange cannot execute the save closure.
    pub(super) fn save(
        &self,
        persist: impl FnOnce() -> Result<String, String>,
    ) -> Option<LoginStatus> {
        let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        if !status.phase.is_some_and(LoginPhase::active) {
            return None;
        }
        if self.expired() {
            status.phase = Some(LoginPhase::TimedOut);
            status.message = "Login timed out".into();
        } else {
            match persist() {
                Ok(id) => {
                    status.ok = true;
                    status.phase = Some(LoginPhase::Succeeded);
                    status.message = "Account saved; current Codex login unchanged".into();
                    status.account_id = Some(id);
                }
                Err(_) => {
                    status.phase = Some(LoginPhase::Failed);
                    status.reason = Some("storage".into());
                    status.message = "Account could not be saved".into();
                }
            }
        }
        Some(status.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_discards_a_late_success_without_saving() {
        let attempt = LoginAttempt::new("old".into());
        attempt.transition(LoginPhase::Exchanging, None).unwrap();
        assert!(attempt.transition(LoginPhase::Cancelled, None).is_some());
        assert!(attempt
            .save(|| panic!("cancelled login must not save"))
            .is_none());
        assert!(attempt
            .transition(LoginPhase::Failed, Some("network"))
            .is_none());
        assert_eq!(attempt.snapshot().phase, Some(LoginPhase::Cancelled));
    }

    #[test]
    fn completed_login_cannot_be_cancelled_or_saved_twice() {
        let attempt = LoginAttempt::new("one".into());
        let result = attempt.save(|| Ok("account".into())).unwrap();
        assert!(result.ok);
        assert!(!attempt.active());
        assert!(attempt.transition(LoginPhase::Cancelled, None).is_none());
        assert!(attempt.save(|| panic!("duplicate save")).is_none());
    }

    #[test]
    fn expired_exchange_does_not_commit() {
        let mut attempt = LoginAttempt::new("expired".into());
        attempt.started = Instant::now() - LOGIN_TIMEOUT;
        let status = attempt
            .save(|| panic!("expired login must not save"))
            .unwrap();
        assert_eq!(status.phase, Some(LoginPhase::TimedOut));
        assert!(!attempt.active());
    }

    #[test]
    fn save_errors_are_bounded_and_do_not_expose_credentials_or_paths() {
        let attempt = LoginAttempt::new("one".into());
        let status = attempt
            .save(|| Err("SECRET_CANARY /private/account".into()))
            .unwrap();
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("SECRET_CANARY"));
        assert!(!serialized.contains("/private"));
        assert_eq!(status.reason.as_deref(), Some("storage"));
    }

    #[test]
    fn fallback_is_not_a_terminal_failure_and_windows_are_attempt_scoped() {
        let old = LoginAttempt::new("old".into());
        let new = LoginAttempt::new("new".into());
        old.transition(LoginPhase::BrowserFallback, None).unwrap();
        assert!(old.active());
        assert_ne!(old.window_label(), new.window_label());
        old.transition(LoginPhase::Cancelled, None).unwrap();
        assert!(new.active());
    }

    #[test]
    fn cancellation_after_commit_reports_the_committed_result() {
        let attempt = LoginAttempt::new("one".into());
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let saving_attempt = &attempt;
            let saving = scope.spawn(move || {
                saving_attempt.save(|| {
                    entered.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok("account".into())
                })
            });
            entered_rx.recv().unwrap();
            let cancelling = scope.spawn(|| attempt.transition(LoginPhase::Cancelled, None));
            release.send(()).unwrap();
            assert!(saving.join().unwrap().unwrap().ok);
            assert!(cancelling.join().unwrap().is_none());
        });
        assert_eq!(attempt.snapshot().phase, Some(LoginPhase::Succeeded));
    }
}
