//! Descriptive counts of retained prospective trials, never model certification.
use crate::{UtcTimestamp, pace::PACE_ALGORITHM_VERSION, pace_trial::*};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceEvidenceCounts {
    pub total: u32,
    pub pending: u32,
    pub within: u32,
    pub below: u32,
    pub above: u32,
    pub unscorable: u32,
}
impl PaceEvidenceCounts {
    fn add(&mut self, class: &str) {
        self.total += 1;
        match class {
            "pending" => self.pending += 1,
            "within_band" => self.within += 1,
            "below_band" => self.below += 1,
            "above_band" => self.above += 1,
            _ => self.unscorable += 1,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceEvidenceGroup {
    pub algorithm_version: String,
    pub verifier_version: String,
    pub window_minutes: Option<u64>,
    pub compatibility_unverified: bool,
    pub counts: PaceEvidenceCounts,
    /// Ranges describe the inputs in this group, not confidence or active time.
    pub training_maximum_gap_seconds: Option<[u64; 2]>,
    pub training_rate_ratio: Option<[f64; 2]>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceEvidenceReason {
    pub reason_code: String,
    pub count: u32,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceEvidenceSummary {
    pub generated_at: UtcTimestamp,
    pub oldest_issued_at: Option<UtcTimestamp>,
    pub newest_issued_at: Option<UtcTimestamp>,
    pub limited: bool,
    pub counts: PaceEvidenceCounts,
    pub groups: Vec<PaceEvidenceGroup>,
    pub unscorable_reasons: Vec<PaceEvidenceReason>,
}
pub struct PaceEvidenceAccumulator {
    summary: PaceEvidenceSummary,
    // The limit identity is a grouping key, never a user-visible label.
    groups: BTreeMap<(String, String, String, Option<u64>, bool), PaceEvidenceGroup>,
    reasons: BTreeMap<String, u32>,
}
fn include<T: Copy + PartialOrd>(range: &mut Option<[T; 2]>, value: T) {
    if let Some([lo, hi]) = range {
        if value < *lo {
            *lo = value;
        }
        if value > *hi {
            *hi = value;
        }
    } else {
        *range = Some([value, value]);
    }
}
impl PaceEvidenceAccumulator {
    pub fn new(now: &UtcTimestamp) -> Self {
        Self {
            summary: PaceEvidenceSummary {
                generated_at: now.clone(),
                oldest_issued_at: None,
                newest_issued_at: None,
                limited: false,
                counts: PaceEvidenceCounts::default(),
                groups: vec![],
                unscorable_reasons: vec![],
            },
            groups: BTreeMap::new(),
            reasons: BTreeMap::new(),
        }
    }
    pub fn add(&mut self, seed: &PaceTrialSeed, outcome: Option<&PaceOutcomeSummary>) {
        let unverified =
            seed.compatibility_unverified || outcome.is_some_and(|v| v.compatibility_unverified);
        let known = seed.algorithm_version == PACE_ALGORITHM_VERSION
            && seed.outcome_version == OUTCOME_VERSION
            && outcome.is_none_or(|v| v.verifier_version == OUTCOME_VERSION);
        let class = match outcome {
            None => "pending",
            Some(v)
                if known
                    && matches!(
                        v.classification.as_str(),
                        "within_band" | "below_band" | "above_band"
                    ) =>
            {
                v.classification.as_str()
            }
            Some(_) => "unscorable",
        };
        if class == "unscorable" {
            let reason = if known && outcome.is_some_and(|v| v.classification == "unscorable") {
                outcome.expect("terminal").reason_code.as_str()
            } else {
                "unsupported_algorithm"
            };
            *self.reasons.entry(reason.into()).or_default() += 1;
        }
        self.summary.counts.add(class);
        if self
            .summary
            .oldest_issued_at
            .as_ref()
            .is_none_or(|v| epoch(v) > epoch(&seed.issued_at))
        {
            self.summary.oldest_issued_at = Some(seed.issued_at.clone());
        }
        if self
            .summary
            .newest_issued_at
            .as_ref()
            .is_none_or(|v| epoch(v) < epoch(&seed.issued_at))
        {
            self.summary.newest_issued_at = Some(seed.issued_at.clone());
        }
        let verifier = outcome.map_or(seed.outcome_version.as_str(), |v| &v.verifier_version);
        let key = (
            seed.algorithm_version.clone(),
            verifier.to_owned(),
            seed.current.limit_id.clone(),
            seed.current.window_minutes,
            unverified,
        );
        let group = self.groups.entry(key).or_insert_with(|| PaceEvidenceGroup {
            algorithm_version: seed.algorithm_version.clone(),
            verifier_version: verifier.into(),
            window_minutes: seed.current.window_minutes,
            compatibility_unverified: unverified,
            counts: PaceEvidenceCounts::default(),
            training_maximum_gap_seconds: None,
            training_rate_ratio: None,
        });
        group.counts.add(class);
        if known {
            let mut times: Vec<_> = seed
                .samples
                .iter()
                .map(|v| epoch(&v.quota.captured_at))
                .collect();
            times.sort_unstable();
            times.dedup();
            if let Some(gap) = times
                .windows(2)
                .map(|p| ((p[1] - p[0] + 999) / 1000) as u64)
                .max()
            {
                include(&mut group.training_maximum_gap_seconds, gap);
            }
            let [low, high] = seed.rate_range;
            if low > 0.0 && high >= low && high.is_finite() && (high / low).is_finite() {
                include(&mut group.training_rate_ratio, high / low);
            }
        }
    }
    pub fn finish(mut self, limited: bool) -> PaceEvidenceSummary {
        self.summary.limited = limited;
        self.summary.groups = self.groups.into_values().collect();
        self.summary.unscorable_reasons = self
            .reasons
            .into_iter()
            .map(|(reason_code, count)| PaceEvidenceReason { reason_code, count })
            .collect();
        self.summary.unscorable_reasons.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(a.reason_code.cmp(&b.reason_code))
        });
        self.summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Availability, Compatibility, Freshness, pace::PaceCurrent};
    fn seed() -> PaceTrialSeed {
        let time = UtcTimestamp::parse("2026-09-26T06:00:00Z").unwrap();
        PaceTrialSeed {
            algorithm_version: PACE_ALGORITHM_VERSION.into(),
            outcome_version: OUTCOME_VERSION.into(),
            issued_at: time.clone(),
            observed_at: time.clone(),
            horizon: UtcTimestamp::parse("2026-09-26T12:00:00Z").unwrap(),
            current: PaceCurrent {
                limit_id: "codex:weekly".into(),
                window_minutes: Some(10080),
                remaining_percent: 50.0,
                resets_at: None,
                observed_at: time,
                account_plan_type: Some("pro".into()),
                availability: Availability::Complete,
                freshness: Freshness::Live,
                compatibility: Compatibility::Tested,
            },
            samples: vec![],
            balance_range: [20.0, 30.0],
            rate_range: [1.0, 2.0],
            assumptions: vec![],
            invalidation_factors: vec![],
            compatibility_unverified: false,
        }
    }
    fn outcome(class: &str) -> PaceOutcomeSummary {
        PaceOutcomeSummary {
            verifier_version: OUTCOME_VERSION.into(),
            assessed_at: UtcTimestamp::parse("2026-09-26T12:01:00Z").unwrap(),
            classification: class.into(),
            reason_code: "target_samples_missing".into(),
            observed_range: None,
            observed_from: None,
            observed_until: None,
            sample_count: 0,
            maximum_gap_seconds: 0,
            compatibility_unverified: false,
        }
    }
    #[test]
    fn counts_all_outcomes_without_scoring_pending_or_missing() {
        let seed = seed();
        let mut report = PaceEvidenceAccumulator::new(&seed.issued_at);
        report.add(&seed, None);
        for class in ["within_band", "below_band", "above_band", "unscorable"] {
            report.add(&seed, Some(&outcome(class)));
        }
        let result = report.finish(false);
        assert_eq!(
            (
                result.counts.total,
                result.counts.pending,
                result.counts.within,
                result.counts.below,
                result.counts.above,
                result.counts.unscorable
            ),
            (5, 1, 1, 1, 1, 1)
        );
        assert_eq!(result.unscorable_reasons[0].count, 1);
        assert_eq!(result.groups[0].training_rate_ratio, Some([2.0, 2.0]));
        assert!(result.groups[0].training_maximum_gap_seconds.is_none());
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("validated")
        );
    }
    #[test]
    fn separates_versions_windows_and_compatibility_and_rejects_unknown_success() {
        let mut seed = seed();
        let mut report = PaceEvidenceAccumulator::new(&seed.issued_at);
        report.add(&seed, Some(&outcome("within_band")));
        seed.compatibility_unverified = true;
        report.add(&seed, Some(&outcome("below_band")));
        seed.current.window_minutes = Some(300);
        report.add(&seed, None);
        seed.algorithm_version = "future-version".into();
        report.add(&seed, Some(&outcome("within_band")));
        let result = report.finish(true);
        assert!(result.limited);
        assert_eq!(result.groups.len(), 4);
        assert_eq!(result.counts.within, 1);
        assert_eq!(result.counts.unscorable, 1);
        assert_eq!(
            result.unscorable_reasons[0].reason_code,
            "unsupported_algorithm"
        );
    }
    #[test]
    fn empty_report_has_no_dates_or_invented_zero_percentage() {
        let result = PaceEvidenceAccumulator::new(&seed().issued_at).finish(false);
        assert_eq!(result.counts.total, 0);
        assert!(result.oldest_issued_at.is_none());
        assert!(result.newest_issued_at.is_none());
        assert!(result.groups.is_empty());
    }
}
