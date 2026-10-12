use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;

/// Source of the current time; injected so schedules are testable without sleeping.
pub trait Clock: Send + Sync + 'static {
    /// Current UTC time.
    fn now(&self) -> DateTime<Utc>;
}

/// Wall-clock time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        // Stored timestamps have millisecond precision; truncating here keeps every value
        // returned to a caller identical to what a later read of the store yields.
        DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).unwrap_or_else(Utc::now)
    }
}

/// Manually advanced clock for deterministic tests.
#[derive(Debug)]
pub struct ManualClock(Mutex<DateTime<Utc>>);

impl ManualClock {
    /// Start at a fixed instant.
    pub fn new(start: DateTime<Utc>) -> Self {
        Self(Mutex::new(start))
    }

    /// Move time forward.
    pub fn advance(&self, by: Duration) {
        *self.0.lock() += by;
    }

    /// Jump to an absolute instant.
    pub fn set(&self, to: DateTime<Utc>) {
        *self.0.lock() = to;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock()
    }
}
