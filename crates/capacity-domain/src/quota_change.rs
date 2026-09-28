//! Replayable observations of quota rises, NOT proof of a global reset. Callers
//! supply one stable account's ordered, live-recorded history. No public signals,
//! quota credits, account identities or inferred timestamps enter this model.

use crate::UtcTimestamp;
use chrono::DateTime;
use serde::Serialize;

pub const QUOTA_CHANGE_VERSION: &str = "observed-quota-rise-v1";
const MIN_RISE_BASIS_POINTS: i64 = 500;
const MAX_GAP_MS: i64 = 3_600_000;
const MIN_FOLLOWUP_MS: i64 = 30_000;
const MIN_REANCHOR_MS: i64 = 60_000;
const MAX_OBSERVATIONS: usize = 32;

#[derive(Debug, Clone)]
pub struct QuotaChangeSample {
    pub snapshot_id: String,
    pub captured_at: UtcTimestamp,
    pub limit_id: String,
    pub window_minutes: Option<u64>,
    pub remaining_percent: f64,
    pub resets_at: Option<UtcTimestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaRiseClassification {
    AwaitingFollowup,
    AroundScheduledBoundary,
    BeforeScheduledBoundary,
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaRiseLimitation {
    AwaitingFollowup,
    ObservationGap,
    WindowUnavailable,
    WindowNotReanchored,
    WindowChangedAgain,
    NotSustained,
    AmbiguousTiming,
    SourceChanged,
    InvalidSample,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaRiseObservation {
    pub before_snapshot_id: String,
    pub first_after_snapshot_id: String,
    pub confirmation_snapshot_id: Option<String>,
    pub before_at: UtcTimestamp,
    pub first_after_at: UtcTimestamp,
    pub confirmed_at: Option<UtcTimestamp>,
    pub before_remaining_percent: f64,
    pub after_remaining_percent: f64,
    pub confirmed_remaining_percent: Option<f64>,
    pub before_resets_at: Option<UtcTimestamp>,
    pub after_resets_at: Option<UtcTimestamp>,
    pub classification: QuotaRiseClassification,
    pub limitation: Option<QuotaRiseLimitation>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaChangeSummary {
    pub algorithm_version: &'static str,
    pub valid_samples: u64,
    pub comparable_intervals: u64,
    pub excluded_intervals: u64,
    pub date_only_changes: u64,
    pub total_rises: u64,
    pub observations: Vec<QuotaRiseObservation>,
}

impl Default for QuotaChangeSummary {
    fn default() -> Self {
        Self {
            algorithm_version: QUOTA_CHANGE_VERSION,
            valid_samples: 0,
            comparable_intervals: 0,
            excluded_intervals: 0,
            date_only_changes: 0,
            total_rises: 0,
            observations: Vec::new(),
        }
    }
}

#[derive(Default)]
pub struct QuotaChangeObserver {
    previous: Option<QuotaChangeSample>,
    summary: QuotaChangeSummary,
}

fn epoch(time: &UtcTimestamp) -> i64 {
    DateTime::parse_from_rfc3339(time.as_str())
        .expect("validated UTC timestamp")
        .timestamp_millis()
}

fn basis_points(percent: f64) -> i64 {
    (percent * 100.0).round() as i64
}

fn same_window(left: &QuotaChangeSample, right: &QuotaChangeSample) -> bool {
    left.limit_id == right.limit_id && left.window_minutes == right.window_minutes
}

impl QuotaChangeObserver {
    pub fn observe(&mut self, sample: QuotaChangeSample) {
        if !sample.remaining_percent.is_finite()
            || !(0.0..=100.0).contains(&sample.remaining_percent)
            || sample.snapshot_id.is_empty()
            || sample.snapshot_id.len() > 256
            || sample.limit_id.is_empty()
            || sample.limit_id.len() > 128
        {
            self.finish_pending(QuotaRiseLimitation::InvalidSample);
            self.previous = None;
            self.summary.excluded_intervals += 1;
            return;
        }
        let Some(previous) = self.previous.take() else {
            self.summary.valid_samples += 1;
            self.previous = Some(sample);
            return;
        };
        let elapsed = epoch(&sample.captured_at) - epoch(&previous.captured_at);
        if elapsed <= 0 || sample.snapshot_id == previous.snapshot_id {
            // A duplicated capture is not the second successful observation.
            if sample.snapshot_id == previous.snapshot_id
                || (elapsed == 0
                    && sample.remaining_percent == previous.remaining_percent
                    && sample.resets_at.as_ref().map(epoch)
                        == previous.resets_at.as_ref().map(epoch)
                    && same_window(&sample, &previous))
            {
                self.previous = Some(previous);
            } else {
                self.finish_pending(QuotaRiseLimitation::AmbiguousTiming);
                self.summary.excluded_intervals += 1;
            }
            return;
        }
        self.summary.valid_samples += 1;
        if !same_window(&previous, &sample) {
            self.finish_pending(QuotaRiseLimitation::SourceChanged);
            self.summary.excluded_intervals += 1;
            self.previous = Some(sample);
            return;
        }
        if elapsed > MAX_GAP_MS {
            self.summary.excluded_intervals += 1;
            self.finish_pending(QuotaRiseLimitation::ObservationGap);
        } else {
            self.summary.comparable_intervals += 1;
            self.check_followup(&sample);
        }

        let rise =
            basis_points(sample.remaining_percent) - basis_points(previous.remaining_percent);
        let before_reset = previous.resets_at.as_ref().map(epoch);
        let after_reset = sample.resets_at.as_ref().map(epoch);
        if before_reset.is_some()
            && after_reset.is_some()
            && before_reset != after_reset
            && rise < MIN_RISE_BASIS_POINTS
        {
            self.summary.date_only_changes += 1;
        }
        if rise >= MIN_RISE_BASIS_POINTS {
            // A new rise cannot silently replace an unresolved earlier rise.
            self.finish_pending(QuotaRiseLimitation::WindowChangedAgain);
            let limitation = if elapsed > MAX_GAP_MS {
                QuotaRiseLimitation::ObservationGap
            } else if previous.window_minutes.is_none_or(|value| value == 0)
                || before_reset.is_none()
                || after_reset.is_none()
            {
                QuotaRiseLimitation::WindowUnavailable
            } else if after_reset.unwrap() - before_reset.unwrap() < MIN_REANCHOR_MS {
                QuotaRiseLimitation::WindowNotReanchored
            } else if before_reset.unwrap() < epoch(&previous.captured_at)
                || after_reset.unwrap() <= epoch(&sample.captured_at)
            {
                QuotaRiseLimitation::AmbiguousTiming
            } else {
                QuotaRiseLimitation::AwaitingFollowup
            };
            self.summary.total_rises += 1;
            if self.summary.observations.len() == MAX_OBSERVATIONS {
                self.summary.observations.remove(0);
            }
            self.summary.observations.push(QuotaRiseObservation {
                before_snapshot_id: previous.snapshot_id.clone(),
                first_after_snapshot_id: sample.snapshot_id.clone(),
                confirmation_snapshot_id: None,
                before_at: previous.captured_at.clone(),
                first_after_at: sample.captured_at.clone(),
                confirmed_at: None,
                before_remaining_percent: previous.remaining_percent,
                after_remaining_percent: sample.remaining_percent,
                confirmed_remaining_percent: None,
                before_resets_at: previous.resets_at.clone(),
                after_resets_at: sample.resets_at.clone(),
                classification: if limitation == QuotaRiseLimitation::AwaitingFollowup {
                    QuotaRiseClassification::AwaitingFollowup
                } else {
                    QuotaRiseClassification::Unverified
                },
                limitation: Some(limitation),
            });
        }
        self.previous = Some(sample);
    }

    fn finish_pending(&mut self, reason: QuotaRiseLimitation) {
        if let Some(pending) = self
            .summary
            .observations
            .last_mut()
            .filter(|item| item.classification == QuotaRiseClassification::AwaitingFollowup)
        {
            pending.classification = QuotaRiseClassification::Unverified;
            pending.limitation = Some(reason);
        }
    }

    fn check_followup(&mut self, sample: &QuotaChangeSample) {
        let Some(pending) = self
            .summary
            .observations
            .last_mut()
            .filter(|item| item.classification == QuotaRiseClassification::AwaitingFollowup)
        else {
            return;
        };
        let elapsed = epoch(&sample.captured_at) - epoch(&pending.first_after_at);
        let failure = if elapsed > MAX_GAP_MS {
            Some(QuotaRiseLimitation::ObservationGap)
        } else if sample.resets_at.as_ref().map(epoch)
            != pending.after_resets_at.as_ref().map(epoch)
        {
            Some(QuotaRiseLimitation::WindowChangedAgain)
        } else if basis_points(sample.remaining_percent)
            - basis_points(pending.before_remaining_percent)
            < MIN_RISE_BASIS_POINTS
        {
            Some(QuotaRiseLimitation::NotSustained)
        } else {
            None
        };
        if let Some(reason) = failure {
            pending.classification = QuotaRiseClassification::Unverified;
            pending.limitation = Some(reason);
        } else if elapsed >= MIN_FOLLOWUP_MS
            && sample.snapshot_id != pending.first_after_snapshot_id
        {
            let prior_reset = pending
                .before_resets_at
                .as_ref()
                .map(epoch)
                .expect("pending has a known boundary");
            pending.classification = if epoch(&pending.first_after_at) >= prior_reset {
                QuotaRiseClassification::AroundScheduledBoundary
            } else {
                QuotaRiseClassification::BeforeScheduledBoundary
            };
            pending.limitation = None;
            pending.confirmed_at = Some(sample.captured_at.clone());
            pending.confirmation_snapshot_id = Some(sample.snapshot_id.clone());
            pending.confirmed_remaining_percent = Some(sample.remaining_percent);
        }
    }

    pub fn finish(self) -> QuotaChangeSummary {
        self.summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OLD: &str = "2026-09-12T12:00:00Z";
    const NEW: &str = "2026-09-19T12:00:00Z";

    fn sample(time: &str, remaining: f64, reset: Option<&str>) -> QuotaChangeSample {
        QuotaChangeSample {
            snapshot_id: format!("sample-{time}"),
            captured_at: UtcTimestamp::parse(time).unwrap(),
            limit_id: "codex:weekly".into(),
            window_minutes: Some(10080),
            remaining_percent: remaining,
            resets_at: reset.map(|value| UtcTimestamp::parse(value).unwrap()),
        }
    }
    fn before() -> QuotaChangeSample {
        sample("2026-09-12T10:00:00Z", 18.4, Some(OLD))
    }
    fn first() -> QuotaChangeSample {
        sample("2026-09-12T10:05:00Z", 99.2, Some(NEW))
    }
    fn followup() -> QuotaChangeSample {
        sample("2026-09-12T10:06:00Z", 98.7, Some(NEW))
    }
    fn evaluate(samples: Vec<QuotaChangeSample>) -> QuotaChangeSummary {
        let mut observer = QuotaChangeObserver::default();
        for item in samples {
            observer.observe(item);
        }
        observer.finish()
    }

    #[test]
    fn early_rise_needs_two_distinct_successes_and_remains_cause_unknown() {
        let pending = evaluate(vec![before(), first()]);
        assert_eq!(
            pending.observations[0].classification,
            QuotaRiseClassification::AwaitingFollowup
        );
        let result = evaluate(vec![before(), first(), followup()]);
        let item = &result.observations[0];
        assert_eq!(
            item.classification,
            QuotaRiseClassification::BeforeScheduledBoundary
        );
        assert_eq!(item.before_remaining_percent, 18.4);
        assert_eq!(item.after_remaining_percent, 99.2);
        assert_eq!(item.confirmed_remaining_percent, Some(98.7));
        assert!(item.limitation.is_none());
        let json = serde_json::to_string(item).unwrap();
        assert!(!json.contains("global"));
        assert!(!json.contains("official"));
    }

    #[test]
    fn scheduled_rollover_is_separate_from_early_rise() {
        let result = evaluate(vec![
            sample("2026-09-12T11:59:00Z", 12.0, Some(OLD)),
            sample("2026-09-12T12:01:00Z", 100.0, Some(NEW)),
            sample("2026-09-12T12:02:00Z", 99.0, Some(NEW)),
        ]);
        assert_eq!(
            result.observations[0].classification,
            QuotaRiseClassification::AroundScheduledBoundary
        );
    }

    #[test]
    fn date_drift_without_a_material_rise_does_not_create_a_reset() {
        let result = evaluate(vec![
            before(),
            sample("2026-09-12T10:05:00Z", 18.5, Some(NEW)),
        ]);
        assert!(result.observations.is_empty());
        assert_eq!(result.date_only_changes, 1);
    }

    #[test]
    fn duplicate_and_sub_thirty_second_captures_cannot_confirm() {
        let duplicate = first();
        let too_soon = sample("2026-09-12T10:05:20Z", 99.0, Some(NEW));
        let result = evaluate(vec![before(), first(), duplicate, too_soon]);
        assert_eq!(
            result.observations[0].classification,
            QuotaRiseClassification::AwaitingFollowup
        );
        assert_eq!(result.valid_samples, 3);
        let thirty = sample("2026-09-12T10:05:30Z", 99.0, Some(NEW));
        assert_eq!(
            evaluate(vec![before(), first(), thirty]).observations[0].classification,
            QuotaRiseClassification::BeforeScheduledBoundary
        );
    }

    #[test]
    fn gaps_preserve_the_rise_but_do_not_verify_it() {
        let late = sample("2026-09-12T11:05:01Z", 99.0, Some(NEW));
        for samples in [vec![before(), late.clone()], vec![before(), first(), late]] {
            let result = evaluate(samples);
            assert_eq!(
                result.observations[0].classification,
                QuotaRiseClassification::Unverified
            );
            assert_eq!(
                result.observations[0].limitation,
                Some(QuotaRiseLimitation::ObservationGap)
            );
        }
    }

    #[test]
    fn rollback_or_second_boundary_change_invalidates_followup() {
        for (sample, expected) in [
            (
                sample("2026-09-12T10:06:00Z", 18.0, Some(NEW)),
                QuotaRiseLimitation::NotSustained,
            ),
            (
                sample("2026-09-12T10:06:00Z", 99.0, Some("2026-09-20T12:00:00Z")),
                QuotaRiseLimitation::WindowChangedAgain,
            ),
        ] {
            assert_eq!(
                evaluate(vec![before(), first(), sample]).observations[0].limitation,
                Some(expected)
            );
        }
    }

    #[test]
    fn absent_or_unchanged_boundaries_never_prove_reanchoring() {
        for (boundary, limitation) in [
            (None, QuotaRiseLimitation::WindowUnavailable),
            (Some(OLD), QuotaRiseLimitation::WindowNotReanchored),
        ] {
            let result = evaluate(vec![
                before(),
                sample("2026-09-12T10:05:00Z", 99.0, boundary),
                sample("2026-09-12T10:06:00Z", 98.0, boundary),
            ]);
            assert_eq!(result.observations[0].limitation, Some(limitation));
        }
    }

    #[test]
    fn bucket_or_duration_changes_break_continuity() {
        for change_duration in [false, true] {
            let mut other = followup();
            if change_duration {
                other.window_minutes = Some(300);
            } else {
                other.limit_id = "spark:weekly".into();
            }
            let result = evaluate(vec![before(), first(), other]);
            assert_eq!(
                result.observations[0].limitation,
                Some(QuotaRiseLimitation::SourceChanged)
            );
        }
    }

    #[test]
    fn invalid_or_out_of_order_samples_cannot_be_bridged() {
        let mut invalid = followup();
        invalid.remaining_percent = f64::NAN;
        let result = evaluate(vec![
            before(),
            first(),
            invalid,
            sample("2026-09-12T10:07:00Z", 99.0, Some(NEW)),
        ]);
        assert_eq!(
            result.observations[0].limitation,
            Some(QuotaRiseLimitation::InvalidSample)
        );
        let result = evaluate(vec![
            before(),
            first(),
            sample("2026-09-12T10:04:00Z", 95.0, Some(NEW)),
        ]);
        assert_eq!(
            result.observations[0].limitation,
            Some(QuotaRiseLimitation::AmbiguousTiming)
        );
    }

    #[test]
    fn threshold_uses_basis_points_and_output_is_bounded() {
        let before = sample("2026-09-12T10:00:00Z", 48.2, Some(OLD));
        let first = sample("2026-09-12T10:05:00Z", 53.2, Some(NEW));
        assert_eq!(evaluate(vec![before, first]).total_rises, 1);
        let mut observer = QuotaChangeObserver::default();
        for minute in 0..100 {
            let time = format!("2026-09-12T{:02}:{:02}:00Z", minute / 60, minute % 60);
            observer.observe(sample(
                &time,
                if minute % 2 == 0 { 10.0 } else { 80.0 },
                Some(NEW),
            ));
        }
        let result = observer.finish();
        assert_eq!(result.total_rises, 50);
        assert_eq!(result.observations.len(), 32);
    }

    #[test]
    fn equivalent_utc_forms_are_not_a_reanchor_or_a_changed_followup() {
        let equivalent = "2026-09-19T12:00:00.000+00:00";
        let result = evaluate(vec![
            before(),
            first(),
            sample("2026-09-12T10:06:00Z", 98.0, Some(equivalent)),
        ]);
        assert_eq!(
            result.observations[0].classification,
            QuotaRiseClassification::BeforeScheduledBoundary
        );
        assert_eq!(result.date_only_changes, 0);
    }

    #[test]
    fn stale_previous_boundary_is_not_mislabeled_normal_rollover() {
        let result = evaluate(vec![
            sample("2026-09-12T12:01:00Z", 18.0, Some(OLD)),
            sample("2026-09-12T12:05:00Z", 99.0, Some(NEW)),
            sample("2026-09-12T12:06:00Z", 98.0, Some(NEW)),
        ]);
        assert_eq!(
            result.observations[0].limitation,
            Some(QuotaRiseLimitation::AmbiguousTiming)
        );
    }
}
