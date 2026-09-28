//! Explicit user intent and arithmetic allowances, not consumption forecasts.
use crate::UtcTimestamp;
use serde::{Deserialize, Serialize};
use thiserror::Error;

fn elapsed(later: &UtcTimestamp, earlier: &UtcTimestamp) -> Option<i64> {
    let later = chrono::DateTime::parse_from_rfc3339(later.as_str()).ok()?;
    let earlier = chrono::DateTime::parse_from_rfc3339(earlier.as_str()).ok()?;
    Some((later - earlier).num_seconds())
}

/// This first slice deliberately does not guess a definition of a heavy day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemandKind {
    ActiveHours,
    MaintainRecentPace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapacityDemandInput {
    pub horizon_end: UtcTimestamp,
    pub demand_kind: DemandKind,
    pub planned_codex_active_hours: Option<f64>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DemandError {
    #[error("the plan deadline must be in the next 366 days")]
    InvalidHorizon,
    #[error("active hours must be finite, positive and fit before the deadline")]
    InvalidHours,
}

impl CapacityDemandInput {
    pub fn validate(&self, now: &UtcTimestamp) -> Result<(), DemandError> {
        let seconds = elapsed(&self.horizon_end, now).ok_or(DemandError::InvalidHorizon)?;
        if seconds <= 0 || seconds > 366 * 86_400 {
            return Err(DemandError::InvalidHorizon);
        }
        match (self.demand_kind, self.planned_codex_active_hours) {
            (DemandKind::MaintainRecentPace, None) => Ok(()),
            (DemandKind::ActiveHours, Some(hours))
                if hours.is_finite() && hours > 0.0 && hours * 3600.0 <= seconds as f64 =>
            {
                Ok(())
            }
            _ => Err(DemandError::InvalidHours),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemandAllowance {
    pub method: &'static str,
    pub reason_code: &'static str,
    /// Percentage points per 24 elapsed hours, not per working day.
    pub daily_percent: Option<f64>,
    /// An upper allocation per planned hour, never measured consumption.
    pub per_planned_hour_percent: Option<f64>,
}

/// No extrapolation across a reset, no added cards/public-reset probability,
/// no plan-fit decision. The caller must independently expose source freshness.
pub fn demand_allowance(
    demand: &CapacityDemandInput,
    remaining: f64,
    resets_at: Option<&UtcTimestamp>,
    now: &UtcTimestamp,
) -> DemandAllowance {
    let mut result = DemandAllowance {
        method: "equal_allocation_v1",
        reason_code: "allowance_available",
        daily_percent: None,
        per_planned_hour_percent: None,
    };
    let seconds = elapsed(&demand.horizon_end, now).unwrap_or(0);
    result.reason_code = if seconds <= 0 {
        "plan_ended"
    } else if !remaining.is_finite() || !(0.0..=100.0).contains(&remaining) {
        "quota_unavailable"
    } else if resets_at.is_none() {
        "reset_unknown"
    } else if resets_at.is_some_and(|reset| elapsed(reset, now).is_none_or(|seconds| seconds <= 0))
    {
        "window_ended"
    } else if resets_at.is_some_and(|reset| !reset.is_not_before(&demand.horizon_end)) {
        "reset_before_deadline"
    } else {
        // A sub-day plan still cannot allocate more than its total balance.
        result.daily_percent = Some(remaining / (seconds as f64 / 86_400.0).max(1.0));
        result.per_planned_hour_percent = match demand.planned_codex_active_hours {
            Some(hours)
                if demand.demand_kind == DemandKind::ActiveHours
                    && hours.is_finite()
                    && hours > 0.0 =>
            {
                Some(remaining / hours)
            }
            _ => None,
        };
        "allowance_available"
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).unwrap()
    }
    fn demand() -> CapacityDemandInput {
        CapacityDemandInput {
            horizon_end: at("2026-09-28T04:00:00Z"),
            demand_kind: DemandKind::ActiveHours,
            planned_codex_active_hours: Some(8.0),
        }
    }
    #[test]
    fn validates_explicit_demand_not_elapsed_active_time() {
        let now = at("2026-09-26T04:00:00Z");
        let mut input = demand();
        assert!(input.validate(&now).is_ok());
        for hours in [0.0, -1.0, f64::NAN, f64::INFINITY, 49.0] {
            input.planned_codex_active_hours = Some(hours);
            assert_eq!(input.validate(&now), Err(DemandError::InvalidHours));
        }
        input.demand_kind = DemandKind::MaintainRecentPace;
        input.planned_codex_active_hours = None;
        assert!(input.validate(&now).is_ok());
        assert_eq!(
            input.validate(&input.horizon_end),
            Err(DemandError::InvalidHorizon)
        );
        assert_eq!(
            input.validate(&at("2025-01-01T00:00:00Z")),
            Err(DemandError::InvalidHorizon)
        );
    }
    #[test]
    fn divides_current_balance_without_claiming_fit_or_adding_a_reset() {
        let now = at("2026-09-26T04:00:00Z");
        let input = demand();
        let value = demand_allowance(&input, 40.0, Some(&input.horizon_end), &now);
        assert_eq!(value.daily_percent, Some(20.0));
        assert_eq!(value.per_planned_hour_percent, Some(5.0));
        for (reset, reason) in [
            (None, "reset_unknown"),
            (Some(now.clone()), "window_ended"),
            (Some(at("2026-09-27T04:00:00Z")), "reset_before_deadline"),
        ] {
            let result = demand_allowance(&input, 40.0, reset.as_ref(), &now);
            assert_eq!(result.reason_code, reason);
            assert_eq!(result.daily_percent, None);
        }
        assert_eq!(
            demand_allowance(&input, 0.0, Some(&input.horizon_end), &now).daily_percent,
            Some(0.0)
        );
        assert_eq!(
            demand_allowance(&input, f64::NAN, Some(&input.horizon_end), &now).daily_percent,
            None
        );
    }
    #[test]
    fn subday_budget_is_capped_and_dst_is_elapsed_utc_time() {
        let mut input = demand();
        input.horizon_end = at("2026-03-09T04:00:00Z");
        input.planned_codex_active_hours = Some(4.0);
        let now = at("2026-03-08T05:00:00Z"); // 23-hour local day in New York.
        assert_eq!(
            demand_allowance(&input, 30.0, Some(&input.horizon_end), &now).daily_percent,
            Some(30.0)
        );
        assert_eq!(
            demand_allowance(&input, 30.0, Some(&input.horizon_end), &input.horizon_end)
                .reason_code,
            "plan_ended"
        );
    }
}
