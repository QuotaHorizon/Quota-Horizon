//! Prospective, immutable experimental pace checks. Never plan-fit validation.
use crate::{Availability, Compatibility, Freshness, UtcTimestamp, pace::*};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const OUTCOME_VERSION: &str = "pace-balance-outcome-v1";
pub const MAX_TRIAL_SAMPLES: usize = 4096;
const MAX_FROZEN_BYTES: usize = 4 * 1024 * 1024;
pub fn epoch(value: &UtcTimestamp) -> i64 {
    DateTime::parse_from_rfc3339(value.as_str())
        .expect("validated UTC")
        .timestamp_millis()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaceTrialSeed {
    pub algorithm_version: String,
    pub outcome_version: String,
    pub issued_at: UtcTimestamp,
    pub observed_at: UtcTimestamp,
    pub horizon: UtcTimestamp,
    pub current: PaceCurrent,
    pub samples: Vec<PaceSample>,
    #[serde(with = "crate::frozen_number::pair")]
    pub balance_range: [f64; 2],
    #[serde(with = "crate::frozen_number::pair")]
    pub rate_range: [f64; 2],
    pub assumptions: Vec<String>,
    pub invalidation_factors: Vec<String>,
    pub compatibility_unverified: bool,
}

impl PaceTrialSeed {
    pub fn freeze(
        samples: &[PaceSample],
        current: &PaceCurrent,
        now: &UtcTimestamp,
        truncated: bool,
    ) -> Option<Self> {
        // Prospective records are never manufactured from a displayed cache.
        if current.freshness != Freshness::Live {
            return None;
        }
        let result = estimate_pace(samples, current, now, truncated);
        if result.state != "pace_only"
            || result.cached_source
            || result.observed_at.as_ref().map(epoch) != Some(epoch(&current.observed_at))
        {
            return None;
        }
        let ids: HashSet<_> = result.input_snapshot_ids.iter().collect();
        let samples: Vec<_> = samples
            .iter()
            .filter(|s| ids.contains(&s.quota.snapshot_id))
            .cloned()
            .collect();
        if samples.is_empty() || samples.len() > MAX_TRIAL_SAMPLES {
            return None;
        }
        Some(Self {
            algorithm_version: result.algorithm_version.into(),
            outcome_version: OUTCOME_VERSION.into(),
            issued_at: now.clone(),
            observed_at: result.observed_at?,
            horizon: result.forecast_horizon?,
            current: current.clone(),
            samples,
            balance_range: [
                result.balance_at_horizon.lower?,
                result.balance_at_horizon.upper?,
            ],
            rate_range: [result.rate_range.lower?, result.rate_range.upper?],
            assumptions: result.assumptions.into_iter().map(str::to_owned).collect(),
            invalidation_factors: result
                .invalidation_factors
                .into_iter()
                .map(str::to_owned)
                .collect(),
            compatibility_unverified: result.compatibility_unverified,
        })
    }
    pub fn grace_seconds(&self) -> u64 {
        ((self.current.window_minutes.unwrap_or(0).saturating_mul(60) / 10).min(7200) / 4).min(900)
    }
    pub fn query_until(&self, now: &UtcTimestamp) -> UtcTimestamp {
        let end = epoch(now).min(epoch(&self.horizon) + self.grace_seconds() as i64 * 1000);
        UtcTimestamp::parse(
            chrono::DateTime::from_timestamp_millis(end)
                .expect("bounded UTC")
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
        .expect("UTC")
    }
    pub fn encode(&self) -> Option<String> {
        encode(self)
    }
    pub fn decode(json: &str) -> Option<Self> {
        decode(json)
    }
}
fn encode<T: Serialize>(value: &T) -> Option<String> {
    let json = serde_json::to_string(value).ok()?;
    (json.len() <= MAX_FROZEN_BYTES).then_some(json)
}
fn decode<T: for<'de> Deserialize<'de>>(json: &str) -> Option<T> {
    (json.len() <= MAX_FROZEN_BYTES)
        .then(|| serde_json::from_str(json).ok())
        .flatten()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceOutcomeSummary {
    pub verifier_version: String,
    pub assessed_at: UtcTimestamp,
    pub classification: String,
    pub reason_code: String,
    pub observed_range: Option<[f64; 2]>,
    pub observed_from: Option<UtcTimestamp>,
    pub observed_until: Option<UtcTimestamp>,
    pub sample_count: usize,
    pub maximum_gap_seconds: u64,
    pub compatibility_unverified: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaceTrialOutcome {
    pub summary: PaceOutcomeSummary,
    pub samples: Vec<PaceSample>,
}
impl PaceTrialOutcome {
    pub fn encode(&self) -> Option<String> {
        encode(self)
    }
    pub fn decode(json: &str) -> Option<Self> {
        decode(json)
    }
}

/// None means genuinely awaiting future observations. A terminal result is
/// append-only; late observations cannot rewrite the original assessment.
pub fn assess(
    seed: &PaceTrialSeed,
    future: &[PaceSample],
    now: &UtcTimestamp,
    truncated: bool,
) -> Option<PaceTrialOutcome> {
    let mut out = PaceTrialOutcome {
        summary: PaceOutcomeSummary {
            verifier_version: OUTCOME_VERSION.into(),
            assessed_at: now.clone(),
            classification: "unscorable".into(),
            reason_code: "target_samples_missing".into(),
            observed_range: None,
            observed_from: None,
            observed_until: None,
            sample_count: 0,
            maximum_gap_seconds: 0,
            compatibility_unverified: seed.compatibility_unverified,
        },
        samples: vec![],
    };
    let fail = |mut out: PaceTrialOutcome, reason: &str| {
        out.summary.reason_code = reason.into();
        Some(out)
    };
    if seed.algorithm_version != PACE_ALGORITHM_VERSION || seed.outcome_version != OUTCOME_VERSION {
        return fail(out, "unsupported_algorithm");
    }
    // Check that the frozen numbers really belong to the original version.
    let replay = estimate_pace(&seed.samples, &seed.current, &seed.issued_at, false);
    if seed.samples.len() > MAX_TRIAL_SAMPLES
        || replay.state != "pace_only"
        || replay.forecast_horizon.as_ref() != Some(&seed.horizon)
        || replay.observed_at.as_ref() != Some(&seed.observed_at)
        || replay.balance_at_horizon.lower != Some(seed.balance_range[0])
        || replay.balance_at_horizon.upper != Some(seed.balance_range[1])
        || replay.rate_range.lower != Some(seed.rate_range[0])
        || replay.rate_range.upper != Some(seed.rate_range[1])
    {
        return fail(out, "frozen_inputs_invalid");
    }
    let target = epoch(&seed.horizon);
    if epoch(now) < target {
        return None;
    }
    if truncated {
        return fail(out, "query_truncated");
    }
    let grace = seed.grace_seconds() as i64 * 1000;
    let mut rows: Vec<_> = future
        .iter()
        .filter(|s| {
            s.quota.limit_id == seed.current.limit_id
                && epoch(&s.quota.captured_at) > epoch(&seed.issued_at)
                && epoch(&s.quota.captured_at) <= epoch(now)
                && epoch(&s.quota.captured_at) <= target + grace
        })
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        epoch(&a.quota.captured_at)
            .cmp(&epoch(&b.quota.captured_at))
            .then(a.quota.snapshot_id.cmp(&b.quota.snapshot_id))
    });
    if rows.len() > MAX_TRIAL_SAMPLES {
        return fail(out, "query_truncated");
    }
    out.samples = rows.clone();
    out.summary.sample_count = rows.len();
    let Some(right) = rows
        .iter()
        .position(|s| epoch(&s.quota.captured_at) >= target)
    else {
        return if epoch(now) <= target + grace {
            None
        } else {
            Some(out)
        };
    };
    let right_time = epoch(&rows[right].quota.captured_at);
    // Include every capture at the chosen right boundary to detect conflicts.
    rows.retain(|s| epoch(&s.quota.captured_at) <= right_time);
    if rows.len() > MAX_TRIAL_SAMPLES {
        return fail(out, "query_truncated");
    }
    out.samples = rows.clone();
    out.summary.sample_count = rows.len();
    if rows.windows(2).any(|p| {
        epoch(&p[0].quota.captured_at) == epoch(&p[1].quota.captured_at)
            && (p[0].quota.remaining_percent != p[1].quota.remaining_percent
                || p[0].quota.resets_at.as_ref().map(epoch)
                    != p[1].quota.resets_at.as_ref().map(epoch)
                || p[0].account_plan_type != p[1].account_plan_type
                || p[0].quota.window_minutes != p[1].quota.window_minutes
                || p[0].quota.availability != p[1].quota.availability
                || p[0].quota.compatibility != p[1].quota.compatibility
                || p[0].quota.freshness != p[1].quota.freshness)
    }) {
        return fail(out, "conflicting_samples");
    }
    rows.dedup_by(|a, b| epoch(&a.quota.captured_at) == epoch(&b.quota.captured_at));
    let Some(left) = rows
        .iter()
        .rposition(|s| epoch(&s.quota.captured_at) <= target)
    else {
        return Some(out);
    };
    if target - epoch(&rows[left].quota.captured_at) > grace {
        return Some(out);
    }
    let plan = normalize_plan_type(seed.current.account_plan_type.as_deref());
    let reset = seed.current.resets_at.as_ref().map(epoch);
    if rows
        .iter()
        .any(|s| normalize_plan_type(s.account_plan_type.as_deref()) != plan)
    {
        return fail(out, "plan_type_changed");
    }
    if rows.iter().any(|s| {
        s.quota.window_minutes != seed.current.window_minutes
            || s.quota.resets_at.as_ref().map(epoch) != reset
            || reset.is_none_or(|r| epoch(&s.quota.captured_at) >= r)
    }) {
        return fail(out, "window_changed");
    }
    if rows.iter().any(|s| {
        !s.quota.remaining_percent.is_finite()
            || !(0.0..=100.0).contains(&s.quota.remaining_percent)
            || s.quota.freshness != Freshness::Live
            || !matches!(
                s.quota.availability,
                Availability::Complete | Availability::Partial
            )
            || !matches!(
                s.quota.compatibility,
                Compatibility::Tested
                    | Compatibility::ExpectedCompatible
                    | Compatibility::NotTested
            )
    }) {
        return fail(out, "source_unavailable");
    }
    let mut previous_time = epoch(&seed.observed_at);
    let mut previous_balance = seed
        .samples
        .iter()
        .max_by_key(|s| epoch(&s.quota.captured_at))
        .expect("replayed inputs")
        .quota
        .remaining_percent;
    for row in &rows {
        let time = epoch(&row.quota.captured_at);
        out.summary.maximum_gap_seconds = out
            .summary
            .maximum_gap_seconds
            .max(((time - previous_time + 999) / 1000) as u64);
        if row.quota.remaining_percent > previous_balance {
            return fail(out, "balance_increased");
        }
        previous_time = time;
        previous_balance = row.quota.remaining_percent;
    }
    let max_gap = (seed.current.window_minutes.unwrap_or(0).saturating_mul(60) / 10).min(7200) / 2;
    if out.summary.maximum_gap_seconds > max_gap.min(1800) {
        return fail(out, "observation_gap");
    }
    out.summary.compatibility_unverified |= rows
        .iter()
        .any(|s| s.quota.compatibility != Compatibility::Tested);
    let left = &rows[left].quota;
    let right = &rows.last().expect("nonempty").quota;
    let actual = [right.remaining_percent, left.remaining_percent];
    out.summary.observed_range = Some(actual);
    out.summary.observed_from = Some(left.captured_at.clone());
    out.summary.observed_until = Some(right.captured_at.clone());
    let epsilon = 1e-9;
    let classification = if actual[0] >= seed.balance_range[0] - epsilon
        && actual[1] <= seed.balance_range[1] + epsilon
    {
        "within_band"
    } else if actual[1] < seed.balance_range[0] - epsilon {
        "below_band"
    } else if actual[0] > seed.balance_range[1] + epsilon {
        "above_band"
    } else {
        "unscorable"
    };
    out.summary.classification = classification.into();
    out.summary.reason_code = if classification == "unscorable" {
        "boundary_uncertain"
    } else {
        "observed_comparison"
    }
    .into();
    Some(out)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceTrialView {
    pub trial_id: String,
    pub algorithm_version: String,
    pub issued_at: UtcTimestamp,
    pub observed_at: UtcTimestamp,
    pub forecast_horizon: UtcTimestamp,
    pub limit_id: String,
    pub window_minutes: Option<u64>,
    pub balance_range: [f64; 2],
    pub compatibility_unverified: bool,
    pub outcome: Option<PaceOutcomeSummary>,
}
pub fn trial_view(
    id: String,
    seed: PaceTrialSeed,
    outcome: Option<PaceTrialOutcome>,
) -> PaceTrialView {
    PaceTrialView {
        trial_id: id,
        algorithm_version: seed.algorithm_version,
        issued_at: seed.issued_at,
        observed_at: seed.observed_at,
        forecast_horizon: seed.horizon,
        limit_id: seed.current.limit_id,
        window_minutes: seed.current.window_minutes,
        balance_range: seed.balance_range,
        compatibility_unverified: seed.compatibility_unverified,
        outcome: outcome.map(|o| o.summary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_quota::ActivityQuotaSample;
    fn at(seconds: i64) -> UtcTimestamp {
        UtcTimestamp::parse(
            chrono::DateTime::from_timestamp(1_790_380_800 + seconds, 0)
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        )
        .unwrap()
    }
    fn sample(second: i64, balance: f64) -> PaceSample {
        PaceSample {
            account_plan_type: Some("plus".into()),
            quota: ActivityQuotaSample {
                snapshot_id: format!("sample-{second}"),
                captured_at: at(second),
                limit_id: "codex:weekly".into(),
                window_minutes: Some(10080),
                remaining_percent: balance,
                resets_at: Some(at(604800)),
                availability: Availability::Complete,
                freshness: Freshness::Live,
                compatibility: Compatibility::Tested,
            },
        }
    }
    fn fixture() -> (PaceTrialSeed, Vec<PaceSample>) {
        let samples: Vec<_> = (0..=24)
            .map(|n| sample(n * 900, 80.0 - n as f64 / 2.0))
            .collect();
        let current = PaceCurrent {
            limit_id: "codex:weekly".into(),
            window_minutes: Some(10080),
            remaining_percent: 68.0,
            resets_at: Some(at(604800)),
            observed_at: at(21600),
            account_plan_type: Some("plus".into()),
            availability: Availability::Complete,
            freshness: Freshness::Live,
            compatibility: Compatibility::Tested,
        };
        (
            PaceTrialSeed::freeze(&samples, &current, &at(21600), false).unwrap(),
            (1..=24)
                .map(|n| sample(21600 + n * 900, 68.0 - n as f64 / 2.0))
                .collect(),
        )
    }
    #[test]
    fn frozen_inputs_round_trip_and_future_checks_do_not_refit() {
        let (seed, future) = fixture();
        let decoded = PaceTrialSeed::decode(&seed.encode().unwrap()).unwrap();
        assert_eq!(decoded.samples.len(), 25);
        assert!(assess(&decoded, &future, &at(43199), false).is_none());
        let result = assess(&decoded, &future, &at(43200), false).unwrap();
        assert_eq!(result.summary.classification, "within_band");
        assert_eq!(result.summary.observed_range, Some([56.0, 56.0]));
        assert_eq!(
            PaceTrialOutcome::decode(&result.encode().unwrap())
                .unwrap()
                .summary
                .sample_count,
            24
        );
        let mut changed = future.clone();
        for (n, row) in changed.iter_mut().enumerate() {
            row.quota.remaining_percent = 68.0 - (n + 1) as f64;
        }
        assert_eq!(
            assess(&decoded, &changed, &at(43200), false)
                .unwrap()
                .summary
                .classification,
            "below_band"
        );
        for (n, row) in changed.iter_mut().enumerate() {
            row.quota.remaining_percent = 68.0 - (n + 1) as f64 / 4.0;
        }
        assert_eq!(
            assess(&decoded, &changed, &at(43200), false)
                .unwrap()
                .summary
                .classification,
            "above_band"
        );
        assert_eq!(seed.balance_range, [56.0, 56.0]);
    }
    #[test]
    fn uncertain_brackets_and_missing_future_never_count_as_hits() {
        let (seed, mut future) = fixture();
        future.pop();
        future.push(sample(44100, 55.5));
        assert!(assess(&seed, &future, &at(43200), false).is_none());
        let value = assess(&seed, &future, &at(44100), false).unwrap();
        assert_eq!(value.summary.reason_code, "boundary_uncertain");
        assert_eq!(value.summary.classification, "unscorable");
        assert_eq!(value.summary.observed_range, Some([55.5, 56.5]));
        future.pop();
        assert_eq!(
            assess(&seed, &future, &at(44101), false)
                .unwrap()
                .summary
                .reason_code,
            "target_samples_missing"
        );
        assert_eq!(
            assess(&seed, &future, &at(44101), true)
                .unwrap()
                .summary
                .reason_code,
            "query_truncated"
        );
    }
    #[test]
    fn boundary_changes_rises_conflicts_and_gaps_are_not_prediction_errors() {
        let (seed, future) = fixture();
        for (kind, reason) in [
            (0, "plan_type_changed"),
            (1, "window_changed"),
            (2, "balance_increased"),
            (3, "conflicting_samples"),
            (4, "observation_gap"),
            (5, "source_unavailable"),
        ] {
            let mut rows = future.clone();
            match kind {
                0 => rows[4].account_plan_type = Some("pro".into()),
                1 => rows[4].quota.resets_at = Some(at(500000)),
                2 => rows[4].quota.remaining_percent = 69.0,
                3 => {
                    let mut row = rows[4].clone();
                    row.quota.remaining_percent -= 1.0;
                    rows.push(row);
                }
                4 => {
                    rows.drain(4..6);
                }
                _ => rows[4].quota.compatibility = Compatibility::KnownBroken,
            }
            let value = assess(&seed, &rows, &at(43200), false).unwrap();
            assert_eq!(value.summary.reason_code, reason);
            assert_eq!(value.summary.classification, "unscorable");
        }
    }
    #[test]
    fn caches_late_generation_unknown_versions_and_changed_frozen_numbers_are_rejected() {
        let (mut seed, future) = fixture();
        let mut current = seed.current.clone();
        current.freshness = Freshness::Stale;
        assert!(PaceTrialSeed::freeze(&seed.samples, &current, &seed.issued_at, false).is_none());
        assert!(PaceTrialSeed::freeze(&seed.samples, &seed.current, &at(43201), false).is_none());
        seed.balance_range = [0.0, 100.0];
        assert_eq!(
            assess(&seed, &future, &at(43200), false)
                .unwrap()
                .summary
                .reason_code,
            "frozen_inputs_invalid"
        );
        seed.algorithm_version = "different-version".into();
        assert_eq!(
            assess(&seed, &future, &at(43200), false)
                .unwrap()
                .summary
                .reason_code,
            "unsupported_algorithm"
        );
    }
    #[test]
    fn before_issue_or_after_boundary_samples_cannot_change_an_outcome() {
        let (seed, mut future) = fixture();
        future.push(sample(21599, 100.0));
        future.push(sample(44101, 0.0));
        let result = assess(&seed, &future, &at(45000), false).unwrap();
        assert_eq!(result.summary.classification, "within_band");
        assert_eq!(result.samples.len(), 24);
        assert_eq!(seed.grace_seconds(), 900);
        let mut short = seed.clone();
        short.current.window_minutes = Some(300);
        assert_eq!(short.grace_seconds(), 450);
    }
    #[test]
    fn fractional_basis_points_survive_frozen_json_replay() {
        for drop in 10..=50 {
            let (mut seed, _) = fixture();
            for (index, sample) in seed.samples.iter_mut().enumerate() {
                sample.quota.remaining_percent = (9999 - index as i64 * drop) as f64 / 100.0;
            }
            seed.current.remaining_percent = (9999 - 24 * drop) as f64 / 100.0;
            let frozen =
                PaceTrialSeed::freeze(&seed.samples, &seed.current, &seed.issued_at, false)
                    .unwrap();
            let restored = PaceTrialSeed::decode(&frozen.encode().unwrap()).unwrap();
            let future: Vec<_> = (25..=48)
                .map(|n| sample(n * 900, (9999 - n * drop) as f64 / 100.0))
                .collect();
            assert_ne!(
                assess(&restored, &future, &at(43200), false)
                    .unwrap()
                    .summary
                    .reason_code,
                "frozen_inputs_invalid",
                "drop {drop}"
            );
        }
    }
}
