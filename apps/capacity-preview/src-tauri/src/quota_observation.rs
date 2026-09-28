//! In-process, identity-checked host observations. Never accepts renderer input.
use super::*;

#[derive(Clone, Debug)]
pub struct DesktopQuotaObservationWindow {
    pub limit_id: String,
    pub window_minutes: Option<u64>,
    pub remaining_percent: f64,
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DesktopQuotaObservation {
    pub captured_at: String,
    pub plan_type: Option<String>,
    pub windows: Vec<DesktopQuotaObservationWindow>,
}

impl DesktopQuotaObservation {
    pub fn is_valid(&self) -> bool {
        DateTime::parse_from_rfc3339(&self.captured_at).is_ok()
            && !self.windows.is_empty()
            && self.windows.iter().all(|window| {
                canonical_codex_limit(&window.limit_id)
                    && window.remaining_percent.is_finite()
                    && (0.0..=100.0).contains(&window.remaining_percent)
                    && window.window_minutes != Some(0)
                    && window
                        .resets_at
                        .as_ref()
                        .is_none_or(|reset| DateTime::parse_from_rfc3339(reset).is_ok())
            })
    }
}

impl DesktopStatusEnvelope {
    /// Low-priority bootstrap only: never counted as a live history capture.
    pub fn from_cached_quota(observation: &DesktopQuotaObservation) -> Option<Self> {
        if !observation.is_valid() {
            return None;
        }
        let mut envelope =
            envelope_from_discovery(None, None, None).reconcile_managed_quota(Some(observation));
        envelope.lifecycle = DesktopLifecycle::Stale;
        Some(envelope)
    }

    pub fn set_host_sequence(&mut self, sequence: u64) {
        self.sequence = sequence;
    }

    /// Complete process-local snapshot for a newly opened webview. Preserve
    /// credits, reader and history scope, but never call an old capture live or
    /// allow this bootstrap response to overwrite a later sequenced event.
    pub fn into_cached_snapshot(mut self) -> Self {
        self.sequence = 0;
        if self.lifecycle == DesktopLifecycle::Ready {
            self.lifecycle = DesktopLifecycle::Stale;
        }
        if let Some(status) = self.status.as_mut() {
            status.data_status.freshness = Freshness::Stale;
            if status.quota_freshness == Some(Freshness::Live) {
                status.quota_freshness = Some(Freshness::Stale);
            }
        }
        self
    }

    /// Only canonical live reads may advance the managed account cache.
    pub fn live_quota_observation(&self) -> Option<DesktopQuotaObservation> {
        let status = self.status.as_ref()?;
        if status.data_status.freshness != Freshness::Live || status.quota_source.is_some() {
            return None;
        }
        let observation = DesktopQuotaObservation {
            captured_at: status.captured_at.clone(),
            plan_type: status
                .account
                .as_ref()
                .and_then(|account| account.plan_type.clone()),
            windows: status
                .quota_windows
                .iter()
                .filter(|window| canonical_codex_limit(&window.limit_id))
                .map(|window| DesktopQuotaObservationWindow {
                    limit_id: window.limit_id.clone(),
                    window_minutes: window.window_minutes,
                    remaining_percent: window.remaining_percent,
                    resets_at: window.resets_at.clone(),
                })
                .collect(),
        };
        observation.is_valid().then_some(observation)
    }

    /// Reconcile quota only. Do not promote older credit/usage data to live.
    pub fn reconcile_managed_quota(
        mut self,
        observation: Option<&DesktopQuotaObservation>,
    ) -> Self {
        let Some(observation) = observation.filter(|value| value.is_valid()) else {
            return self;
        };
        // The account-list reader may finish later with the same quota. That
        // does not invalidate an independent live app-server observation. Keep
        // its provenance/time; never promote the cache's timestamp to live.
        if self.status.as_ref().is_some_and(|status| {
            status.data_status.freshness == Freshness::Live
                && status.quota_source.is_none()
                && same_quota_windows(status, observation)
        }) {
            return self;
        }
        let incoming_time = DateTime::parse_from_rfc3339(&observation.captured_at).unwrap();
        let should_overlay = self.status.as_ref().is_none_or(|status| {
            !status
                .quota_windows
                .iter()
                .any(|window| canonical_codex_limit(&window.limit_id))
                || DateTime::parse_from_rfc3339(
                    status
                        .quota_observed_at
                        .as_ref()
                        .unwrap_or(&status.captured_at),
                )
                .map_or(true, |current| incoming_time > current)
        });
        if !should_overlay {
            return self;
        }
        let status = self.status.get_or_insert_with(|| DesktopStatusView {
            schema_version: DESKTOP_SCHEMA_VERSION.to_owned(),
            captured_at: observation.captured_at.clone(),
            codex_version: None,
            account: Some(DesktopAccountView {
                auth_mode: Some("chatgpt".to_owned()),
                plan_type: observation.plan_type.clone(),
                binding_status: AccountBindingStatus::Ephemeral,
            }),
            data_status: DesktopDataStatusView {
                availability: Availability::Partial,
                freshness: Freshness::Stale,
                compatibility: Compatibility::NotTested,
                reason_codes: vec!["managed_quota_only".to_owned()],
            },
            quota_windows: Vec::new(),
            quota_observed_at: None,
            quota_freshness: None,
            quota_source: None,
            reset_credits: DesktopResetCreditView {
                summary_status: SummaryStatus::Unavailable,
                available_count: None,
                details_status: ResetCreditDetailsStatus::Unavailable,
            },
            usage: DesktopUsageView {
                availability: Availability::Unsupported,
                has_summary: false,
                reason_codes: Vec::new(),
            },
            diagnostic_codes: Vec::new(),
        });
        status
            .quota_windows
            .retain(|window| !canonical_codex_limit(&window.limit_id));
        status
            .quota_windows
            .extend(
                observation
                    .windows
                    .iter()
                    .map(|window| DesktopQuotaWindowView {
                        limit_id: window.limit_id.clone(),
                        label: None,
                        window_minutes: window.window_minutes,
                        used_percent: 100.0 - window.remaining_percent,
                        remaining_percent: window.remaining_percent,
                        resets_at: window.resets_at.clone(),
                    }),
            );
        status.quota_observed_at = Some(observation.captured_at.clone());
        status.quota_freshness = Some(Freshness::Stale);
        status.quota_source = Some("managed_account_cache");
        self
    }
}

fn same_quota_windows(status: &DesktopStatusView, observation: &DesktopQuotaObservation) -> bool {
    let windows = status
        .quota_windows
        .iter()
        .filter(|window| canonical_codex_limit(&window.limit_id))
        .collect::<Vec<_>>();
    windows.len() == observation.windows.len()
        && windows.iter().all(|window| {
            observation.windows.iter().any(|other| {
                window.limit_id == other.limit_id
                    && window.window_minutes == other.window_minutes
                    && window.remaining_percent == other.remaining_percent
                    && match (&window.resets_at, &other.resets_at) {
                        (None, None) => true,
                        (Some(a), Some(b)) => DateTime::parse_from_rfc3339(a)
                            .ok()
                            .zip(DateTime::parse_from_rfc3339(b).ok())
                            .is_some_and(|(a, b)| a == b),
                        _ => false,
                    }
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(time: &str, remaining: f64) -> DesktopQuotaObservation {
        DesktopQuotaObservation {
            captured_at: time.to_owned(),
            plan_type: Some("pro".to_owned()),
            windows: vec![DesktopQuotaObservationWindow {
                limit_id: "codex:primary".to_owned(),
                window_minutes: Some(10080),
                remaining_percent: remaining,
                resets_at: Some("2026-09-10T00:00:00Z".to_owned()),
            }],
        }
    }

    #[test]
    fn full_bootstrap_retains_credits_reader_and_scope_without_claiming_live_history() {
        let mut envelope =
            DesktopStatusEnvelope::from_cached_quota(&observed("2026-09-26T00:00:00Z", 28.0))
                .unwrap();
        envelope.lifecycle = DesktopLifecycle::Ready;
        envelope.sequence = 9;
        envelope.history_context_id = Some("opaque-scope".into());
        envelope.selected_executable_id = Some("reader".into());
        let status = envelope.status.as_mut().unwrap();
        status.data_status.freshness = Freshness::Live;
        status.quota_freshness = Some(Freshness::Live);
        status.quota_source = None;
        status.reset_credits.summary_status = SummaryStatus::Available;
        status.reset_credits.available_count = Some(2);
        status.reset_credits.details_status = ResetCreditDetailsStatus::Complete;
        assert!(envelope.live_quota_observation().is_some());
        let cached = envelope.into_cached_snapshot();
        assert_eq!(cached.sequence, 0);
        assert_eq!(cached.lifecycle, DesktopLifecycle::Stale);
        assert_eq!(cached.history_context_id.as_deref(), Some("opaque-scope"));
        assert_eq!(cached.selected_executable_id.as_deref(), Some("reader"));
        let status = cached.status.as_ref().unwrap();
        assert_eq!(status.reset_credits.available_count, Some(2));
        assert_eq!(status.quota_windows[0].remaining_percent, 28.0);
        assert_eq!(status.captured_at, "2026-09-26T00:00:00Z");
        assert_eq!(status.quota_freshness, Some(Freshness::Stale));
        assert!(cached.live_quota_observation().is_none());
    }

    #[test]
    fn bootstrap_does_not_hide_reader_selection_or_error() {
        for lifecycle in [DesktopLifecycle::SelectionRequired, DesktopLifecycle::Error] {
            let mut envelope = envelope_from_discovery(None, None, None);
            envelope.lifecycle = lifecycle.clone();
            let cached = envelope.into_cached_snapshot();
            assert_eq!(cached.lifecycle, lifecycle);
            assert!(cached.status.is_none());
        }
    }

    #[test]
    fn identical_newer_cache_does_not_downgrade_a_live_read_or_advance_its_time() {
        let live = observed("2026-09-23T02:11:00Z", 10.0);
        let cache = observed("2026-09-23T02:11:05Z", 10.0);
        let mut envelope =
            envelope_from_discovery(None, None, None).reconcile_managed_quota(Some(&live));
        let status = envelope.status.as_mut().unwrap();
        status.data_status.freshness = Freshness::Live;
        status.quota_source = None;
        status.quota_freshness = None;
        status.quota_observed_at = None;
        let envelope = envelope.reconcile_managed_quota(Some(&cache));
        let status = envelope.status.as_ref().unwrap();
        assert!(status.quota_source.is_none());
        assert!(status.quota_observed_at.is_none());
        assert_eq!(status.captured_at, live.captured_at);
        assert!(envelope.live_quota_observation().is_some());
        // A real window reset with the same percentage is a different fact.
        let mut changed_window = cache;
        changed_window.windows[0].resets_at = Some("2026-09-30T02:11:00Z".into());
        assert!(
            envelope
                .reconcile_managed_quota(Some(&changed_window))
                .status
                .unwrap()
                .quota_source
                .is_some()
        );
    }

    #[test]
    fn newer_managed_refresh_replaces_older_live_quota_without_promoting_other_data() {
        let old = observed("2026-09-05T00:00:00Z", 29.0);
        let mut envelope =
            envelope_from_discovery(None, None, None).reconcile_managed_quota(Some(&old));
        let status = envelope.status.as_mut().unwrap();
        status.data_status.freshness = Freshness::Live;
        status.quota_source = None;
        status.quota_observed_at = None;
        let new = observed("2026-09-05T00:01:00Z", 28.0);
        let envelope = envelope.reconcile_managed_quota(Some(&new));
        let status = envelope.status.as_ref().unwrap();
        assert_eq!(status.quota_windows[0].remaining_percent, 28.0);
        assert_eq!(status.captured_at, old.captured_at);
        assert_eq!(
            status.quota_observed_at.as_deref(),
            Some(new.captured_at.as_str())
        );
        let input = select_work_plan_window(
            &envelope,
            None,
            &DateTime::parse_from_rfc3339("2026-09-05T00:02:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();
        assert_eq!(input.remaining_percent, 28.0);
        assert_eq!(input.source, "managed_account_cache");
        assert_eq!(input.captured_at.as_str(), new.captured_at);
    }

    #[test]
    fn delayed_cache_cannot_roll_back_a_newer_observation() {
        let newest = observed("2026-09-05T00:02:00Z", 28.0);
        let old = observed("2026-09-05T00:01:00Z", 29.0);
        let envelope = envelope_from_discovery(None, None, None)
            .reconcile_managed_quota(Some(&newest))
            .reconcile_managed_quota(Some(&old));
        assert_eq!(
            envelope.status.unwrap().quota_windows[0].remaining_percent,
            28.0
        );
    }

    #[test]
    fn invalid_or_model_specific_observations_are_rejected() {
        let mut observation = observed("2026-09-05T00:00:00Z", f64::NAN);
        assert!(!observation.is_valid());
        observation.windows[0].remaining_percent = 28.0;
        observation.windows[0].limit_id = "codex_bengalfox:primary".to_owned();
        assert!(!observation.is_valid());
    }

    #[test]
    fn startup_snapshot_is_stale_and_never_invents_credits_or_live_history() {
        let observation = observed("2026-09-05T00:00:00Z", 28.0);
        let envelope = DesktopStatusEnvelope::from_cached_quota(&observation).unwrap();
        assert_eq!(envelope.lifecycle, DesktopLifecycle::Stale);
        assert_eq!(envelope.sequence, 0);
        assert!(envelope.live_quota_observation().is_none());
        let status = envelope.status.unwrap();
        assert_eq!(status.quota_windows[0].remaining_percent, 28.0);
        assert_eq!(
            status.quota_observed_at.as_deref(),
            Some(observation.captured_at.as_str())
        );
        assert_eq!(status.quota_freshness, Some(Freshness::Stale));
        assert_eq!(status.reset_credits.available_count, None);
    }
}
