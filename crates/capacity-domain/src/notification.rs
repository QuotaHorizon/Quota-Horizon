use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, FixedOffset};

use crate::{
    Availability, Compatibility, Freshness, ResetCreditDetailsStatus, StatusSnapshot, UtcTimestamp,
};

const MINUTES_PER_DAY: u16 = 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    start_minute: u16,
    end_minute: u16,
}

impl QuietHours {
    pub fn new(start_minute: u16, end_minute: u16) -> Result<Self, NotificationPolicyError> {
        if start_minute >= MINUTES_PER_DAY
            || end_minute >= MINUTES_PER_DAY
            || start_minute == end_minute
        {
            return Err(NotificationPolicyError::InvalidQuietHours);
        }
        Ok(Self {
            start_minute,
            end_minute,
        })
    }

    pub fn contains(self, local_minute: u16) -> bool {
        if local_minute >= MINUTES_PER_DAY {
            return false;
        }
        if self.start_minute < self.end_minute {
            (self.start_minute..self.end_minute).contains(&local_minute)
        } else {
            local_minute >= self.start_minute || local_minute < self.end_minute
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationPolicy {
    quota_threshold_basis_points: Option<u16>,
    reset_credit_notice_hours: u16,
    quiet_hours: Option<QuietHours>,
    lock_screen_privacy: bool,
    minimum_delivery_interval_minutes: u16,
}

impl NotificationPolicy {
    pub fn new(
        quota_threshold_basis_points: Option<u16>,
        reset_credit_notice_hours: u16,
        quiet_hours: Option<QuietHours>,
        lock_screen_privacy: bool,
        minimum_delivery_interval_minutes: u16,
    ) -> Result<Self, NotificationPolicyError> {
        if quota_threshold_basis_points.is_some_and(|value| value > 10_000) {
            return Err(NotificationPolicyError::InvalidQuotaThreshold);
        }
        if !(1..=720).contains(&reset_credit_notice_hours) {
            return Err(NotificationPolicyError::InvalidResetCreditNoticeHours);
        }
        if minimum_delivery_interval_minutes == 0 {
            return Err(NotificationPolicyError::InvalidDeliveryInterval);
        }
        Ok(Self {
            quota_threshold_basis_points,
            reset_credit_notice_hours,
            quiet_hours,
            lock_screen_privacy,
            minimum_delivery_interval_minutes,
        })
    }

    fn is_quiet(self, local_minute: u16) -> bool {
        self.quiet_hours
            .is_some_and(|quiet_hours| quiet_hours.contains(local_minute))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationPolicyError {
    InvalidQuotaThreshold,
    InvalidResetCreditNoticeHours,
    InvalidQuietHours,
    InvalidDeliveryInterval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDelivery {
    Deliver,
    SuppressedQuietHours,
    SuppressedRateLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationPrivacy {
    Generic,
    Detailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaThresholdCrossing {
    pub limit_id: String,
    pub remaining_basis_points: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorNotificationKind {
    QuotaThresholdCrossed {
        windows: Vec<QuotaThresholdCrossing>,
    },
    ResetCreditsExpiring {
        credit_count: usize,
        earliest_expires_at: UtcTimestamp,
    },
    LiveRestored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorNotificationDecision {
    pub kind: MonitorNotificationKind,
    pub delivery: NotificationDelivery,
    pub privacy: NotificationPrivacy,
    pub observed_at: UtcTimestamp,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct WindowKey {
    limit_id: String,
    resets_at: String,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ExpiringCreditKey {
    opaque_id: String,
    expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NotificationCategory {
    QuotaThreshold,
    ResetCreditExpiry,
    LiveRestored,
}

#[derive(Default)]
pub struct NotificationEngine {
    previous_freshness: Option<Freshness>,
    previous_was_trusted_live: bool,
    previous_live_windows: BTreeMap<WindowKey, u16>,
    expiring_credits: BTreeSet<ExpiringCreditKey>,
    last_delivery_at: BTreeMap<NotificationCategory, DateTime<FixedOffset>>,
}

impl NotificationEngine {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn observe(
        &mut self,
        snapshot: &StatusSnapshot,
        policy: NotificationPolicy,
        evaluated_at: &UtcTimestamp,
        local_minute: u16,
    ) -> Vec<MonitorNotificationDecision> {
        let Ok(now) = DateTime::parse_from_rfc3339(evaluated_at.as_str()) else {
            return Vec::new();
        };
        let trusted_live = is_trusted_live(snapshot);
        let previous_freshness = self.previous_freshness;
        let mut events = Vec::new();

        if trusted_live
            && self.previous_was_trusted_live
            && let Some(threshold) = policy.quota_threshold_basis_points
        {
            let mut crossings = snapshot
                .quota
                .windows
                .iter()
                .filter_map(|window| {
                    let resets_at = window.resets_at.as_ref()?;
                    let key = WindowKey {
                        limit_id: window.limit_id.clone(),
                        resets_at: resets_at.as_str().to_owned(),
                    };
                    let current = percentage_basis_points(window.remaining_percent);
                    self.previous_live_windows
                        .get(&key)
                        .filter(|previous| **previous > threshold && current <= threshold)
                        .map(|_| QuotaThresholdCrossing {
                            limit_id: window.limit_id.clone(),
                            remaining_basis_points: current,
                        })
                })
                .collect::<Vec<_>>();
            crossings.sort_by(|left, right| left.limit_id.cmp(&right.limit_id));
            if !crossings.is_empty() {
                events.push((
                    NotificationCategory::QuotaThreshold,
                    MonitorNotificationKind::QuotaThresholdCrossed { windows: crossings },
                ));
            }
        }

        if trusted_live {
            if let Some(event) = self.observe_reset_credit_expiry(snapshot, policy, now) {
                events.push((NotificationCategory::ResetCreditExpiry, event));
            }
        } else {
            self.prune_expired_credits(now);
        }

        if trusted_live && previous_freshness == Some(Freshness::Stale) {
            events.push((
                NotificationCategory::LiveRestored,
                MonitorNotificationKind::LiveRestored,
            ));
        }

        if trusted_live {
            self.previous_live_windows = snapshot
                .quota
                .windows
                .iter()
                .filter_map(|window| {
                    window.resets_at.as_ref().map(|resets_at| {
                        (
                            WindowKey {
                                limit_id: window.limit_id.clone(),
                                resets_at: resets_at.as_str().to_owned(),
                            },
                            percentage_basis_points(window.remaining_percent),
                        )
                    })
                })
                .collect();
        }
        self.previous_freshness = Some(snapshot.data_status.freshness);
        self.previous_was_trusted_live = trusted_live;

        events
            .into_iter()
            .map(|(category, kind)| {
                let delivery = self.delivery_for(category, policy, now, local_minute);
                if delivery == NotificationDelivery::Deliver {
                    self.last_delivery_at.insert(category, now);
                }
                MonitorNotificationDecision {
                    kind,
                    delivery,
                    privacy: if policy.lock_screen_privacy {
                        NotificationPrivacy::Generic
                    } else {
                        NotificationPrivacy::Detailed
                    },
                    observed_at: evaluated_at.clone(),
                }
            })
            .collect()
    }

    fn observe_reset_credit_expiry(
        &mut self,
        snapshot: &StatusSnapshot,
        policy: NotificationPolicy,
        now: DateTime<FixedOffset>,
    ) -> Option<MonitorNotificationKind> {
        self.prune_expired_credits(now);
        let horizon = now + Duration::hours(i64::from(policy.reset_credit_notice_hours));
        let observed = snapshot
            .quota
            .reset_credit_summary
            .credits
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|credit| credit.status.eq_ignore_ascii_case("available"))
            .filter_map(|credit| {
                let expires_at = credit.expires_at.as_ref()?;
                let parsed = DateTime::parse_from_rfc3339(expires_at.as_str()).ok()?;
                (parsed > now && parsed <= horizon).then(|| {
                    (
                        ExpiringCreditKey {
                            opaque_id: credit.opaque_id.clone(),
                            expires_at: expires_at.as_str().to_owned(),
                        },
                        parsed,
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();

        let newly_expiring = observed
            .iter()
            .filter(|(key, _)| !self.expiring_credits.contains(*key))
            .map(|(key, expires_at)| (key.clone(), *expires_at))
            .collect::<Vec<_>>();

        match snapshot.quota.reset_credit_summary.details_status {
            ResetCreditDetailsStatus::Complete => {
                self.expiring_credits = observed.into_keys().collect();
            }
            ResetCreditDetailsStatus::Partial => {
                self.expiring_credits.extend(observed.into_keys());
            }
            ResetCreditDetailsStatus::Unavailable => {}
        }

        let earliest = newly_expiring
            .iter()
            .map(|(_, expires_at)| expires_at)
            .min()
            .copied()?;
        let earliest = UtcTimestamp::parse(earliest.to_rfc3339()).ok()?;
        Some(MonitorNotificationKind::ResetCreditsExpiring {
            credit_count: newly_expiring.len(),
            earliest_expires_at: earliest,
        })
    }

    fn prune_expired_credits(&mut self, now: DateTime<FixedOffset>) {
        self.expiring_credits.retain(|credit| {
            DateTime::parse_from_rfc3339(&credit.expires_at)
                .is_ok_and(|expires_at| expires_at > now)
        });
    }

    fn delivery_for(
        &self,
        category: NotificationCategory,
        policy: NotificationPolicy,
        now: DateTime<FixedOffset>,
        local_minute: u16,
    ) -> NotificationDelivery {
        if policy.is_quiet(local_minute) {
            return NotificationDelivery::SuppressedQuietHours;
        }
        let cooldown = Duration::minutes(i64::from(policy.minimum_delivery_interval_minutes));
        if self
            .last_delivery_at
            .get(&category)
            .is_some_and(|previous| now.signed_duration_since(*previous) < cooldown)
        {
            return NotificationDelivery::SuppressedRateLimit;
        }
        NotificationDelivery::Deliver
    }
}

fn is_trusted_live(snapshot: &StatusSnapshot) -> bool {
    snapshot.data_status.freshness == Freshness::Live
        && matches!(
            snapshot.data_status.availability,
            Availability::Complete | Availability::Partial
        )
        && matches!(
            snapshot.data_status.compatibility,
            Compatibility::Tested | Compatibility::ExpectedCompatible
        )
}

fn percentage_basis_points(value: f64) -> u16 {
    (value * 100.0).round().clamp(0.0, 10_000.0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ResetCredit, ResetCreditDetailsStatus};

    fn fixture_status(name: &str) -> StatusSnapshot {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/app-server")
                .join(name)
                .join("expected-status.json"),
        )
        .unwrap();
        let mut snapshot: StatusSnapshot = serde_json::from_slice(&bytes).unwrap();
        snapshot.data_status.compatibility = Compatibility::ExpectedCompatible;
        snapshot
    }

    fn timestamp(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).unwrap()
    }

    fn policy(
        threshold: Option<u16>,
        quiet_hours: Option<QuietHours>,
        private: bool,
    ) -> NotificationPolicy {
        NotificationPolicy::new(threshold, 48, quiet_hours, private, 15).unwrap()
    }

    #[test]
    fn quota_notification_requires_a_same_cycle_downward_crossing() {
        let mut engine = NotificationEngine::default();
        let mut first = fixture_status("plus-normal");
        first.quota.windows[0].remaining_percent = 25.0;
        first.quota.windows[0].used_percent = 75.0;
        let at = timestamp("2026-08-30T00:00:00Z");
        assert!(
            engine
                .observe(&first, policy(Some(2_000), None, true), &at, 720)
                .is_empty()
        );

        let mut crossed = first.clone();
        crossed.quota.windows[0].remaining_percent = 20.0;
        crossed.quota.windows[0].used_percent = 80.0;
        let decisions = engine.observe(
            &crossed,
            policy(Some(2_000), None, true),
            &timestamp("2026-08-30T00:05:00Z"),
            725,
        );
        assert_eq!(decisions.len(), 1);
        assert!(matches!(
            &decisions[0].kind,
            MonitorNotificationKind::QuotaThresholdCrossed { windows }
                if windows.len() == 1 && windows[0].remaining_basis_points == 2_000
        ));
        assert_eq!(decisions[0].privacy, NotificationPrivacy::Generic);

        assert!(
            engine
                .observe(
                    &crossed,
                    policy(Some(2_000), None, true),
                    &timestamp("2026-08-30T00:10:00Z"),
                    730,
                )
                .is_empty()
        );

        let mut new_cycle = first;
        new_cycle.quota.windows[0].remaining_percent = 19.0;
        new_cycle.quota.windows[0].used_percent = 81.0;
        new_cycle.quota.windows[0].resets_at = Some(timestamp("2026-09-02T00:00:00Z"));
        assert!(
            engine
                .observe(
                    &new_cycle,
                    policy(Some(2_000), None, true),
                    &timestamp("2026-08-30T00:20:00Z"),
                    740,
                )
                .is_empty()
        );
    }

    #[test]
    fn stale_recovery_is_distinct_and_does_not_create_a_threshold_crossing() {
        let mut engine = NotificationEngine::default();
        let mut live = fixture_status("plus-normal");
        live.quota.windows[0].remaining_percent = 25.0;
        live.quota.windows[0].used_percent = 75.0;
        engine.observe(
            &live,
            policy(Some(2_000), None, false),
            &timestamp("2026-08-30T00:00:00Z"),
            720,
        );

        let mut stale = live.clone();
        stale.data_status.freshness = Freshness::Stale;
        engine.observe(
            &stale,
            policy(Some(2_000), None, false),
            &timestamp("2026-08-30T00:05:00Z"),
            725,
        );

        let mut restored = live;
        restored.quota.windows[0].remaining_percent = 19.0;
        restored.quota.windows[0].used_percent = 81.0;
        let decisions = engine.observe(
            &restored,
            policy(Some(2_000), None, false),
            &timestamp("2026-08-30T00:10:00Z"),
            730,
        );
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].kind, MonitorNotificationKind::LiveRestored);
        assert_eq!(decisions[0].privacy, NotificationPrivacy::Detailed);
    }

    #[test]
    fn unverified_compatibility_breaks_threshold_comparison() {
        let mut engine = NotificationEngine::default();
        let mut snapshot = fixture_status("plus-normal");
        snapshot.quota.windows[0].remaining_percent = 25.0;
        snapshot.quota.windows[0].used_percent = 75.0;
        engine.observe(
            &snapshot,
            policy(Some(2_000), None, true),
            &timestamp("2026-08-30T00:00:00Z"),
            720,
        );

        snapshot.data_status.compatibility = Compatibility::NotTested;
        snapshot.quota.windows[0].remaining_percent = 19.0;
        snapshot.quota.windows[0].used_percent = 81.0;
        assert!(
            engine
                .observe(
                    &snapshot,
                    policy(Some(2_000), None, true),
                    &timestamp("2026-08-30T00:05:00Z"),
                    725,
                )
                .is_empty()
        );

        snapshot.data_status.compatibility = Compatibility::ExpectedCompatible;
        snapshot.quota.windows[0].remaining_percent = 18.0;
        snapshot.quota.windows[0].used_percent = 82.0;
        assert!(
            engine
                .observe(
                    &snapshot,
                    policy(Some(2_000), None, true),
                    &timestamp("2026-08-30T00:10:00Z"),
                    730,
                )
                .is_empty()
        );
    }

    #[test]
    fn missing_reset_boundary_abstains_from_threshold_crossing() {
        let mut engine = NotificationEngine::default();
        let mut snapshot = fixture_status("plus-normal");
        snapshot.quota.windows[0].resets_at = None;
        snapshot.quota.windows[0].remaining_percent = 25.0;
        snapshot.quota.windows[0].used_percent = 75.0;
        engine.observe(
            &snapshot,
            policy(Some(2_000), None, true),
            &timestamp("2026-08-30T00:00:00Z"),
            720,
        );

        snapshot.quota.windows[0].remaining_percent = 19.0;
        snapshot.quota.windows[0].used_percent = 81.0;
        assert!(
            engine
                .observe(
                    &snapshot,
                    policy(Some(2_000), None, true),
                    &timestamp("2026-08-30T00:05:00Z"),
                    725,
                )
                .is_empty()
        );
    }

    #[test]
    fn reset_credit_expiry_is_emitted_once_when_it_enters_the_horizon() {
        let mut engine = NotificationEngine::default();
        let mut snapshot = fixture_status("plus-normal");
        snapshot.quota.reset_credit_summary.credits = Some(vec![ResetCredit {
            opaque_id: "credit-one".to_owned(),
            reset_type: "codexRateLimits".to_owned(),
            status: "available".to_owned(),
            granted_at: None,
            expires_at: Some(timestamp("2026-08-31T12:00:00Z")),
            title: None,
            description: None,
        }]);
        snapshot.quota.reset_credit_summary.available_count = Some(1);
        snapshot.quota.reset_credit_summary.details_status = ResetCreditDetailsStatus::Complete;

        let decisions = engine.observe(
            &snapshot,
            policy(None, None, true),
            &timestamp("2026-08-30T00:00:00Z"),
            720,
        );
        assert!(matches!(
            &decisions[0].kind,
            MonitorNotificationKind::ResetCreditsExpiring {
                credit_count: 1,
                ..
            }
        ));
        assert!(
            engine
                .observe(
                    &snapshot,
                    policy(None, None, true),
                    &timestamp("2026-08-30T00:05:00Z"),
                    725,
                )
                .is_empty()
        );
    }

    #[test]
    fn quiet_hours_wrap_midnight_and_consume_the_transition_without_catch_up() {
        let quiet = QuietHours::new(22 * 60, 7 * 60).unwrap();
        assert!(quiet.contains(23 * 60));
        assert!(quiet.contains(6 * 60 + 59));
        assert!(!quiet.contains(12 * 60));

        let mut engine = NotificationEngine::default();
        let mut first = fixture_status("plus-normal");
        first.quota.windows[0].remaining_percent = 21.0;
        first.quota.windows[0].used_percent = 79.0;
        engine.observe(
            &first,
            policy(Some(2_000), Some(quiet), true),
            &timestamp("2026-08-30T21:55:00Z"),
            21 * 60 + 55,
        );

        let mut crossed = first;
        crossed.quota.windows[0].remaining_percent = 19.0;
        crossed.quota.windows[0].used_percent = 81.0;
        let decisions = engine.observe(
            &crossed,
            policy(Some(2_000), Some(quiet), true),
            &timestamp("2026-08-30T22:05:00Z"),
            22 * 60 + 5,
        );
        assert_eq!(
            decisions[0].delivery,
            NotificationDelivery::SuppressedQuietHours
        );
        assert!(
            engine
                .observe(
                    &crossed,
                    policy(Some(2_000), Some(quiet), true),
                    &timestamp("2026-08-31T07:05:00Z"),
                    7 * 60 + 5,
                )
                .is_empty()
        );
    }

    #[test]
    fn same_category_is_rate_limited_across_distinct_transitions() {
        let mut engine = NotificationEngine::default();
        let mut snapshot = fixture_status("plus-normal");
        snapshot.quota.reset_credit_summary.credits = Some(vec![ResetCredit {
            opaque_id: "credit-one".to_owned(),
            reset_type: "codexRateLimits".to_owned(),
            status: "available".to_owned(),
            granted_at: None,
            expires_at: Some(timestamp("2026-08-31T00:00:00Z")),
            title: None,
            description: None,
        }]);
        snapshot.quota.reset_credit_summary.available_count = Some(1);
        snapshot.quota.reset_credit_summary.details_status = ResetCreditDetailsStatus::Complete;
        let first = engine.observe(
            &snapshot,
            policy(None, None, false),
            &timestamp("2026-08-30T00:00:00Z"),
            720,
        );
        assert_eq!(first[0].delivery, NotificationDelivery::Deliver);

        snapshot.quota.reset_credit_summary.credits = Some(vec![ResetCredit {
            opaque_id: "credit-two".to_owned(),
            reset_type: "codexRateLimits".to_owned(),
            status: "available".to_owned(),
            granted_at: None,
            expires_at: Some(timestamp("2026-08-31T00:10:00Z")),
            title: None,
            description: None,
        }]);
        let second = engine.observe(
            &snapshot,
            policy(None, None, false),
            &timestamp("2026-08-30T00:10:00Z"),
            730,
        );
        assert_eq!(
            second[0].delivery,
            NotificationDelivery::SuppressedRateLimit
        );
    }

    #[test]
    fn policy_rejects_invalid_boundaries() {
        assert_eq!(
            QuietHours::new(1_440, 60),
            Err(NotificationPolicyError::InvalidQuietHours)
        );
        assert_eq!(
            NotificationPolicy::new(Some(10_001), 48, None, true, 15),
            Err(NotificationPolicyError::InvalidQuotaThreshold)
        );
        assert_eq!(
            NotificationPolicy::new(None, 0, None, true, 15),
            Err(NotificationPolicyError::InvalidResetCreditNoticeHours)
        );
    }
}
