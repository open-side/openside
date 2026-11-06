import type { RoutineSchedule } from "../../api/types";
import { readRrule, readWallClock, untilDate, WEEK_ORDER, weekdayOf, WORK_WEEK, zonedWallClock, type RruleDay } from "../../pages/format";

/** `existing` keeps a cron or interval schedule (older routines, suggestions) until another choice replaces it. */
export type Repeat = "once" | "hourly" | "daily" | "weekdays" | "weekly" | "monthly" | "custom" | "existing";
export type Ends = "never" | "on" | "after";

/** The routine dialog's schedule fields, as the inputs hold them. */
export interface ScheduleForm {
  repeat: Repeat;
  /** "Repeat every N" for hourly, daily, weekly and monthly. */
  interval: string;
  /** First run (the series start) as `YYYY-MM-DD` and `HH:MM`, wall clock in `timezone`. */
  date: string;
  time: string;
  /** Weekly: the chosen weekdays. */
  days: RruleDay[];
  /** Monthly: the day of the month, 1-31. */
  monthDay: string;
  ends: Ends;
  /** Ends on this date (`YYYY-MM-DD`, inclusive, in `timezone`). */
  until: string;
  /** Ends after this many runs. */
  count: string;
  timezone: string;
  /** Custom: the RRULE as typed. */
  rule: string;
}

export function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

const pad = (value: number) => String(value).padStart(2, "0");
const isoDate = ({ year, month, day }: { year: number; month: number; day: number }) => `${year}-${pad(month)}-${pad(day)}`;

/** UTC milliseconds of a wall-clock time in `timeZone`; the offset is read twice so a DST change between guess and answer settles. */
function zonedInstant(date: string, time: string, timeZone: string): number {
  const wall = readWallClock(`${date}T${time}`)!;
  const asUtc = Date.UTC(wall.year, wall.month - 1, wall.day, wall.hour, wall.minute, wall.second);
  const offset = (instant: number) => {
    const seen = zonedWallClock(instant, timeZone);
    return Date.UTC(seen.year, seen.month - 1, seen.day, seen.hour, seen.minute, seen.second) - instant;
  };
  return asUtc - offset(asUtc - offset(asUtc));
}

/** RFC 5545 UNTIL in UTC (`YYYYMMDDTHHMMSSZ`): the last second of `date` in `timeZone`, so that day's run still happens. */
export function untilStamp(date: string, timeZone: string): string {
  return new Date(zonedInstant(date, "23:59:59", timeZone)).toISOString().replace(/\.\d{3}Z$/, "Z").replace(/[-:]/g, "");
}

/** A new routine: daily from the next full hour in `timezone`. */
export function defaultForm(now = Date.now(), timezone = browserTimeZone()): ScheduleForm {
  const next = zonedWallClock(now + 3_600_000, timezone);
  const date = isoDate(next);
  return {
    repeat: "daily",
    interval: "1",
    date,
    time: `${pad(next.hour)}:00`,
    days: [weekdayOf(next.year, next.month, next.day)],
    monthDay: String(next.day),
    ends: "never",
    until: date,
    count: "5",
    timezone,
    rule: "",
  };
}

/**
 * The form for an existing schedule. An RRULE the form can express comes back field by field;
 * any other rule opens as Custom with its text, and cron or interval schedules as `existing`.
 */
export function formFromSchedule(schedule: RoutineSchedule, now = Date.now()): ScheduleForm {
  if (schedule.type !== "rrule") {
    return { ...defaultForm(now, (schedule.type === "cron" && schedule.timezone) || browserTimeZone()), repeat: "existing" };
  }
  const timezone = schedule.timezone || "UTC";
  const parsed = readRrule(schedule.rule);
  const start = readWallClock(schedule.start);
  // RFC 5545: BYHOUR/BYMINUTE that are not given come from the series start.
  const hour = parsed?.hour ?? start?.hour;
  const minute = parsed?.minute ?? start?.minute;
  const base = defaultForm(now, timezone);
  const form: ScheduleForm = {
    ...base,
    repeat: "custom",
    date: start ? isoDate(start) : base.date,
    time: hour !== undefined && minute !== undefined ? `${pad(hour)}:${pad(minute)}` : base.time,
    rule: schedule.rule,
  };
  const day = readWallClock(`${form.date}T00:00`)!;
  form.days = [weekdayOf(day.year, day.month, day.day)];
  form.monthDay = String(day.day);
  if (!parsed) return form;
  form.interval = String(parsed.interval);
  if (parsed.count !== undefined) {
    form.ends = "after";
    form.count = String(parsed.count);
  } else if (parsed.until !== undefined) {
    form.ends = "on";
    form.until = isoDate(untilDate(parsed.until, timezone));
  }
  switch (parsed.freq) {
    case "HOURLY":
      // The form writes no BYMINUTE for hourly runs; a rule with one stays Custom.
      if (parsed.minute === undefined) form.repeat = "hourly";
      break;
    case "DAILY":
      if (parsed.count === 1 && parsed.hour === undefined && parsed.minute === undefined) {
        form.repeat = "once";
        form.ends = "never";
        form.interval = "1";
      } else {
        form.repeat = "daily";
      }
      break;
    case "WEEKLY":
      if (parsed.days) form.days = parsed.days;
      form.repeat = parsed.interval === 1 && form.days.join() === WORK_WEEK.join() ? "weekdays" : "weekly";
      break;
    case "MONTHLY":
      if (parsed.monthDay !== undefined) form.monthDay = String(parsed.monthDay);
      form.repeat = "monthly";
      break;
    case "YEARLY":
      break;
  }
  return form;
}

const positive = (value: string) => /^\d+$/.test(value.trim()) && Number(value) >= 1;
/** The daemon refuses COUNT above this (docs/specs/routines.md §11). */
export const MAX_COUNT = 10_000;

/** Whether every field the chosen repeat needs holds a value the schedule can be built from. */
export function formComplete(form: ScheduleForm): boolean {
  if (form.repeat === "existing") return true;
  if (!/^\d{4}-\d{2}-\d{2}$/.test(form.date) || !/^\d{2}:\d{2}$/.test(form.time)) return false;
  if (form.repeat === "custom") return form.rule.trim() !== "";
  if (form.repeat === "once") return true;
  if (form.repeat !== "weekdays" && !positive(form.interval)) return false;
  if (form.repeat === "weekly" && form.days.length === 0) return false;
  if (form.repeat === "monthly" && !(positive(form.monthDay) && Number(form.monthDay) <= 31)) return false;
  if (form.ends === "after") return positive(form.count) && Number(form.count) <= MAX_COUNT;
  if (form.ends === "on") return /^\d{4}-\d{2}-\d{2}$/.test(form.until);
  return true;
}

/**
 * The schedule to send (contract C1): an RRULE read in `timezone` from `start`. Once is
 * `FREQ=DAILY;COUNT=1`; `existing` hands back the schedule the routine already has.
 */
export function scheduleFromForm(form: ScheduleForm, existing?: RoutineSchedule): RoutineSchedule {
  if (form.repeat === "existing" && existing) return existing;
  const start = `${form.date}T${form.time}:00`;
  const rrule = (rule: string): RoutineSchedule => ({ type: "rrule", rule, timezone: form.timezone, start });
  if (form.repeat === "once") return rrule("FREQ=DAILY;COUNT=1");
  if (form.repeat === "custom") return rrule(form.rule.trim());
  const [hour, minute] = form.time.split(":").map(Number);
  const clock = [`BYHOUR=${hour}`, `BYMINUTE=${minute}`];
  const interval = Number(form.interval);
  const every = interval > 1 ? [`INTERVAL=${interval}`] : [];
  const parts =
    form.repeat === "hourly"
      ? ["FREQ=HOURLY", ...every]
      : form.repeat === "daily"
        ? ["FREQ=DAILY", ...every, ...clock]
        : form.repeat === "weekdays"
          ? ["FREQ=WEEKLY", `BYDAY=${WORK_WEEK.join(",")}`, ...clock]
          : form.repeat === "weekly"
            ? ["FREQ=WEEKLY", ...every, `BYDAY=${WEEK_ORDER.filter((day) => form.days.includes(day)).join(",")}`, ...clock]
            : ["FREQ=MONTHLY", ...every, `BYMONTHDAY=${Number(form.monthDay)}`, ...clock];
  if (form.ends === "after") parts.push(`COUNT=${Number(form.count)}`);
  if (form.ends === "on") parts.push(`UNTIL=${untilStamp(form.until, form.timezone)}`);
  return rrule(parts.join(";"));
}
