//! Explicit, user-confirmed activity; a timer is only an unverified draft.
use crate::UtcTimestamp;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_ACTIVITY_SECONDS: u32 = 86_400;
pub const MAX_ACTIVITY_SPAN_SECONDS: i64 = 7 * 86_400;

#[derive(Debug, Error, PartialEq)]
pub enum ActiveTimeError {
    #[error("invalid activity interval or duration")]
    InvalidObservation,
    #[error("invalid timer transition")]
    InvalidTransition,
}

pub fn seconds_between(start: &UtcTimestamp, end: &UtcTimestamp) -> i64 {
    DateTime::parse_from_rfc3339(end.as_str())
        .expect("validated UTC")
        .signed_duration_since(DateTime::parse_from_rfc3339(start.as_str()).expect("validated UTC"))
        .num_seconds()
}

pub fn whole_second(value: &UtcTimestamp) -> UtcTimestamp {
    UtcTimestamp::parse(
        DateTime::parse_from_rfc3339(value.as_str())
            .expect("validated UTC")
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
    .expect("UTC output")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveTimeInput {
    pub started_at: UtcTimestamp,
    pub ended_at: UtcTimestamp,
    pub duration_seconds: u32,
}

impl ActiveTimeInput {
    pub fn validated(&self, now: &UtcTimestamp) -> Result<Self, ActiveTimeError> {
        let value = Self {
            started_at: whole_second(&self.started_at),
            ended_at: whole_second(&self.ended_at),
            duration_seconds: self.duration_seconds,
        };
        let span = seconds_between(&value.started_at, &value.ended_at);
        if !(1..=MAX_ACTIVITY_SPAN_SECONDS).contains(&span)
            || !(1..=MAX_ACTIVITY_SECONDS).contains(&value.duration_seconds)
            || i64::from(value.duration_seconds) > span
            || !now.is_not_before(&self.ended_at)
        {
            return Err(ActiveTimeError::InvalidObservation);
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveTimerState {
    Running,
    Paused,
    Review,
    Saved,
    Discarded,
}

impl ActiveTimerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Review => "review",
            Self::Saved => "saved",
            Self::Discarded => "discarded",
        }
    }
    pub fn is_open(self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::Review)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTimer {
    pub timer_id: String,
    pub revision: u32,
    pub state: ActiveTimerState,
    pub started_at: UtcTimestamp,
    pub ended_at: Option<UtcTimestamp>,
    /// Wall-clock suggestion minus explicit pauses. Never measured activity.
    pub suggested_seconds: u32,
    pub updated_at: UtcTimestamp,
    pub interrupted: bool,
}

impl ActiveTimer {
    pub fn start(id: String, now: &UtcTimestamp) -> Self {
        let now = whole_second(now);
        Self {
            timer_id: id,
            revision: 1,
            state: ActiveTimerState::Running,
            started_at: now.clone(),
            ended_at: None,
            suggested_seconds: 0,
            updated_at: now,
            interrupted: false,
        }
    }
    /// A restart/scope change keeps only the last persisted checkpoint. A clock
    /// regression or excessive duration is also review-only, never auto-resumed.
    pub fn checkpoint(&mut self, now: &UtcTimestamp, same_context: bool) -> bool {
        if !matches!(
            self.state,
            ActiveTimerState::Running | ActiveTimerState::Paused
        ) {
            return false;
        }
        let now = whole_second(now);
        let elapsed = seconds_between(&self.updated_at, &now);
        let suggested = i64::from(self.suggested_seconds)
            + if self.state == ActiveTimerState::Running {
                elapsed
            } else {
                0
            };
        if !same_context
            || elapsed < 0
            || suggested > i64::from(MAX_ACTIVITY_SECONDS)
            || seconds_between(&self.started_at, &now) > MAX_ACTIVITY_SPAN_SECONDS
        {
            self.state = ActiveTimerState::Review;
            self.ended_at = Some(self.updated_at.clone());
            self.interrupted = true;
            self.revision += 1;
            return true;
        }
        self.suggested_seconds = suggested as u32;
        self.updated_at = now;
        false
    }
    pub fn transition(
        &mut self,
        action: TimerTransition,
        now: &UtcTimestamp,
    ) -> Result<(), ActiveTimeError> {
        let next = match (self.state, action) {
            (ActiveTimerState::Running, TimerTransition::Pause) => ActiveTimerState::Paused,
            (ActiveTimerState::Paused, TimerTransition::Resume) => ActiveTimerState::Running,
            (ActiveTimerState::Running | ActiveTimerState::Paused, TimerTransition::Finish) => {
                ActiveTimerState::Review
            }
            (state, TimerTransition::Discard) if state.is_open() => ActiveTimerState::Discarded,
            _ => return Err(ActiveTimeError::InvalidTransition),
        };
        self.state = next;
        self.revision += 1;
        self.updated_at = whole_second(now);
        if matches!(next, ActiveTimerState::Review | ActiveTimerState::Discarded) {
            self.ended_at = Some(self.updated_at.clone());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TimerTransition {
    Pause,
    Resume,
    Finish,
    Discard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTimeObservation {
    pub observation_id: String,
    pub source: &'static str,
    pub quality: &'static str,
    pub observation: ActiveTimeInput,
    pub included: bool,
    pub revision: u32,
    pub created_at: UtcTimestamp,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ActiveTimeAction {
    Start {
        timer_id: String,
    },
    Pause {
        timer_id: String,
        expected_revision: u32,
    },
    Resume {
        timer_id: String,
        expected_revision: u32,
    },
    Finish {
        timer_id: String,
        expected_revision: u32,
    },
    Discard {
        timer_id: String,
        expected_revision: u32,
    },
    Confirm {
        timer_id: String,
        expected_revision: u32,
        observation: ActiveTimeInput,
    },
    Record {
        observation_id: String,
        observation: ActiveTimeInput,
    },
    SetIncluded {
        observation_id: String,
        expected_revision: u32,
        included: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(time: &str) -> UtcTimestamp {
        UtcTimestamp::parse(format!("2026-09-26T{time}Z")).unwrap()
    }
    #[test]
    fn explicit_pauses_are_not_in_the_suggestion_and_finish_still_needs_review() {
        let mut t = ActiveTimer::start("draft".into(), &at("02:00:00"));
        t.checkpoint(&at("02:10:00"), true);
        t.transition(TimerTransition::Pause, &at("02:10:00"))
            .unwrap();
        t.checkpoint(&at("03:00:00"), true);
        t.transition(TimerTransition::Resume, &at("03:00:00"))
            .unwrap();
        t.checkpoint(&at("03:05:00"), true);
        t.transition(TimerTransition::Finish, &at("03:05:00"))
            .unwrap();
        assert_eq!(t.suggested_seconds, 900);
        assert_eq!(t.state, ActiveTimerState::Review);
        assert!(
            t.transition(TimerTransition::Resume, &at("03:06:00"))
                .is_err()
        );
    }
    #[test]
    fn restart_and_clock_regression_preserve_only_the_checkpoint() {
        for (now, same) in [(at("04:00:00"), false), (at("01:00:00"), true)] {
            let mut t = ActiveTimer::start("draft".into(), &at("02:00:00"));
            t.checkpoint(&at("02:10:00"), true);
            assert!(t.checkpoint(&now, same));
            assert_eq!(t.suggested_seconds, 600);
            assert_eq!(t.ended_at, Some(at("02:10:00")));
            assert!(t.interrupted);
        }
    }
    #[test]
    fn duration_and_dates_are_validated_and_utc_is_normalized() {
        let mut v = ActiveTimeInput {
            started_at: at("02:00:00"),
            ended_at: at("03:00:00"),
            duration_seconds: 1800,
        };
        assert!(v.validated(&at("03:00:00")).is_ok());
        assert!(v.validated(&at("02:59:00")).is_err());
        for duration in [0, 3601, MAX_ACTIVITY_SECONDS + 1] {
            v.duration_seconds = duration;
            assert!(v.validated(&at("03:00:00")).is_err());
        }
        assert_eq!(
            whole_second(&UtcTimestamp::parse("2026-09-26T02:00:00.999+00:00").unwrap()),
            at("02:00:00")
        );
    }
}
