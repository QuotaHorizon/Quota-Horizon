use chrono::{SecondsFormat, Utc};

use crate::UtcTimestamp;

pub trait Clock: Send + Sync {
    fn now(&self) -> UtcTimestamp;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UtcTimestamp {
        UtcTimestamp::new_unchecked(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

#[derive(Debug, Clone)]
pub struct FixedClock {
    now: UtcTimestamp,
}

impl FixedClock {
    pub fn new(now: UtcTimestamp) -> Self {
        Self { now }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> UtcTimestamp {
        self.now.clone()
    }
}
