//! Occurrence arithmetic for cron (with time zones) and interval schedules.

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use croner::{
    parser::{CronParser, Seconds, Year},
    Cron,
};
use openside_proto::RoutineSchedule;

use crate::Invalid;

/// Longest accepted interval (ten years); keeps chrono arithmetic far from overflow.
pub const MAX_INTERVAL_SECS: u64 = 315_360_000;

/// Five fields exactly (`minute hour day-of-month month day-of-week`) or an `@` nickname.
/// croner's default also takes a leading seconds field and a trailing year; the public
/// contract has neither, so both are disallowed. Weekdays are POSIX (0 and 7 are Sunday).
static PARSER: std::sync::LazyLock<CronParser> = std::sync::LazyLock::new(|| {
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
});

enum Compiled {
    Cron { cron: Box<Cron>, tz: Tz },
    Interval { secs: i64 },
}

fn compile(schedule: &RoutineSchedule) -> Result<Compiled, Invalid> {
    match schedule {
        RoutineSchedule::Cron { expr, timezone } => {
            let tz = match timezone.as_deref().map(str::trim) {
                None | Some("") => Tz::UTC,
                Some(name) => Tz::from_str(name).map_err(|_| Invalid::Timezone(name.to_owned()))?,
            };
            let cron = PARSER
                .parse(expr.trim())
                .map_err(|error| Invalid::Schedule(format!("invalid cron expression: {error}")))?;
            Ok(Compiled::Cron {
                cron: Box::new(cron),
                tz,
            })
        }
        RoutineSchedule::Interval { secs } => {
            if *secs == 0 || *secs > MAX_INTERVAL_SECS {
                return Err(Invalid::Schedule(format!(
                    "interval must be between 1 and {MAX_INTERVAL_SECS} seconds"
                )));
            }
            Ok(Compiled::Interval { secs: *secs as i64 })
        }
    }
}

/// Reject schedules that could never fire or that exceed supported ranges. `now` is the
/// caller's clock, so validation never depends on wall time.
pub fn validate(schedule: &RoutineSchedule, now: DateTime<Utc>) -> Result<(), Invalid> {
    let compiled = compile(schedule)?;
    if let Compiled::Cron { .. } = compiled {
        // A pattern such as "0 0 30 2 *" parses but never occurs.
        next_after(schedule, now, now)?
            .ok_or_else(|| Invalid::Schedule("cron expression never fires".into()))?;
    }
    Ok(())
}

fn cron_next(cron: &Cron, tz: Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    cron.find_next_occurrence(&after.with_timezone(&tz), false)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn cron_latest(cron: &Cron, tz: Tz, through: DateTime<Utc>) -> Option<DateTime<Utc>> {
    cron.find_previous_occurrence(&through.with_timezone(&tz), true)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

/// First occurrence strictly after `after`; interval schedules are anchored at `anchor`.
pub fn next_after(
    schedule: &RoutineSchedule,
    after: DateTime<Utc>,
    anchor: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Invalid> {
    Ok(match compile(schedule)? {
        Compiled::Cron { cron, tz } => cron_next(&cron, tz, after),
        Compiled::Interval { secs } => {
            let elapsed = (after - anchor).num_seconds();
            let periods = if elapsed < 0 { 0 } else { elapsed / secs + 1 };
            Some(anchor + Duration::seconds(periods * secs))
        }
    })
}

/// Occurrences that came due between `first` (inclusive) and `now` (inclusive).
pub struct Due {
    /// The oldest occurrences, at most `cap` of them.
    pub listed: Vec<DateTime<Utc>>,
    /// More than `cap` occurrences were due; `listed` is only a prefix.
    pub truncated: bool,
    /// The most recent due occurrence.
    pub latest: DateTime<Utc>,
    /// First occurrence after `now`.
    pub next: Option<DateTime<Utc>>,
}

/// Enumerate due occurrences without replaying an unbounded backlog.
///
/// `first` must be an occurrence of the schedule that is not after `now`.
pub fn due(
    schedule: &RoutineSchedule,
    first: DateTime<Utc>,
    now: DateTime<Utc>,
    cap: usize,
) -> Result<Due, Invalid> {
    Ok(match compile(schedule)? {
        Compiled::Cron { cron, tz } => {
            let mut listed = vec![first];
            let mut truncated = false;
            let mut current = first;
            while let Some(next) = cron_next(&cron, tz, current).filter(|next| *next <= now) {
                if listed.len() >= cap {
                    truncated = true;
                    break;
                }
                listed.push(next);
                current = next;
            }
            let latest = if truncated {
                cron_latest(&cron, tz, now).unwrap_or(current)
            } else {
                current
            };
            Due {
                listed,
                truncated,
                latest,
                next: cron_next(&cron, tz, now),
            }
        }
        Compiled::Interval { secs } => {
            let count = ((now - first).num_seconds().max(0) / secs) + 1;
            let listed = (0..count.min(cap as i64))
                .map(|i| first + Duration::seconds(i * secs))
                .collect();
            Due {
                listed,
                truncated: count > cap as i64,
                latest: first + Duration::seconds((count - 1) * secs),
                next: Some(first + Duration::seconds(count * secs)),
            }
        }
    })
}
