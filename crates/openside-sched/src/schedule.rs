//! Occurrence arithmetic for cron and RRULE schedules (with time zones) and interval schedules.

use std::{collections::HashMap, str::FromStr, sync::Arc};

use chrono::{
    DateTime, Datelike, Duration, LocalResult, Month, Months, NaiveDateTime, Offset, TimeZone,
    Timelike, Utc,
};
use chrono_tz::Tz;
use croner::{
    parser::{CronParser, Seconds, Year},
    Cron,
};
use openside_proto::RoutineSchedule;
use parking_lot::Mutex;
use rrule::{Frequency, NWeekday, RRule, RRuleError, RRuleSet, Unvalidated};

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

/// Longest COUNT accepted. A counted series is always expanded from its own start (the count
/// depends on it), so this bounds the occurrences one computation walks.
pub const MAX_RRULE_COUNT: u32 = 10_000;

/// Shortest gap allowed between two occurrences of an RRULE; denser rules (a BYMINUTE list,
/// for example) belong in an interval schedule.
pub const MIN_RRULE_SPACING: Duration = Duration::minutes(15);

/// Upcoming occurrences inspected for [`MIN_RRULE_SPACING`] when a rule is validated.
const SPACING_CHECKED: usize = 200;

/// Periods (FREQ x INTERVAL) a rule may span between two occurrences, and in total across the
/// spacing check, when it is validated. The `rrule` crate's own limit (100,000 empty steps per
/// occurrence) is fixed, so this is measured on the occurrences it yields; a rule over budget
/// is refused instead of being stored and paying that cost on every tick.
pub const RRULE_STEP_BUDGET: i64 = 10_000;

/// Compiled RRULEs kept for reuse across ticks; cleared wholesale when it grows past this.
const SERIES_CACHE_LIMIT: usize = 512;

type SeriesKey = (String, Option<String>, NaiveDateTime);

static SERIES_CACHE: std::sync::LazyLock<Mutex<HashMap<SeriesKey, Arc<Series>>>> =
    std::sync::LazyLock::new(Mutex::default);

enum Compiled {
    Cron { cron: Box<Cron>, tz: Tz },
    Interval { secs: i64 },
    Rrule(Arc<Series>),
}

fn time_zone(name: Option<&str>) -> Result<Tz, Invalid> {
    match name.map(str::trim) {
        None | Some("") => Ok(Tz::UTC),
        Some(name) => Tz::from_str(name).map_err(|_| Invalid::Timezone(name.to_owned())),
    }
}

fn invalid_rule(detail: impl std::fmt::Display) -> Invalid {
    Invalid::Schedule(format!("invalid recurrence rule: {detail}"))
}

fn rrule_error(error: RRuleError) -> Invalid {
    match error {
        RRuleError::ParserError(error) => invalid_rule(error),
        RRuleError::ValidationError(error) => invalid_rule(error),
        RRuleError::IterError(error) => invalid_rule(error),
    }
}

/// The bare RRULE value: one optional `RRULE:` prefix, never a DTSTART or a second line.
fn rule_value(rule: &str) -> Result<&str, Invalid> {
    let rule = rule.trim();
    let rule = match rule.get(..6) {
        Some(prefix) if prefix.eq_ignore_ascii_case("RRULE:") => rule[6..].trim_start(),
        _ => rule,
    };
    if rule.to_ascii_uppercase().contains("DTSTART") {
        return Err(invalid_rule(
            "DTSTART belongs in the schedule's `start` field",
        ));
    }
    if rule.contains([':', '\n', '\r']) {
        return Err(invalid_rule(
            "expected a single RRULE value such as FREQ=DAILY;BYHOUR=9;BYMINUTE=0",
        ));
    }
    Ok(rule)
}

/// `anchor` as wall-clock time in `tz`, truncated to the minute: the DTSTART of an RRULE
/// schedule that has no `start`.
fn default_start(anchor: DateTime<Utc>, tz: Tz) -> NaiveDateTime {
    let local = anchor.with_timezone(&tz).naive_local();
    local
        .with_second(0)
        .and_then(|local| local.with_nanosecond(0))
        .unwrap_or(local)
}

/// The instant a wall-clock reading names in `tz`: the earlier one inside a fall-back fold,
/// and the reading moved forward by the gap's length inside a spring-forward gap.
fn local_instant(tz: Tz, local: NaiveDateTime) -> DateTime<Tz> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(time) => time,
        LocalResult::Ambiguous(earlier, _) => earlier,
        LocalResult::None => {
            // Reading the gap time with the offset in force before the gap lands that many
            // minutes past the change (Berlin 02:30 -> 01:30Z = 03:30 CEST). A day earlier is
            // safely before the gap whatever the zone's sign; reading `local` itself as UTC
            // falls after the change in zones east of UTC.
            let before = tz
                .offset_from_utc_datetime(&(local - Duration::days(1)))
                .fix();
            tz.from_utc_datetime(&(local - Duration::seconds(before.local_minus_utc().into())))
        }
    }
}

/// An RRULE ready to expand from any whole period of its series.
///
/// RFC 5545 fills BY parts a rule leaves out from DTSTART. They are made explicit here from the
/// series start, so a later DTSTART that is a whole number of periods (FREQ x INTERVAL) after
/// it yields exactly the same occurrences from that point on. Expanding from such a re-anchored
/// start keeps every computation to roughly one period of work, however old the series is.
pub(crate) struct Series {
    rule: RRule<Unvalidated>,
    tz: Tz,
    /// Series start (DTSTART) as wall-clock time in `tz`.
    start: NaiveDateTime,
    freq: Frequency,
    interval: u32,
    /// Counted series are never re-anchored: COUNT is measured from the series start.
    counted: bool,
    until: Option<DateTime<Utc>>,
}

impl Series {
    fn compile(rule: &str, tz: Tz, start: NaiveDateTime) -> Result<Self, Invalid> {
        let parsed: RRule<Unvalidated> = rule_value(rule)?.parse().map_err(rrule_error)?;
        let freq = parsed.get_freq();
        if matches!(freq, Frequency::Minutely | Frequency::Secondly) {
            return Err(invalid_rule(
                "FREQ must be HOURLY, DAILY, WEEKLY, MONTHLY or YEARLY; use an interval \
                 schedule for shorter periods",
            ));
        }
        if parsed
            .get_count()
            .is_some_and(|count| count > MAX_RRULE_COUNT)
        {
            return Err(invalid_rule(format!(
                "COUNT must be at most {MAX_RRULE_COUNT}; use UNTIL or no end for longer series"
            )));
        }
        if !parsed.get_by_second().is_empty() {
            return Err(invalid_rule("BYSECOND is not supported"));
        }
        if never_in_listed_months(&parsed) {
            return Err(invalid_rule(
                "BYMONTHDAY never falls in the BYMONTH months listed",
            ));
        }
        let series = Self {
            interval: u32::from(parsed.get_interval()),
            counted: parsed.get_count().is_some(),
            until: parsed.get_until().map(|until| until.with_timezone(&Utc)),
            rule: explicit_by_parts(parsed, start),
            tz,
            start,
            freq,
        };
        // Surfaces BY ranges and UNTIL before the start as validation errors.
        series
            .rule
            .clone()
            .validate(series.dt_start(local_instant(tz, start)))
            .map_err(rrule_error)?;
        Ok(series)
    }

    fn dt_start(&self, at: DateTime<Tz>) -> DateTime<rrule::Tz> {
        at.with_timezone(&rrule::Tz::from(self.tz))
    }

    /// `periods` whole periods after the series start, as wall-clock time.
    fn shifted(&self, periods: i64) -> Option<NaiveDateTime> {
        let steps = periods.checked_mul(i64::from(self.interval))?;
        let months = |n: i64| u32::try_from(n).ok().map(Months::new);
        match self.freq {
            Frequency::Hourly => self.start.checked_add_signed(Duration::try_hours(steps)?),
            Frequency::Daily => self.start.checked_add_signed(Duration::try_days(steps)?),
            Frequency::Weekly => self
                .start
                .checked_add_signed(Duration::try_days(steps.checked_mul(7)?)?),
            Frequency::Monthly => self.start.checked_add_months(months(steps)?),
            Frequency::Yearly => self
                .start
                .checked_add_months(months(steps.checked_mul(12)?)?),
            Frequency::Minutely | Frequency::Secondly => None,
        }
    }

    /// Whole periods from the series start to `local`, rounded down (or one more, which
    /// [`Series::anchor`] corrects).
    fn periods_until(&self, local: NaiveDateTime) -> i64 {
        let start = self.start;
        let months = |time: NaiveDateTime| i64::from(time.year()) * 12 + i64::from(time.month0());
        let units = match self.freq {
            Frequency::Hourly => (local - start).num_hours(),
            Frequency::Daily => (local - start).num_days(),
            Frequency::Weekly => (local - start).num_days() / 7,
            Frequency::Monthly => months(local) - months(start),
            Frequency::Yearly => i64::from(local.year() - start.year()),
            Frequency::Minutely | Frequency::Secondly => 0,
        };
        units.max(0) / i64::from(self.interval.max(1))
    }

    /// The DTSTART to expand from so that every occurrence after `not_after` is produced: the
    /// latest whole period of the series that starts no later than `not_after`, `back` periods
    /// earlier still, or the series start itself.
    fn anchor(&self, not_after: DateTime<Utc>, back: i64) -> DateTime<Tz> {
        let original = local_instant(self.tz, self.start);
        if self.counted || not_after <= original.with_timezone(&Utc) {
            return original;
        }
        let local = not_after.with_timezone(&self.tz).naive_local();
        let mut periods = self.periods_until(local).saturating_sub(back);
        // At most a couple of steps: an overshoot of one period, or a period that starts
        // inside a daylight-saving gap.
        while periods > 0 {
            if let Some(shifted) = self.shifted(periods) {
                let instant = match self.tz.from_local_datetime(&shifted) {
                    LocalResult::Single(time) | LocalResult::Ambiguous(time, _) => Some(time),
                    LocalResult::None => None,
                };
                if let Some(instant) = instant.filter(|time| *time <= not_after) {
                    return instant;
                }
            }
            periods -= 1;
        }
        original
    }

    /// Occurrences from `dt_start` on, in order; empty once UNTIL is before it. Validation
    /// limits stay on, so a rule whose BY parts never match stops instead of looping.
    fn occurrences(&self, dt_start: DateTime<Tz>) -> impl Iterator<Item = DateTime<Utc>> {
        let ended = matches!(self.until, Some(until) if until < dt_start.with_timezone(&Utc));
        let set = if ended {
            None
        } else {
            self.rule
                .clone()
                .build(self.dt_start(dt_start))
                .ok()
                .map(RRuleSet::limit)
        };
        // The set iterator owns its state, so it outlives the set it came from.
        set.into_iter()
            .flat_map(|set| (&set).into_iter())
            .map(|time| time.with_timezone(&Utc))
    }

    fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.occurrences(self.anchor(after, 0))
            .find(|time| *time > after)
    }

    /// The last occurrence at or before `through`, looking no further back than `floor` (an
    /// occurrence the caller already knows). Widens the window by doubling, so a recent
    /// occurrence costs about one period of work.
    fn latest_through(
        &self,
        through: DateTime<Utc>,
        floor: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let mut back = 0_i64;
        loop {
            let dt_start = self.anchor(through, back);
            let found = self
                .occurrences(dt_start)
                .take_while(|time| *time <= through)
                .last();
            if found.is_some()
                || dt_start.with_timezone(&Utc) <= floor
                || back > i64::from(u32::MAX)
            {
                return found;
            }
            back = (back * 2).max(1);
        }
    }
}

/// True when the rule lists both months and days of the month and no listed day exists in any
/// listed month (BYMONTH=2;BYMONTHDAY=30). Such a rule parses but would only be found empty
/// after the `rrule` crate walked its whole iteration limit.
fn never_in_listed_months(rule: &RRule<Unvalidated>) -> bool {
    const LONGEST: [u8; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let (months, days) = (rule.get_by_month(), rule.get_by_month_day());
    if months.is_empty() || days.is_empty() {
        return false;
    }
    !months.iter().any(|month| {
        let longest = LONGEST[usize::from((*month).clamp(1, 12)) - 1];
        // A negative day counts from the month's end; -31 needs a 31-day month too.
        days.iter().any(|day| day.unsigned_abs() <= longest)
    })
}

/// RFC 5545 defaults for BY parts the rule leaves out, taken from `start` as the `rrule` crate
/// does when it validates against DTSTART.
fn explicit_by_parts(rule: RRule<Unvalidated>, start: NaiveDateTime) -> RRule<Unvalidated> {
    let freq = rule.get_freq();
    let mut rule = rule;
    let day_parts_empty = rule.get_by_week_no().is_empty()
        && rule.get_by_year_day().is_empty()
        && rule.get_by_month_day().is_empty()
        && rule.get_by_weekday().is_empty();
    if day_parts_empty {
        let day = i8::try_from(start.day()).unwrap_or(1);
        match freq {
            Frequency::Yearly => {
                if rule.get_by_month().is_empty() {
                    let month = Month::try_from(u8::try_from(start.month()).unwrap_or(1))
                        .unwrap_or(Month::January);
                    rule = rule.by_month(&[month]);
                }
                rule = rule.by_month_day(vec![day]);
            }
            Frequency::Monthly => rule = rule.by_month_day(vec![day]),
            Frequency::Weekly => rule = rule.by_weekday(vec![NWeekday::Every(start.weekday())]),
            _ => {}
        }
    }
    let part = |value: u32| u8::try_from(value).unwrap_or(0);
    if rule.get_by_hour().is_empty() && freq < Frequency::Hourly {
        rule = rule.by_hour(vec![part(start.hour())]);
    }
    if rule.get_by_minute().is_empty() && freq < Frequency::Minutely {
        rule = rule.by_minute(vec![part(start.minute())]);
    }
    if rule.get_by_second().is_empty() && freq < Frequency::Secondly {
        rule = rule.by_second(vec![part(start.second())]);
    }
    rule
}

/// The compiled series for this rule, zone and start, shared by every routine using them.
fn series(
    rule: &str,
    timezone: Option<&str>,
    tz: Tz,
    start: NaiveDateTime,
) -> Result<Arc<Series>, Invalid> {
    let key = (rule.to_owned(), timezone.map(str::to_owned), start);
    if let Some(series) = SERIES_CACHE.lock().get(&key) {
        return Ok(series.clone());
    }
    let series = Arc::new(Series::compile(rule, tz, start)?);
    let mut cache = SERIES_CACHE.lock();
    if cache.len() >= SERIES_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, series.clone());
    Ok(series)
}

fn compile(schedule: &RoutineSchedule, anchor: DateTime<Utc>) -> Result<Compiled, Invalid> {
    match schedule {
        RoutineSchedule::Cron { expr, timezone } => {
            let tz = time_zone(timezone.as_deref())?;
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
        RoutineSchedule::Rrule {
            rule,
            timezone,
            start,
        } => {
            let tz = time_zone(timezone.as_deref())?;
            let start = start
                .map(|start| start.with_nanosecond(0).unwrap_or(start))
                .unwrap_or_else(|| default_start(anchor, tz));
            Ok(Compiled::Rrule(series(
                rule,
                timezone.as_deref(),
                tz,
                start,
            )?))
        }
    }
}

/// The schedule as it is stored for a routine anchored at `anchor` (its creation or its last
/// schedule change). An RRULE without `start` gets the anchor's minute in its time zone, so
/// restarts and later re-enables continue the same series, and a leading `RRULE:` is dropped.
/// Other schedules are returned unchanged.
pub fn resolve(
    schedule: RoutineSchedule,
    anchor: DateTime<Utc>,
) -> Result<RoutineSchedule, Invalid> {
    Ok(match schedule {
        RoutineSchedule::Rrule {
            rule,
            timezone,
            start,
        } => {
            let tz = time_zone(timezone.as_deref())?;
            RoutineSchedule::Rrule {
                rule: rule_value(&rule)?.to_owned(),
                start: Some(start.unwrap_or_else(|| default_start(anchor, tz))),
                timezone,
            }
        }
        other => other,
    })
}

/// Reject schedules that could never fire or that exceed supported ranges. `now` is the
/// caller's clock, so validation never depends on wall time.
pub fn validate(schedule: &RoutineSchedule, now: DateTime<Utc>) -> Result<(), Invalid> {
    let compiled = compile(schedule, now)?;
    match compiled {
        // A pattern such as "0 0 30 2 *" parses but never occurs.
        Compiled::Cron { .. } => next_compiled(&compiled, now, now)
            .map(drop)
            .ok_or_else(|| Invalid::Schedule("cron expression never fires".into())),
        // Also rejects a series that ended already, such as a one-time routine in the past.
        Compiled::Rrule(series) => {
            let anchor = series.anchor(now, 0);
            let step = |time: NaiveDateTime| series.periods_until(time);
            let mut position = step(anchor.naive_local());
            let mut walked = 0_i64;
            let mut previous: Option<DateTime<Utc>> = None;
            for time in series
                .occurrences(anchor)
                .skip_while(|time| *time <= now)
                .take(SPACING_CHECKED)
            {
                let here = step(time.with_timezone(&series.tz).naive_local());
                let gap = here - position;
                position = here;
                // A routine computation expands about one gap, so a rule whose gaps outgrow
                // the budget is refused rather than paid for on every tick.
                if gap > RRULE_STEP_BUDGET {
                    return Err(Invalid::Schedule(
                        "recurrence rule takes too long to expand; use a coarser FREQ \
                         (for example DAILY instead of HOURLY)"
                            .into(),
                    ));
                }
                if previous.is_some_and(|previous| time - previous < MIN_RRULE_SPACING) {
                    return Err(invalid_rule(format!(
                        "occurrences must be at least {} minutes apart; use an interval \
                         schedule for shorter periods",
                        MIN_RRULE_SPACING.num_minutes()
                    )));
                }
                previous = Some(time);
                walked += gap;
                if walked > RRULE_STEP_BUDGET {
                    break;
                }
            }
            if previous.is_none() {
                return Err(Invalid::Schedule("recurrence rule never fires".into()));
            }
            Ok(())
        }
        Compiled::Interval { .. } => Ok(()),
    }
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

fn next_compiled(
    compiled: &Compiled,
    after: DateTime<Utc>,
    anchor: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    match compiled {
        Compiled::Cron { cron, tz } => cron_next(cron, *tz, after),
        Compiled::Interval { secs } => {
            let elapsed = (after - anchor).num_seconds();
            let periods = if elapsed < 0 { 0 } else { elapsed / secs + 1 };
            Some(anchor + Duration::seconds(periods * secs))
        }
        Compiled::Rrule(series) => series.next_after(after),
    }
}

/// First occurrence strictly after `after`, or `None` once an RRULE series has ended.
/// Interval schedules, and RRULE schedules without `start`, are anchored at `anchor`.
pub fn next_after(
    schedule: &RoutineSchedule,
    after: DateTime<Utc>,
    anchor: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Invalid> {
    Ok(next_compiled(&compile(schedule, anchor)?, after, anchor))
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
/// `first` must be an occurrence of the schedule that is not after `now`. An RRULE without
/// `start` is read from `first`; the scheduler always stores [`resolve`]d schedules, so this
/// only matters to callers that skipped it.
pub fn due(
    schedule: &RoutineSchedule,
    first: DateTime<Utc>,
    now: DateTime<Utc>,
    cap: usize,
) -> Result<Due, Invalid> {
    Ok(match compile(schedule, first)? {
        Compiled::Cron { cron, tz } => {
            // `first` is due, so with no room to list it the listing is already truncated.
            let mut listed = Vec::new();
            let mut truncated = cap == 0;
            let mut current = first;
            if !truncated {
                listed.push(first);
                while let Some(next) = cron_next(&cron, tz, current).filter(|next| *next <= now) {
                    if listed.len() >= cap {
                        truncated = true;
                        break;
                    }
                    listed.push(next);
                    current = next;
                }
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
        Compiled::Rrule(series) => {
            let mut listed = Vec::new();
            let mut truncated = cap == 0;
            let mut current = first;
            let mut next = None;
            if !truncated {
                listed.push(first);
                for time in series
                    .occurrences(series.anchor(first, 0))
                    .skip_while(|time| *time <= first)
                {
                    if time > now {
                        next = Some(time);
                        break;
                    }
                    if listed.len() >= cap {
                        truncated = true;
                        break;
                    }
                    listed.push(time);
                    current = time;
                }
            }
            // Past the cap the backlog is skipped, not walked: the latest occurrence and the
            // next one are expanded from the period around `now`.
            let (latest, next) = if truncated {
                (
                    series.latest_through(now, current).unwrap_or(current),
                    series.next_after(now),
                )
            } else {
                (current, next)
            };
            Due {
                listed,
                truncated,
                latest,
                next,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn local(text: &str) -> NaiveDateTime {
        text.parse().unwrap()
    }

    #[test]
    fn an_old_series_is_expanded_from_the_period_around_the_query_not_from_its_start() {
        let series = Series::compile(
            "FREQ=HOURLY;INTERVAL=5;BYMINUTE=30",
            Tz::UTC,
            local("2006-01-01T00:30:00"),
        )
        .unwrap();
        let now = at("2026-11-06T12:00:00Z");
        let anchor = series.anchor(now, 0).with_timezone(&Utc);
        assert!(
            anchor <= now && now - anchor < Duration::hours(5),
            "{anchor}"
        );
        // Hours since the start stay a multiple of INTERVAL.
        assert_eq!((anchor - at("2006-01-01T00:30:00Z")).num_hours() % 5, 0);
        // One period of expansion reaches the next occurrence.
        assert!(series
            .occurrences(series.anchor(now, 0))
            .take(2)
            .any(|time| time > now));
    }

    #[test]
    fn re_anchoring_keeps_by_parts_that_came_from_the_start() {
        // Implicit BYMONTHDAY=31 from the start must survive an anchor clamped to 28 February.
        let series =
            Series::compile("FREQ=MONTHLY", Tz::UTC, local("2020-01-31T08:00:00")).unwrap();
        let after = at("2027-02-15T00:00:00Z");
        assert_eq!(series.next_after(after), Some(at("2027-03-31T08:00:00Z")));
        let leap = Series::compile("FREQ=YEARLY", Tz::UTC, local("2024-02-29T09:00:00")).unwrap();
        assert_eq!(
            leap.next_after(at("2025-01-01T00:00:00Z")),
            Some(at("2028-02-29T09:00:00Z"))
        );
        let weekly = Series::compile(
            "FREQ=WEEKLY;INTERVAL=3",
            Tz::UTC,
            local("2026-01-02T10:00:00"), // a Friday
        )
        .unwrap();
        let next = weekly.next_after(at("2026-11-06T12:00:00Z")).unwrap();
        assert_eq!(next.weekday(), chrono::Weekday::Fri);
        assert_eq!((next - at("2026-01-02T10:00:00Z")).num_days() % 21, 0);
    }

    #[test]
    fn counted_series_always_expand_from_their_start() {
        let series =
            Series::compile("FREQ=DAILY;COUNT=3", Tz::UTC, local("2026-11-01T09:00:00")).unwrap();
        let start = series
            .anchor(at("2026-12-01T00:00:00Z"), 0)
            .with_timezone(&Utc);
        assert_eq!(start, at("2026-11-01T09:00:00Z"));
        assert_eq!(
            series.next_after(at("2026-11-02T10:00:00Z")),
            Some(at("2026-11-03T09:00:00Z"))
        );
        assert_eq!(series.next_after(at("2026-11-03T09:00:00Z")), None);
    }

    #[test]
    fn wall_clock_starts_resolve_gaps_forward_and_folds_to_the_earlier_instant() {
        let instant = |zone: &str, time: &str| {
            local_instant(Tz::from_str(zone).unwrap(), local(time)).with_timezone(&Utc)
        };
        // Spring gaps move forward by the gap's length, east and west of UTC.
        assert_eq!(
            instant("Europe/Berlin", "2026-03-29T02:30:00"),
            at("2026-03-29T01:30:00Z"), // 03:30 CEST
        );
        assert_eq!(
            instant("America/New_York", "2026-03-08T02:30:00"),
            at("2026-03-08T07:30:00Z"), // 03:30 EDT
        );
        // Fall folds keep the earlier instant.
        assert_eq!(
            instant("America/New_York", "2026-11-01T01:30:00"),
            at("2026-11-01T05:30:00Z"), // 01:30 EDT
        );
        assert_eq!(
            instant("Europe/Berlin", "2026-10-25T02:30:00"),
            at("2026-10-25T00:30:00Z"), // 02:30 CEST
        );
        // No daylight saving: the same reading every day of the year.
        assert_eq!(
            instant("Asia/Seoul", "2026-03-29T02:30:00"),
            at("2026-03-28T17:30:00Z")
        );
        assert_eq!(
            instant("Asia/Seoul", "2026-11-01T01:30:00"),
            at("2026-10-31T16:30:00Z")
        );

        let berlin = Series::compile(
            "FREQ=DAILY",
            Tz::Europe__Berlin,
            local("2026-03-29T02:30:00"),
        )
        .unwrap();
        assert_eq!(
            berlin.next_after(at("2026-03-28T12:00:00Z")),
            Some(at("2026-03-29T01:30:00Z"))
        );
        assert_eq!(
            berlin.next_after(at("2026-03-29T01:30:00Z")),
            Some(at("2026-03-30T00:30:00Z")), // back to 02:30 CEST
        );
    }

    #[test]
    fn compiled_series_are_reused() {
        let first = series("FREQ=DAILY", None, Tz::UTC, local("2031-05-04T03:02:00")).unwrap();
        let again = series("FREQ=DAILY", None, Tz::UTC, local("2031-05-04T03:02:00")).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        let other = series(
            "FREQ=DAILY",
            Some("UTC"),
            Tz::UTC,
            local("2031-05-04T03:02:00"),
        )
        .unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
    }
}
