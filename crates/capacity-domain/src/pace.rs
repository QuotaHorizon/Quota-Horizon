//! Experimental short-horizon quota-balance scenarios from three recent consumption blocks.
//! Scenario bounds use the observed slowest and fastest rates.
use crate::{
    Availability, Compatibility, Freshness, UtcTimestamp, activity_quota::ActivityQuotaSample,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

pub const PACE_ALGORITHM_VERSION: &str = "recent-block-pace-experimental-v1";
pub const PACE_HISTORY_HOURS: i64 = 72;
pub const MAX_SOURCE_AGE_SECONDS: i64 = 1800;

pub fn normalize_plan_type(value: Option<&str>) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some(match value.as_str() {
        "chatgptpro" => "pro".into(),
        "chatgptprolite" => "prolite".into(),
        _ => value,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaceSample {
    pub quota: ActivityQuotaSample,
    pub account_plan_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaceCurrent {
    pub limit_id: String,
    pub window_minutes: Option<u64>,
    #[serde(with = "crate::frozen_number::scalar")]
    pub remaining_percent: f64,
    pub resets_at: Option<UtcTimestamp>,
    pub observed_at: UtcTimestamp,
    pub account_plan_type: Option<String>,
    pub availability: Availability,
    pub freshness: Freshness,
    pub compatibility: Compatibility,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceRange {
    pub status: &'static str,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    pub reason_code: &'static str,
}
impl PaceRange {
    fn unavailable(reason: &'static str) -> Self {
        Self {
            status: "unavailable",
            lower: None,
            upper: None,
            reason_code: reason,
        }
    }
    fn available(lower: f64, upper: f64) -> Self {
        Self {
            status: "available",
            lower: Some(lower),
            upper: Some(upper),
            reason_code: "pace_scenario",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceTimeRange {
    pub status: &'static str,
    pub earliest_at: Option<UtcTimestamp>,
    pub latest_at: Option<UtcTimestamp>,
    pub reason_code: &'static str,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceCoverage {
    pub sample_count: usize,
    pub covered_seconds: u64,
    pub required_seconds: u64,
    pub maximum_gap_seconds: u64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceEstimate {
    pub algorithm_version: &'static str,
    pub state: &'static str,
    pub reason_code: &'static str,
    pub limit_id: String,
    pub window_minutes: Option<u64>,
    pub generated_at: UtcTimestamp,
    pub observed_at: Option<UtcTimestamp>,
    pub forecast_horizon: Option<UtcTimestamp>,
    pub current_remaining_percent: Option<f64>,
    pub input_snapshot_ids: Vec<String>,
    pub history_coverage: PaceCoverage,
    pub observed_block_rates: Vec<f64>,
    pub rate_range: PaceRange,
    pub balance_at_horizon: PaceRange,
    pub estimated_codex_active_hours_range: PaceRange,
    pub depletion_time_range: PaceTimeRange,
    pub compatibility_unverified: bool,
    pub cached_source: bool,
    pub data_stability: &'static str,
    pub assumptions: Vec<&'static str>,
    pub invalidation_factors: Vec<&'static str>,
    pub decision: &'static str,
    pub backtest_status: &'static str,
}
fn ms(t: &UtcTimestamp) -> i64 {
    DateTime::parse_from_rfc3339(t.as_str())
        .expect("validated UTC")
        .timestamp_millis()
}
fn at(t: i64) -> UtcTimestamp {
    UtcTimestamp::parse(
        DateTime::<Utc>::from_timestamp_millis(t)
            .expect("bounded timestamp")
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
    .expect("UTC timestamp")
}
fn percent(v: f64) -> Option<f64> {
    v.is_finite()
        .then_some(v)
        .filter(|v| (0.0..=100.0).contains(v))
}
fn compatible(c: Compatibility) -> bool {
    matches!(
        c,
        Compatibility::Tested | Compatibility::ExpectedCompatible | Compatibility::NotTested
    )
}
fn same(a: &PaceSample, b: &PaceSample) -> bool {
    a.account_plan_type == b.account_plan_type
        && a.quota.window_minutes == b.quota.window_minutes
        && a.quota.resets_at.as_ref().map(ms) == b.quota.resets_at.as_ref().map(ms)
        && a.quota.remaining_percent == b.quota.remaining_percent
        && a.quota.availability == b.quota.availability
        && a.quota.freshness == b.quota.freshness
        && a.quota.compatibility == b.quota.compatibility
}

pub fn estimate_pace(
    samples: &[PaceSample],
    current: &PaceCurrent,
    now: &UtcTimestamp,
    truncated: bool,
) -> PaceEstimate {
    let mut out = PaceEstimate {
        algorithm_version: PACE_ALGORITHM_VERSION,
        state: "abstained",
        reason_code: "insufficient_history",
        limit_id: current.limit_id.clone(),
        window_minutes: current.window_minutes,
        generated_at: now.clone(),
        observed_at: None,
        forecast_horizon: None,
        current_remaining_percent: percent(current.remaining_percent),
        input_snapshot_ids: vec![],
        history_coverage: PaceCoverage {
            sample_count: 0,
            covered_seconds: 0,
            required_seconds: 0,
            maximum_gap_seconds: 0,
        },
        observed_block_rates: vec![],
        rate_range: PaceRange::unavailable("insufficient_history"),
        balance_at_horizon: PaceRange::unavailable("insufficient_history"),
        estimated_codex_active_hours_range: PaceRange::unavailable(
            "missing_calibrated_active_time",
        ),
        depletion_time_range: PaceTimeRange {
            status: "unavailable",
            earliest_at: None,
            latest_at: None,
            reason_code: "insufficient_history",
        },
        compatibility_unverified: false,
        cached_source: current.freshness == Freshness::Stale,
        data_stability: "not_assessed",
        assumptions: vec![
            "recent_block_rate_continues",
            "elapsed_not_active_time",
            "no_added_resets_or_credits",
        ],
        invalidation_factors: vec![
            "model_or_mode_change",
            "task_complexity_change",
            "other_device_usage",
            "plan_or_quota_policy_change",
        ],
        decision: "not_assessed",
        backtest_status: "not_started",
    };
    let fail = |mut out: PaceEstimate, reason| {
        out.reason_code = reason;
        out.rate_range.reason_code = reason;
        out.balance_at_horizon.reason_code = reason;
        out.depletion_time_range.reason_code = reason;
        out
    };
    if truncated {
        return fail(out, "query_truncated");
    }
    if !(current.limit_id == "codex" || current.limit_id.starts_with("codex:")) {
        return fail(out, "unsupported_window");
    }
    let Some(window) = current
        .window_minutes
        .and_then(|m| m.checked_mul(60))
        .filter(|s| *s >= 9000 && *s <= 366 * 86400)
    else {
        return fail(out, "unsupported_window");
    };
    let block = 7200.min(window / 10) as i64;
    out.history_coverage.required_seconds = (3 * block) as u64;
    if out.current_remaining_percent.is_none()
        || !matches!(
            current.availability,
            Availability::Complete | Availability::Partial
        )
        || current.freshness == Freshness::NotApplicable
        || !compatible(current.compatibility)
    {
        return fail(out, "source_unavailable");
    }
    let source_age = ms(now) - ms(&current.observed_at);
    if source_age < 0 {
        return fail(out, "clock_discontinuity");
    }
    if source_age > MAX_SOURCE_AGE_SECONDS * 1000 {
        return fail(out, "stale_source");
    }
    let Some(plan) = normalize_plan_type(current.account_plan_type.as_deref()) else {
        return fail(out, "plan_context_missing");
    };
    let Some(reset) = current.resets_at.as_ref().map(ms) else {
        return fail(out, "missing_reset_time");
    };
    if reset <= ms(now) {
        return fail(out, "window_ended");
    }
    if current.remaining_percent == 0.0 {
        return fail(out, "balance_empty");
    }
    let mut rows: Vec<_> = samples
        .iter()
        .filter(|s| s.quota.limit_id == current.limit_id && ms(&s.quota.captured_at) <= ms(now))
        .collect();
    rows.sort_by(|a, b| {
        ms(&a.quota.captured_at)
            .cmp(&ms(&b.quota.captured_at))
            .then(a.quota.snapshot_id.cmp(&b.quota.snapshot_id))
    });
    let Some(last) = rows.last().copied() else {
        return out;
    };
    let anchor = ms(&last.quota.captured_at);
    out.cached_source |= anchor < ms(&current.observed_at);
    out.observed_at = Some(last.quota.captured_at.clone());
    if ms(now) - anchor > MAX_SOURCE_AGE_SECONDS * 1000 {
        return fail(out, "stale_source");
    }
    if (last.quota.remaining_percent * 100.0).round() != (current.remaining_percent * 100.0).round()
        || last.quota.window_minutes != current.window_minutes
        || last.quota.resets_at.as_ref().map(ms) != Some(reset)
    {
        return fail(out, "current_capture_pending");
    }
    let lower_time = anchor - 3 * block * 1000 - 900_000;
    rows.retain(|s| ms(&s.quota.captured_at) >= lower_time);
    out.history_coverage.sample_count = rows.len();
    if let Some(first) = rows.first() {
        out.history_coverage.covered_seconds =
            ((anchor - ms(&first.quota.captured_at)) / 1000) as u64;
    }
    if rows.windows(2).any(|pair| {
        ms(&pair[0].quota.captured_at) == ms(&pair[1].quota.captured_at) && !same(pair[0], pair[1])
    }) {
        return fail(out, "conflicting_samples");
    }
    rows.dedup_by(|a, b| ms(&a.quota.captured_at) == ms(&b.quota.captured_at));
    let mut indices = vec![];
    for step in (0..=3).rev() {
        let target = anchor - step * block * 1000;
        let Some(index) = rows
            .iter()
            .rposition(|s| ms(&s.quota.captured_at) <= target)
        else {
            return out;
        };
        if target - ms(&rows[index].quota.captured_at) > 900.min(block / 4) * 1000 {
            return fail(out, "observation_gap");
        }
        indices.push(index);
    }
    let used = &rows[indices[0]..=indices[3]];
    out.input_snapshot_ids = used.iter().map(|s| s.quota.snapshot_id.clone()).collect();
    out.history_coverage.sample_count = used.len();
    out.history_coverage.covered_seconds =
        ((anchor - ms(&used[0].quota.captured_at)) / 1000) as u64;
    out.history_coverage.maximum_gap_seconds = used
        .windows(2)
        .map(|p| ((ms(&p[1].quota.captured_at) - ms(&p[0].quota.captured_at) + 999) / 1000) as u64)
        .max()
        .unwrap_or(0);
    if used.iter().any(|s| s.account_plan_type.is_none()) {
        return fail(out, "plan_context_missing");
    }
    if used
        .iter()
        .any(|s| normalize_plan_type(s.account_plan_type.as_deref()).as_deref() != Some(&plan))
    {
        return fail(out, "plan_type_changed");
    }
    if used.iter().any(|s| {
        s.quota.window_minutes != current.window_minutes
            || s.quota.resets_at.as_ref().map(ms) != Some(reset)
            || ms(&s.quota.captured_at) >= reset
    }) {
        return fail(out, "window_changed");
    }
    if used.iter().any(|s| {
        percent(s.quota.remaining_percent).is_none()
            || s.quota.freshness != Freshness::Live
            || !matches!(
                s.quota.availability,
                Availability::Complete | Availability::Partial
            )
            || !compatible(s.quota.compatibility)
    }) {
        return fail(out, "source_unavailable");
    }
    if used
        .windows(2)
        .any(|p| p[1].quota.remaining_percent > p[0].quota.remaining_percent)
    {
        return fail(out, "balance_increased");
    }
    if out.history_coverage.maximum_gap_seconds > 1800.min(block / 2) as u64 {
        return fail(out, "observation_gap");
    }
    out.compatibility_unverified = current.compatibility != Compatibility::Tested
        || used
            .iter()
            .any(|s| s.quota.compatibility != Compatibility::Tested);
    // The forecast contract allows unverified compatibility only with live
    // inputs. A recent cache is eligible only when compatibility is tested.
    if out.compatibility_unverified && out.cached_source {
        return fail(out, "source_unavailable");
    }
    for pair in indices.windows(2) {
        let a = &rows[pair[0]].quota;
        let b = &rows[pair[1]].quota;
        out.observed_block_rates.push(
            (a.remaining_percent - b.remaining_percent)
                / ((ms(&b.captured_at) - ms(&a.captured_at)) as f64 / 3_600_000.0),
        );
    }
    let slow = out
        .observed_block_rates
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let fast = out.observed_block_rates.iter().copied().fold(0.0, f64::max);
    if slow <= 0.0 || (used[0].quota.remaining_percent - last.quota.remaining_percent) < 1.0 - 1e-9
    {
        return fail(out, "insufficient_usage");
    }
    if fast / slow > 4.0 {
        out.data_stability = "variable";
        return fail(out, "unstable_pace");
    }
    let horizon = (anchor + 3 * block * 1000).min(reset - 1000);
    if horizon <= ms(now) {
        return fail(out, "window_ended");
    }
    let hours = (horizon - anchor) as f64 / 3_600_000.0;
    out.forecast_horizon = Some(at(horizon));
    out.rate_range = PaceRange::available(slow, fast);
    out.balance_at_horizon = PaceRange::available(
        (last.quota.remaining_percent - fast * hours).max(0.0),
        (last.quota.remaining_percent - slow * hours).max(0.0),
    );
    let early = anchor + (last.quota.remaining_percent / fast * 3_600_000.0).floor() as i64;
    let late = anchor + (last.quota.remaining_percent / slow * 3_600_000.0).ceil() as i64;
    if late <= horizon {
        out.depletion_time_range = PaceTimeRange {
            status: "available",
            earliest_at: Some(at(early)),
            latest_at: Some(at(late)),
            reason_code: "pace_scenario",
        };
    } else {
        out.depletion_time_range.reason_code = "beyond_horizon";
    }
    out.state = "pace_only";
    out.reason_code = "pace_scenario";
    out.data_stability = "recent_observed_range";
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn time(seconds: i64) -> UtcTimestamp {
        at(ms(&UtcTimestamp::parse("2026-09-26T00:00:00Z").unwrap()) + seconds * 1000)
    }
    fn fixture(
        minutes: u64,
        step: i64,
        duration: i64,
        start_balance: f64,
        rate: f64,
    ) -> (Vec<PaceSample>, PaceCurrent, UtcTimestamp) {
        let reset = time(minutes as i64 * 60);
        let samples: Vec<_> = (0..=duration / step)
            .map(|n| PaceSample {
                quota: ActivityQuotaSample {
                    snapshot_id: format!("sample-{n}"),
                    captured_at: time(n * step),
                    limit_id: "codex:window".into(),
                    window_minutes: Some(minutes),
                    remaining_percent: start_balance - n as f64 * step as f64 / 3600.0 * rate,
                    resets_at: Some(reset.clone()),
                    availability: Availability::Complete,
                    freshness: Freshness::Live,
                    compatibility: Compatibility::Tested,
                },
                account_plan_type: Some("plus".into()),
            })
            .collect();
        let last = &samples.last().unwrap().quota;
        let now = time(duration);
        let current = PaceCurrent {
            limit_id: last.limit_id.clone(),
            window_minutes: Some(minutes),
            remaining_percent: last.remaining_percent,
            resets_at: Some(reset),
            observed_at: now.clone(),
            account_plan_type: Some("plus".into()),
            availability: Availability::Complete,
            freshness: Freshness::Live,
            compatibility: Compatibility::Tested,
        };
        (samples, current, now)
    }
    #[test]
    fn constant_pace_has_traceable_elapsed_hour_scenario_not_active_time_or_plan_fit() {
        let (rows, current, now) = fixture(10080, 900, 21600, 80.0, 2.0);
        let result = estimate_pace(&rows, &current, &now, false);
        assert_eq!(result.state, "pace_only");
        assert_eq!(result.rate_range.lower, Some(2.0));
        assert_eq!(result.balance_at_horizon.lower, Some(56.0));
        assert_eq!(result.balance_at_horizon.upper, Some(56.0));
        assert_eq!(result.input_snapshot_ids.len(), 25);
        assert_eq!(result.history_coverage.covered_seconds, 21600);
        assert_eq!(
            result.estimated_codex_active_hours_range.status,
            "unavailable"
        );
        assert_eq!(result.decision, "not_assessed");
        assert_eq!(result.backtest_status, "not_started");
        assert_eq!(result.depletion_time_range.reason_code, "beyond_horizon");
    }
    #[test]
    fn short_windows_scale_blocks_and_only_bound_depletion_before_current_reset() {
        let (rows, current, now) = fixture(300, 300, 5400, 10.0, 4.0);
        let value = estimate_pace(&rows, &current, &now, false);
        assert_eq!(value.state, "pace_only");
        assert_eq!(value.history_coverage.required_seconds, 5400);
        assert_eq!(value.forecast_horizon, Some(time(10800)));
        assert_eq!(value.depletion_time_range.earliest_at, Some(time(9000)));
        assert_eq!(value.depletion_time_range.latest_at, Some(time(9000)));
        assert_eq!(value.balance_at_horizon.lower, Some(0.0));
        let (mut rows, mut current, now) = fixture(10080, 900, 21600, 80.0, 2.0);
        for row in &mut rows {
            row.quota.resets_at = Some(time(25200));
        }
        current.resets_at = Some(time(25200));
        assert_eq!(
            estimate_pace(&rows, &current, &now, false).forecast_horizon,
            Some(time(25199))
        );
    }
    #[test]
    fn all_three_blocks_need_observable_use_and_wide_changes_abstain() {
        let (mut rows, mut current, now) = fixture(10080, 900, 21600, 80.0, 0.0);
        assert_eq!(
            estimate_pace(&rows, &current, &now, false).reason_code,
            "insufficient_usage"
        );
        for (n, row) in rows.iter_mut().enumerate() {
            row.quota.remaining_percent = 80.0
                - if n <= 16 {
                    n as f64 / 4.0
                } else {
                    4.0 + (n - 16) as f64 * 2.0
                };
        }
        current.remaining_percent = rows.last().unwrap().quota.remaining_percent;
        assert_eq!(
            estimate_pace(&rows, &current, &now, false).reason_code,
            "unstable_pace"
        );
        let (rows, current, now) = fixture(10080, 900, 21600, 80.0, 0.1);
        assert_eq!(
            estimate_pace(&rows, &current, &now, false).reason_code,
            "insufficient_usage"
        );
    }
    #[test]
    fn captures_never_bridge_plan_reset_rise_missing_context_or_clock_conflicts() {
        let (rows, current, now) = fixture(10080, 900, 21600, 80.0, 2.0);
        for (kind, expected) in [
            (0, "plan_context_missing"),
            (1, "plan_type_changed"),
            (2, "window_changed"),
            (3, "balance_increased"),
            (4, "conflicting_samples"),
            (5, "source_unavailable"),
        ] {
            let mut changed = rows.clone();
            match kind {
                0 => changed[2].account_plan_type = None,
                1 => changed[2].account_plan_type = Some("pro".into()),
                2 => changed[2].quota.resets_at = Some(time(90000)),
                3 => changed[3].quota.remaining_percent = changed[2].quota.remaining_percent + 1.0,
                4 => {
                    let mut duplicate = changed[2].clone();
                    duplicate.quota.remaining_percent -= 1.0;
                    changed.push(duplicate);
                }
                _ => changed[3].quota.compatibility = Compatibility::KnownBroken,
            }
            let value = estimate_pace(&changed, &current, &now, false);
            assert_eq!(value.reason_code, expected);
            assert_eq!(value.state, "abstained");
            assert!(value.balance_at_horizon.lower.is_none());
        }
        let mut future = current.clone();
        future.observed_at = time(21601);
        assert_eq!(
            estimate_pace(&rows, &future, &now, false).reason_code,
            "clock_discontinuity"
        );
        let mut different = current.clone();
        different.remaining_percent -= 1.0;
        assert_eq!(
            estimate_pace(&rows, &different, &now, false).reason_code,
            "current_capture_pending"
        );
    }
    #[test]
    fn gaps_truncation_and_staleness_have_explicit_unavailable_not_zero_values() {
        let (rows, current, now) = fixture(10080, 900, 21600, 80.0, 2.0);
        assert_eq!(
            estimate_pace(&rows, &current, &now, true).reason_code,
            "query_truncated"
        );
        assert_eq!(
            estimate_pace(&rows, &current, &time(23401), false).reason_code,
            "stale_source"
        );
        let filtered: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(n, _)| *n < 10 || *n > 12)
            .map(|(_, r)| r.clone())
            .collect();
        assert_eq!(
            estimate_pace(&filtered, &current, &now, false).reason_code,
            "observation_gap"
        );
        let value = estimate_pace(&rows[2..], &current, &now, false);
        assert_eq!(value.state, "abstained");
        assert_eq!(value.balance_at_horizon.lower, None);
    }
    #[test]
    fn partial_and_unverified_are_permitted_only_as_explicit_experimental_pace() {
        let (mut rows, mut current, now) = fixture(10080, 900, 21600, 80.0, 2.0);
        current.availability = Availability::Partial;
        current.compatibility = Compatibility::NotTested;
        for row in &mut rows {
            row.quota.availability = Availability::Partial;
            row.quota.compatibility = Compatibility::ExpectedCompatible;
        }
        let value = estimate_pace(&rows, &current, &now, false);
        assert_eq!(value.state, "pace_only");
        assert!(value.compatibility_unverified);
        assert!(!value.cached_source);
        assert_eq!(value.decision, "not_assessed");
        current.freshness = Freshness::Stale;
        assert_eq!(
            estimate_pace(&rows, &current, &now, false).reason_code,
            "source_unavailable"
        );
        current.compatibility = Compatibility::Tested;
        for row in &mut rows {
            row.quota.compatibility = Compatibility::Tested;
        }
        let cached = estimate_pace(&rows, &current, &now, false);
        assert_eq!(cached.state, "pace_only");
        assert!(cached.cached_source);
        current.freshness = Freshness::NotApplicable;
        assert_eq!(
            estimate_pace(&rows, &current, &now, false).state,
            "abstained"
        );
    }
    #[test]
    fn equivalent_utc_and_identical_duplicates_do_not_create_extra_samples() {
        let (mut rows, current, now) = fixture(10080, 900, 21600, 80.0, 2.0);
        rows[3].quota.resets_at = Some(
            UtcTimestamp::parse(
                current
                    .resets_at
                    .as_ref()
                    .unwrap()
                    .as_str()
                    .replace('Z', "+00:00"),
            )
            .unwrap(),
        );
        let mut duplicate = rows[4].clone();
        duplicate.quota.snapshot_id = "duplicate".into();
        rows.push(duplicate);
        let value = estimate_pace(&rows, &current, &now, false);
        assert_eq!(value.state, "pace_only");
        assert_eq!(value.history_coverage.sample_count, 25);
        assert_eq!(normalize_plan_type(Some("ChatGPTPro")), Some("pro".into()));
        assert!(normalize_plan_type(Some("bad secret value")).is_none());
    }
    #[test]
    fn synthetic_holdout_is_not_training_data_or_proof_of_real_accuracy() {
        let (rows, current, now) = fixture(10080, 900, 21600, 80.0, 2.0);
        let prediction = estimate_pace(&rows, &current, &now, false);
        let continuation_balance = 44.0_f64; // rate doubles after the training period.
        assert!(continuation_balance < prediction.balance_at_horizon.lower.unwrap());
        let (future, _, _) = fixture(10080, 900, 43200, 80.0, 2.0);
        // Supplying future rows must not leak them into an earlier evaluation.
        let replay = estimate_pace(&future, &current, &now, false);
        assert_eq!(
            prediction.balance_at_horizon.lower,
            replay.balance_at_horizon.lower
        );
        assert_eq!(prediction.input_snapshot_ids, replay.input_snapshot_ids);
        assert_eq!(replay.backtest_status, "not_started");
    }
}
