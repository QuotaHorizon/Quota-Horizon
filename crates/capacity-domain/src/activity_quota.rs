//! Observed quota changes inside explicit activity bounds, not attribution or
//! an active-hour consumption model. Scope is enforced by the store caller.
use crate::{Availability, Compatibility, Freshness, UtcTimestamp, active_time::ActiveTimeInput};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ACTIVITY_QUOTA_VERSION: &str = "activity-quota-comparison-v1";
/// A gap longer than two longest supported monitor intervals is not bridged.
/// This is a comparison guard, NOT a calibrated forecast threshold.
pub const MAX_ACTIVITY_QUOTA_GAP_SECONDS: i64 = 1800;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityQuotaSample {
    pub snapshot_id: String,
    pub captured_at: UtcTimestamp,
    pub limit_id: String,
    pub window_minutes: Option<u64>,
    #[serde(with = "crate::frozen_number::scalar")]
    pub remaining_percent: f64,
    pub resets_at: Option<UtcTimestamp>,
    pub availability: Availability,
    pub freshness: Freshness,
    pub compatibility: Compatibility,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityQuotaComparison {
    pub algorithm_version: &'static str,
    pub query_truncated: bool,
    pub windows: Vec<ActivityQuotaWindow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityQuotaWindow {
    pub limit_id: String,
    pub window_minutes: Option<u64>,
    pub reason_code: &'static str,
    pub sample_count: usize,
    pub first_snapshot_id: String,
    pub last_snapshot_id: String,
    pub observed_from: UtcTimestamp,
    pub observed_until: UtcTimestamp,
    pub first_remaining_percent: Option<f64>,
    pub last_remaining_percent: Option<f64>,
    /// Only a monotonic observed decrease within one verified window boundary.
    /// Includes other devices and idle gaps; never divide by active minutes.
    pub observed_decrease_percent: Option<f64>,
    pub coverage_percent: f64,
    pub leading_gap_seconds: u64,
    pub trailing_gap_seconds: u64,
    pub maximum_gap_seconds: u64,
    pub compatibility_unverified: bool,
}

fn epoch(time: &UtcTimestamp) -> i64 {
    DateTime::parse_from_rfc3339(time.as_str())
        .expect("validated UTC")
        .timestamp_millis()
}
fn valid_percent(value: f64) -> Option<f64> {
    value
        .is_finite()
        .then_some(value)
        .filter(|value| (0.0..=100.0).contains(value))
}
fn same_sample(a: &ActivityQuotaSample, b: &ActivityQuotaSample) -> bool {
    a.window_minutes == b.window_minutes
        && a.remaining_percent == b.remaining_percent
        && a.resets_at.as_ref().map(epoch) == b.resets_at.as_ref().map(epoch)
        && a.availability == b.availability
        && a.freshness == b.freshness
        && a.compatibility == b.compatibility
}

pub fn compare_activity_quota(
    input: &ActiveTimeInput,
    samples: &[ActivityQuotaSample],
    truncated: bool,
) -> ActivityQuotaComparison {
    let mut result = ActivityQuotaComparison {
        algorithm_version: ACTIVITY_QUOTA_VERSION,
        query_truncated: truncated,
        windows: vec![],
    };
    if input.validated(&input.ended_at).is_err() {
        return result;
    }
    let start = epoch(&input.started_at);
    let end = epoch(&input.ended_at);
    let mut groups: BTreeMap<&str, Vec<&ActivityQuotaSample>> = BTreeMap::new();
    for sample in samples {
        let t = epoch(&sample.captured_at);
        if (sample.limit_id == "codex" || sample.limit_id.starts_with("codex:"))
            && t >= start
            && t <= end
        {
            groups.entry(&sample.limit_id).or_default().push(sample);
        }
    }
    for (limit_id, mut rows) in groups {
        rows.sort_by(|a, b| {
            epoch(&a.captured_at)
                .cmp(&epoch(&b.captured_at))
                .then(a.snapshot_id.cmp(&b.snapshot_id))
        });
        let conflicting = rows.windows(2).any(|pair| {
            epoch(&pair[0].captured_at) == epoch(&pair[1].captured_at)
                && !same_sample(pair[0], pair[1])
        });
        rows.dedup_by(|a, b| epoch(&a.captured_at) == epoch(&b.captured_at));
        let first = rows[0];
        let last = rows[rows.len() - 1];
        let maximum_gap = rows
            .windows(2)
            .map(|pair| epoch(&pair[1].captured_at) - epoch(&pair[0].captured_at))
            .max()
            .unwrap_or(0);
        let reason = if truncated {
            "query_truncated"
        } else if conflicting {
            "conflicting_samples"
        } else if rows.iter().any(|s| {
            valid_percent(s.remaining_percent).is_none()
                || s.freshness != Freshness::Live
                || !matches!(
                    s.availability,
                    Availability::Complete | Availability::Partial
                )
                || matches!(
                    s.compatibility,
                    Compatibility::Unsupported
                        | Compatibility::KnownBroken
                        | Compatibility::NotApplicable
                )
        }) {
            "incompatible_source"
        } else if rows.len() < 2 {
            "insufficient_samples"
        } else if first.window_minutes.is_none_or(|m| m == 0)
            || rows
                .iter()
                .any(|s| s.window_minutes != first.window_minutes)
        {
            "window_changed"
        } else if rows.iter().any(|s| s.resets_at.is_none()) {
            "missing_reset_time"
        } else if rows
            .iter()
            .any(|s| s.resets_at.as_ref().map(epoch) != first.resets_at.as_ref().map(epoch))
        {
            "window_changed"
        } else if rows
            .iter()
            .any(|s| epoch(&s.captured_at) >= epoch(s.resets_at.as_ref().unwrap()))
        {
            "expired_window"
        } else if rows
            .windows(2)
            .any(|pair| pair[1].remaining_percent > pair[0].remaining_percent)
        {
            "balance_increased"
        } else if maximum_gap > MAX_ACTIVITY_QUOTA_GAP_SECONDS * 1000 {
            "observation_gap"
        } else {
            "comparable"
        };
        let window_minutes = first
            .window_minutes
            .filter(|m| *m > 0 && rows.iter().all(|s| s.window_minutes == Some(*m)));
        result.windows.push(ActivityQuotaWindow {
            limit_id: limit_id.to_owned(),
            window_minutes,
            reason_code: reason,
            sample_count: rows.len(),
            first_snapshot_id: first.snapshot_id.clone(),
            last_snapshot_id: last.snapshot_id.clone(),
            observed_from: first.captured_at.clone(),
            observed_until: last.captured_at.clone(),
            first_remaining_percent: valid_percent(first.remaining_percent),
            last_remaining_percent: valid_percent(last.remaining_percent),
            observed_decrease_percent: if reason == "comparable" {
                Some(((first.remaining_percent - last.remaining_percent) * 100.0).round() / 100.0)
            } else {
                None
            },
            coverage_percent: ((epoch(&last.captured_at) - epoch(&first.captured_at)) as f64
                / (end - start) as f64
                * 1000.0)
                .floor()
                / 10.0,
            leading_gap_seconds: ((epoch(&first.captured_at) - start + 999) / 1000) as u64,
            trailing_gap_seconds: ((end - epoch(&last.captured_at) + 999) / 1000) as u64,
            maximum_gap_seconds: ((maximum_gap + 999) / 1000) as u64,
            compatibility_unverified: rows
                .iter()
                .any(|s| s.compatibility != Compatibility::Tested),
        });
    }
    result.windows.sort_by(|a, b| {
        b.window_minutes
            .cmp(&a.window_minutes)
            .then(a.limit_id.cmp(&b.limit_id))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(t: &str) -> UtcTimestamp {
        UtcTimestamp::parse(format!("2026-09-26T{t}Z")).unwrap()
    }
    fn input() -> ActiveTimeInput {
        ActiveTimeInput {
            started_at: at("02:00:00"),
            ended_at: at("03:00:00"),
            duration_seconds: 1200,
        }
    }
    fn sample(t: &str, remaining: f64) -> ActivityQuotaSample {
        ActivityQuotaSample {
            snapshot_id: format!("sample-{t}"),
            captured_at: at(t),
            limit_id: "codex:weekly".into(),
            window_minutes: Some(10080),
            remaining_percent: remaining,
            resets_at: Some(at("08:00:00")),
            availability: Availability::Complete,
            freshness: Freshness::Live,
            compatibility: Compatibility::Tested,
        }
    }
    fn rows() -> Vec<ActivityQuotaSample> {
        vec![
            sample("02:10:00", 40.5),
            sample("02:25:00", 39.0),
            sample("02:40:00", 38.0),
            sample("02:50:00", 37.5),
        ]
    }
    #[test]
    fn bounded_observation_does_not_scale_to_active_minutes_or_fill_edges() {
        let mut rows = rows();
        rows.push(sample("01:59:59", 70.0));
        rows.push(sample("03:00:01", 1.0));
        let v = compare_activity_quota(&input(), &rows, false)
            .windows
            .remove(0);
        assert_eq!(v.observed_decrease_percent, Some(3.0));
        assert_eq!(v.sample_count, 4);
        assert_eq!(v.coverage_percent, 66.6);
        assert_eq!(v.leading_gap_seconds, 600);
        assert_eq!(v.trailing_gap_seconds, 600);
        assert_eq!(v.maximum_gap_seconds, 900);
        let altered = ActiveTimeInput {
            duration_seconds: 600,
            ..input()
        };
        assert_eq!(
            compare_activity_quota(&altered, &rows, false).windows[0].observed_decrease_percent,
            Some(3.0)
        );
    }
    #[test]
    fn internal_rise_cannot_be_hidden_by_a_net_decrease() {
        let mut rows = rows();
        rows[1].remaining_percent = 41.0;
        let v = compare_activity_quota(&input(), &rows, false)
            .windows
            .remove(0);
        assert_eq!(v.reason_code, "balance_increased");
        assert_eq!(v.observed_decrease_percent, None);
    }
    #[test]
    fn changed_reset_or_duration_is_not_a_consumption_sample_or_proof_of_global_reset() {
        for duration in [false, true] {
            let mut rows = rows();
            if duration {
                rows[2].window_minutes = Some(300);
            } else {
                rows[2].resets_at = Some(at("09:00:00"));
            }
            let v = compare_activity_quota(&input(), &rows, false)
                .windows
                .remove(0);
            assert_eq!(v.reason_code, "window_changed");
            assert!(v.observed_decrease_percent.is_none());
        }
        let mut rows = rows();
        rows[1].resets_at = Some(UtcTimestamp::parse("2026-09-26T08:00:00+00:00").unwrap());
        assert_eq!(
            compare_activity_quota(&input(), &rows, false).windows[0].reason_code,
            "comparable"
        );
    }
    #[test]
    fn missing_expired_unverified_failed_and_stale_inputs_are_distinct() {
        let mut rows = rows();
        rows[0].resets_at = None;
        assert_eq!(
            compare_activity_quota(&input(), &rows, false).windows[0].reason_code,
            "missing_reset_time"
        );
        let mut rows = self::rows();
        for row in &mut rows {
            row.resets_at = Some(at("02:00:00"));
        }
        assert_eq!(
            compare_activity_quota(&input(), &rows, false).windows[0].reason_code,
            "expired_window"
        );
        let mut rows = self::rows();
        rows[0].compatibility = Compatibility::NotTested;
        let v = compare_activity_quota(&input(), &rows, false)
            .windows
            .remove(0);
        assert_eq!(v.reason_code, "comparable");
        assert!(v.compatibility_unverified);
        rows[0].freshness = Freshness::Stale;
        assert_eq!(
            compare_activity_quota(&input(), &rows, false).windows[0].reason_code,
            "incompatible_source"
        );
        rows[0].freshness = Freshness::Live;
        rows[0].availability = Availability::Failed;
        assert_eq!(
            compare_activity_quota(&input(), &rows, false).windows[0].reason_code,
            "incompatible_source"
        );
    }
    #[test]
    fn gaps_truncation_and_duplicate_conflicts_abstain_instead_of_inventing_precision() {
        let long = vec![sample("02:01:00", 30.0), sample("02:59:00", 28.0)];
        assert_eq!(
            compare_activity_quota(&input(), &long, false).windows[0].reason_code,
            "observation_gap"
        );
        assert_eq!(
            compare_activity_quota(&input(), &rows(), true).windows[0].reason_code,
            "query_truncated"
        );
        let mut rows = rows();
        rows.push(rows[0].clone());
        assert_eq!(
            compare_activity_quota(&input(), &rows, false).windows[0].sample_count,
            4
        );
        rows.last_mut().unwrap().remaining_percent = 1.0;
        assert_eq!(
            compare_activity_quota(&input(), &rows, false).windows[0].reason_code,
            "conflicting_samples"
        );
        assert_eq!(
            compare_activity_quota(&input(), &rows[..1], false).windows[0].reason_code,
            "insufficient_samples"
        );
    }
    #[test]
    fn model_specific_windows_are_excluded_and_short_windows_remain_independent() {
        let mut rows = rows();
        let mut unrelated = sample("02:10:00", 100.0);
        unrelated.limit_id = "codex_model:weekly".into();
        rows.push(unrelated);
        let mut short = rows[0].clone();
        short.limit_id = "codex:short".into();
        short.window_minutes = Some(300);
        rows.push(short);
        let v = compare_activity_quota(&input(), &rows, false);
        assert_eq!(v.windows.len(), 2);
        assert_eq!(v.windows[0].window_minutes, Some(10080));
        assert_eq!(v.windows[1].reason_code, "insufficient_samples");
    }
}
