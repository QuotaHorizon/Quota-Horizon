use chrono::{DateTime, Duration, SecondsFormat, Timelike, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::UtcTimestamp;

pub const MAX_WORK_SCHEDULE_PERIODS: usize = 16;
pub const DEFAULT_PACE_GUARD_PERCENT: f64 = 5.0;
const MINUTES_PER_DAY: u16 = 1_440;
const TARGET_SCAN_MINUTES: usize = 2_882;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkSchedulePeriod {
    pub start_minute_of_day: u16,
    pub end_minute_of_day: u16,
}

impl WorkSchedulePeriod {
    pub fn new(start_minute_of_day: u16, end_minute_of_day: u16) -> Result<Self, PlannerError> {
        let period = Self {
            start_minute_of_day,
            end_minute_of_day,
        };
        period.validate()?;
        Ok(period)
    }

    pub fn validate(&self) -> Result<(), PlannerError> {
        if self.start_minute_of_day >= MINUTES_PER_DAY || self.end_minute_of_day >= MINUTES_PER_DAY
        {
            return Err(PlannerError::InvalidMinuteOfDay);
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.start_minute_of_day == self.end_minute_of_day
    }

    pub fn contains(&self, minute_of_day: u16) -> bool {
        if self.is_empty() || minute_of_day >= MINUTES_PER_DAY {
            return false;
        }
        if self.start_minute_of_day < self.end_minute_of_day {
            minute_of_day >= self.start_minute_of_day && minute_of_day < self.end_minute_of_day
        } else {
            minute_of_day >= self.start_minute_of_day || minute_of_day < self.end_minute_of_day
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkSchedule {
    pub enabled: bool,
    pub off_periods: Vec<WorkSchedulePeriod>,
}

impl WorkSchedule {
    pub fn new(enabled: bool, off_periods: Vec<WorkSchedulePeriod>) -> Result<Self, PlannerError> {
        let mut schedule = Self {
            enabled,
            off_periods,
        };
        schedule.normalize()?;
        Ok(schedule)
    }

    pub fn validate(&self) -> Result<(), PlannerError> {
        if self.off_periods.len() > MAX_WORK_SCHEDULE_PERIODS {
            return Err(PlannerError::TooManyPeriods);
        }
        self.off_periods
            .iter()
            .try_for_each(WorkSchedulePeriod::validate)
    }

    pub fn normalized(mut self) -> Result<Self, PlannerError> {
        self.normalize()?;
        Ok(self)
    }

    pub fn available_minutes_per_day(&self) -> u16 {
        let available = (0..MINUTES_PER_DAY)
            .filter(|minute| !self.raw_is_off(*minute))
            .count();
        if available == 0 {
            MINUTES_PER_DAY
        } else {
            u16::try_from(available).unwrap_or(MINUTES_PER_DAY)
        }
    }

    pub fn is_off(&self, minute_of_day: u16) -> bool {
        self.available_minutes_per_day() != MINUTES_PER_DAY && self.raw_is_off(minute_of_day)
    }

    fn normalize(&mut self) -> Result<(), PlannerError> {
        self.validate()?;
        self.off_periods.retain(|period| !period.is_empty());
        self.off_periods
            .sort_by_key(|period| (period.start_minute_of_day, period.end_minute_of_day));
        self.off_periods.dedup();
        Ok(())
    }

    fn raw_is_off(&self, minute_of_day: u16) -> bool {
        self.off_periods
            .iter()
            .any(|period| period.contains(minute_of_day))
    }

    fn has_effective_off_periods(&self) -> bool {
        self.enabled
            && !self.off_periods.is_empty()
            && self.available_minutes_per_day() < MINUTES_PER_DAY
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkSegmentState {
    Working,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaceState {
    OnPace,
    WithinGuard,
    OverGuard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SchedulePaceComparison {
    pub enabled: bool,
    pub segment: WorkSegmentState,
    pub target_at: Option<UtcTimestamp>,
    pub expected_remaining_percent: f64,
    pub actual_remaining_percent: f64,
    pub overspend_percent: f64,
    pub state: PaceState,
    pub daily_budget_percent: f64,
    pub usable_minutes_per_day: u16,
    pub off_minutes_per_day: u16,
    pub window_expired: bool,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PlannerError {
    #[error("work schedule minute must be in 0...1439")]
    InvalidMinuteOfDay,
    #[error("work schedule has too many periods")]
    TooManyPeriods,
    #[error("planner timestamp is invalid")]
    InvalidTimestamp,
    #[error("quota window is invalid")]
    InvalidWindow,
}

/// Evaluate the deterministic schedule baseline for one quota window.
///
/// `local_minute` is supplied by the platform boundary. Production uses the
/// OS local timezone; tests can inject fixed offsets and DST transitions. The
/// core therefore remains deterministic without hard-coding a timezone.
pub fn evaluate_schedule_pace<F>(
    window_duration_minutes: u64,
    resets_at: &UtcTimestamp,
    actual_remaining_percent: f64,
    now: &UtcTimestamp,
    schedule: &WorkSchedule,
    local_minute: F,
) -> Result<SchedulePaceComparison, PlannerError>
where
    F: Fn(&DateTime<Utc>) -> u16,
{
    schedule.validate()?;
    if window_duration_minutes == 0
        || window_duration_minutes > u64::try_from(i64::MAX / 60).unwrap_or(u64::MAX)
        || !actual_remaining_percent.is_finite()
    {
        return Err(PlannerError::InvalidWindow);
    }
    let reset = parse_utc(resets_at)?;
    let now = parse_utc(now)?;
    let duration_minutes =
        i64::try_from(window_duration_minutes).map_err(|_| PlannerError::InvalidWindow)?;
    let start = reset
        .checked_sub_signed(Duration::minutes(duration_minutes))
        .ok_or(PlannerError::InvalidWindow)?;
    let actual = actual_remaining_percent.clamp(0.0, 100.0);
    let usable = schedule.available_minutes_per_day();
    let off = MINUTES_PER_DAY.saturating_sub(usable);
    let daily_budget =
        (100.0 * f64::from(MINUTES_PER_DAY) / window_duration_minutes as f64).clamp(0.0, 100.0);
    let segment = if schedule.has_effective_off_periods()
        && schedule.is_off(valid_local_minute(&local_minute, &now)?)
    {
        WorkSegmentState::Off
    } else {
        WorkSegmentState::Working
    };

    if now > reset {
        return Ok(SchedulePaceComparison {
            enabled: schedule.enabled,
            segment,
            target_at: None,
            expected_remaining_percent: 100.0,
            actual_remaining_percent: actual,
            overspend_percent: 0.0,
            state: PaceState::OnPace,
            daily_budget_percent: daily_budget,
            usable_minutes_per_day: usable,
            off_minutes_per_day: off,
            window_expired: true,
        });
    }

    let target = if schedule.has_effective_off_periods() {
        match segment {
            WorkSegmentState::Off => current_off_period_start(now, schedule, &local_minute)?,
            WorkSegmentState::Working => next_off_period_start(now, schedule, &local_minute)?,
        }
    } else {
        None
    };
    let evaluation_point = target.unwrap_or(now).clamp(start, reset);
    let total_weight = if schedule.has_effective_off_periods() {
        weighted_seconds(start, reset, schedule, &local_minute)?
    } else {
        (reset - start).num_milliseconds() as f64 / 1_000.0
    };
    if total_weight <= 0.0 {
        return Err(PlannerError::InvalidWindow);
    }
    let elapsed_weight = if schedule.has_effective_off_periods() {
        weighted_seconds(start, evaluation_point, schedule, &local_minute)?
    } else {
        (evaluation_point - start).num_milliseconds().max(0) as f64 / 1_000.0
    };
    let expected = (100.0 - elapsed_weight / total_weight * 100.0).clamp(0.0, 100.0);
    let overspend = (expected - actual).max(0.0);
    let state = if overspend <= 0.0 {
        PaceState::OnPace
    } else if overspend <= DEFAULT_PACE_GUARD_PERCENT {
        PaceState::WithinGuard
    } else {
        PaceState::OverGuard
    };

    Ok(SchedulePaceComparison {
        enabled: schedule.enabled,
        segment,
        target_at: target.map(timestamp),
        expected_remaining_percent: expected,
        actual_remaining_percent: actual,
        overspend_percent: overspend,
        state,
        daily_budget_percent: daily_budget,
        usable_minutes_per_day: usable,
        off_minutes_per_day: off,
        window_expired: false,
    })
}

fn current_off_period_start<F>(
    now: DateTime<Utc>,
    schedule: &WorkSchedule,
    local_minute: &F,
) -> Result<Option<DateTime<Utc>>, PlannerError>
where
    F: Fn(&DateTime<Utc>) -> u16,
{
    let mut cursor = floor_to_minute(now);
    for _ in 0..TARGET_SCAN_MINUTES {
        let previous = cursor - Duration::minutes(1);
        if !schedule.raw_is_off(valid_local_minute(local_minute, &previous)?) {
            return Ok(Some(cursor));
        }
        cursor = previous;
    }
    Ok(None)
}

fn next_off_period_start<F>(
    now: DateTime<Utc>,
    schedule: &WorkSchedule,
    local_minute: &F,
) -> Result<Option<DateTime<Utc>>, PlannerError>
where
    F: Fn(&DateTime<Utc>) -> u16,
{
    let mut cursor = floor_to_minute(now);
    for _ in 0..TARGET_SCAN_MINUTES {
        cursor += Duration::minutes(1);
        if schedule.raw_is_off(valid_local_minute(local_minute, &cursor)?) {
            return current_off_period_start(cursor, schedule, local_minute);
        }
    }
    Ok(None)
}

fn weighted_seconds<F>(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    schedule: &WorkSchedule,
    local_minute: &F,
) -> Result<f64, PlannerError>
where
    F: Fn(&DateTime<Utc>) -> u16,
{
    if end <= start {
        return Ok(0.0);
    }
    let mut cursor = start;
    let mut total = 0.0;
    while cursor < end {
        let segment_end = (floor_to_minute(cursor) + Duration::minutes(1)).min(end);
        if !schedule.raw_is_off(valid_local_minute(local_minute, &cursor)?) {
            total += (segment_end - cursor).num_milliseconds().max(0) as f64 / 1_000.0;
        }
        cursor = segment_end;
    }
    Ok(total)
}

fn valid_local_minute<F>(resolver: &F, timestamp: &DateTime<Utc>) -> Result<u16, PlannerError>
where
    F: Fn(&DateTime<Utc>) -> u16,
{
    let minute = resolver(timestamp);
    if minute < MINUTES_PER_DAY {
        Ok(minute)
    } else {
        Err(PlannerError::InvalidMinuteOfDay)
    }
}

fn parse_utc(timestamp: &UtcTimestamp) -> Result<DateTime<Utc>, PlannerError> {
    DateTime::parse_from_rfc3339(timestamp.as_str())
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| PlannerError::InvalidTimestamp)
}

fn timestamp(value: DateTime<Utc>) -> UtcTimestamp {
    UtcTimestamp::parse(value.to_rfc3339_opts(SecondsFormat::Millis, true))
        .expect("UTC DateTime always serializes as a valid UTC timestamp")
}

fn floor_to_minute(value: DateTime<Utc>) -> DateTime<Utc> {
    value
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).unwrap()
    }

    fn fixed_offset(hours: i64) -> impl Fn(&DateTime<Utc>) -> u16 {
        move |value| {
            let local = *value + Duration::hours(hours);
            u16::try_from(local.hour() * 60 + local.minute()).unwrap()
        }
    }

    fn period(start: u16, end: u16) -> WorkSchedulePeriod {
        WorkSchedulePeriod::new(start, end).unwrap()
    }

    #[test]
    fn cross_midnight_and_overlapping_periods_have_union_semantics() {
        let schedule = WorkSchedule::new(true, vec![period(1_380, 120), period(60, 180)]).unwrap();
        assert!(schedule.is_off(1_400));
        assert!(schedule.is_off(90));
        assert!(!schedule.is_off(600));
        assert_eq!(schedule.available_minutes_per_day(), 1_200);
    }

    #[test]
    fn all_day_off_falls_back_to_all_day_usable() {
        let schedule = WorkSchedule::new(true, vec![period(0, 720), period(720, 0)]).unwrap();
        assert_eq!(schedule.available_minutes_per_day(), 1_440);
        assert!(!schedule.is_off(60));
    }

    #[test]
    fn disabled_schedule_uses_live_elapsed_pace() {
        let comparison = evaluate_schedule_pace(
            1_440,
            &utc("2026-01-02T00:00:00Z"),
            47.0,
            &utc("2026-01-01T12:00:00Z"),
            &WorkSchedule::default(),
            fixed_offset(0),
        )
        .unwrap();
        assert!((comparison.expected_remaining_percent - 50.0).abs() < 0.001);
        assert_eq!(comparison.state, PaceState::WithinGuard);
        assert_eq!(comparison.target_at, None);
    }

    #[test]
    fn off_time_freezes_at_current_stop_and_work_time_uses_next_stop() {
        let schedule = WorkSchedule::new(true, vec![period(0, 720)]).unwrap();
        let during_off = evaluate_schedule_pace(
            1_440,
            &utc("2026-01-02T00:00:00Z"),
            100.0,
            &utc("2026-01-01T06:00:00Z"),
            &schedule,
            fixed_offset(0),
        )
        .unwrap();
        assert!((during_off.expected_remaining_percent - 100.0).abs() < 0.001);
        assert_eq!(during_off.segment, WorkSegmentState::Off);
        assert_eq!(during_off.target_at, Some(utc("2026-01-01T00:00:00.000Z")));

        let during_work = evaluate_schedule_pace(
            1_440,
            &utc("2026-01-02T00:00:00Z"),
            0.0,
            &utc("2026-01-01T18:00:00Z"),
            &schedule,
            fixed_offset(0),
        )
        .unwrap();
        assert!(during_work.expected_remaining_percent.abs() < 0.001);
        assert_eq!(during_work.target_at, Some(utc("2026-01-02T00:00:00.000Z")));
    }

    #[test]
    fn weekly_target_preserves_one_usable_hour_before_reset() {
        let schedule = WorkSchedule::new(true, vec![period(120, 600)]).unwrap();
        let comparison = evaluate_schedule_pace(
            10_080,
            &utc("2026-06-10T03:00:00Z"),
            10.0,
            &utc("2026-06-09T04:00:00Z"),
            &schedule,
            fixed_offset(8),
        )
        .unwrap();
        let expected = 100.0 / (7.0 * 16.0);
        assert!((comparison.expected_remaining_percent - expected).abs() < 0.001);
        assert_eq!(comparison.usable_minutes_per_day, 960);
        assert!((comparison.daily_budget_percent - 100.0 / 7.0).abs() < 0.001);
    }

    #[test]
    fn twenty_three_hour_off_period_only_consumes_in_usable_hour() {
        let schedule = WorkSchedule::new(true, vec![period(660, 600)]).unwrap();
        let comparison = evaluate_schedule_pace(
            10_080,
            &utc("2026-01-08T10:00:00Z"),
            50.0,
            &utc("2026-01-05T11:00:00Z"),
            &schedule,
            fixed_offset(0),
        )
        .unwrap();
        assert_eq!(comparison.usable_minutes_per_day, 60);
        assert!((comparison.expected_remaining_percent - (2.0 / 7.0 * 100.0)).abs() < 0.001);
    }

    #[test]
    fn utc_minute_iteration_respects_a_spring_forward_gap() {
        let schedule = WorkSchedule::new(true, vec![period(90, 210)]).unwrap();
        let start = DateTime::parse_from_rfc3339("2026-03-29T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let end = start + Duration::hours(4);
        let resolver = |value: &DateTime<Utc>| {
            let offset = if *value < start + Duration::hours(1) {
                1
            } else {
                2
            };
            let local = *value + Duration::hours(offset);
            u16::try_from(local.hour() * 60 + local.minute()).unwrap()
        };
        let usable = weighted_seconds(start, end, &schedule, &resolver).unwrap();
        assert!((usable - 3.0 * 3_600.0).abs() < 0.001);
    }

    #[test]
    fn utc_minute_iteration_respects_a_fall_back_fold() {
        let schedule = WorkSchedule::new(true, vec![period(90, 210)]).unwrap();
        let start = DateTime::parse_from_rfc3339("2026-10-25T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let end = start + Duration::hours(4);
        let resolver = |value: &DateTime<Utc>| {
            let offset = if *value < start + Duration::hours(1) {
                2
            } else {
                1
            };
            let local = *value + Duration::hours(offset);
            u16::try_from(local.hour() * 60 + local.minute()).unwrap()
        };
        let usable = weighted_seconds(start, end, &schedule, &resolver).unwrap();
        assert!((usable - 1.5 * 3_600.0).abs() < 0.001);
    }
}
