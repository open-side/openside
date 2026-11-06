import type { RoutineSchedule } from "../api/types";
import { formatDate as formatLocaleDate, formatList, formatNumber, formatRelativeTime, t } from "../i18n";

/** "Oct 12, 2026, 7:01 AM" from an RFC 3339 string or Unix milliseconds. */
export function formatDateTime(value: string | number): string {
  return formatLocaleDate(new Date(value), { dateStyle: "medium", timeStyle: "short" });
}

export function formatDate(value: string | number): string {
  return formatLocaleDate(new Date(value), { dateStyle: "medium" });
}

const ONE_DECIMAL = { minimumFractionDigits: 1, maximumFractionDigits: 1, useGrouping: false } as const;

/** "12.3 MB"; binary units, one decimal from KB up. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${formatNumber(bytes, { useGrouping: false })} B`;
  if (bytes < 1024 * 1024) return `${formatNumber(bytes / 1024, ONE_DECIMAL)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${formatNumber(bytes / 1024 / 1024, ONE_DECIMAL)} MB`;
  return `${formatNumber(bytes / 1024 / 1024 / 1024, { minimumFractionDigits: 2, maximumFractionDigits: 2, useGrouping: false })} GB`;
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["day", 86_400_000],
  ["hour", 3_600_000],
  ["minute", 60_000],
];

/** "in 2 hours", "3 days ago"; under a minute reads as "now". */
export function formatRelative(value: string | number, now = Date.now()): string {
  const delta = new Date(value).getTime() - now;
  for (const [unit, size] of UNITS) {
    if (Math.abs(delta) >= size) return formatRelativeTime(Math.round(delta / size), unit, { numeric: "auto" });
  }
  return formatRelativeTime(0, "minute", { numeric: "auto" });
}

/** The locale's weekday name; 1970-01-04 was a Sunday, so day 0 lines up with cron's 0. */
export function weekdayName(day: number, width: "long" | "short" = "long"): string {
  return formatLocaleDate(Date.UTC(1970, 0, 4 + day), { weekday: width, timeZone: "UTC" });
}

/** "9:00 AM" in the locale's clock, for a wall-clock hour and minute. */
export function clockTime(hour: number, minute: number): string {
  return formatLocaleDate(Date.UTC(1970, 0, 1, hour, minute), { hour: "numeric", minute: "2-digit", timeZone: "UTC" });
}

/** "Nov 6, 2026" for a calendar date (month 1-12), whatever the browser's zone. */
export function calendarDate(year: number, month: number, day: number): string {
  return formatLocaleDate(Date.UTC(year, month - 1, day), { dateStyle: "medium", timeZone: "UTC" });
}

function atTime(when: string, hour: number | undefined, minute: number | undefined): string {
  return hour === undefined || minute === undefined ? when : t("settings.schedule.atTime", { when, time: clockTime(hour, minute) });
}

// Count 1 has its own keys: Korean plural rules only ever pick `other`, so a `one` form would never show there.
const everyHours = (count: number) => (count === 1 ? t("settings.schedule.everyHour") : t("settings.schedule.everyHours", { count }));
const everyDays = (count: number) => (count === 1 ? t("settings.schedule.everyDay") : t("settings.schedule.everyDays", { count }));
const everyWeeks = (count: number) => (count === 1 ? t("settings.schedule.everyWeek") : t("settings.schedule.everyWeeks", { count }));
const everyWeeksOn = (count: number, weekdays: string) =>
  count === 1 ? t("settings.schedule.everyWeekOn", { weekdays }) : t("settings.schedule.everyWeeksOn", { count, weekdays });
const everyMonths = (count: number) => (count === 1 ? t("settings.schedule.everyMonth") : t("settings.schedule.everyMonths", { count }));
const everyMonthsOn = (count: number, day: number) => (count === 1 ? t("settings.schedule.everyMonthOn", { day }) : t("settings.schedule.everyMonthsOn", { count, day }));

function describeAlias(expr: string): string | undefined {
  switch (expr) {
    case "@hourly":
      return everyHours(1);
    case "@daily":
    case "@midnight":
      return atTime(everyDays(1), 0, 0);
    case "@weekly":
      return atTime(everyWeeksOn(1, weekdayName(0)), 0, 0);
    case "@monthly":
      return atTime(everyMonthsOn(1, 1), 0, 0);
    case "@yearly":
    case "@annually":
      return atTime(t("settings.schedule.everyYearOn", { date: yearDay(1, 1) }), 0, 0);
    default:
      return undefined;
  }
}

/** "January 1": a month and day with no year. */
function yearDay(month: number, day: number): string {
  return formatLocaleDate(Date.UTC(2000, month - 1, day), { month: "long", day: "numeric", timeZone: "UTC" });
}

/**
 * Plain-language cron for the common shapes (daily, weekdays, one weekday, hourly); anything
 * else is shown as the expression itself, which is what the person typed.
 */
function describeCron(expr: string): string {
  const alias = describeAlias(expr.trim());
  if (alias) return alias;
  const raw = t("settings.schedule.cron", { expr });
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 5) return raw;
  const [minute, hour, day, month, weekday] = parts as [string, string, string, string, string];
  const numeric = /^\d+$/;
  if (!numeric.test(minute) || Number(minute) > 59 || day !== "*" || month !== "*") return raw;
  if (hour === "*" && weekday === "*") return t("settings.schedule.hourlyAt", { minute: minute.padStart(2, "0") });
  if (!numeric.test(hour) || Number(hour) > 23) return raw;
  const at = (when: string) => atTime(when, Number(hour), Number(minute));
  if (weekday === "*") return at(everyDays(1));
  if (weekday === "1-5") return at(t("settings.schedule.everyWeekday"));
  if (weekday === "0,6" || weekday === "6,0") return at(t("settings.schedule.everyWeekend"));
  if (numeric.test(weekday)) return at(everyWeeksOn(1, weekdayName(Number(weekday) % 7)));
  return raw;
}

function describeInterval(secs: number): string {
  if (secs % 86_400 === 0) return everyDays(secs / 86_400);
  if (secs % 3_600 === 0) return everyHours(secs / 3_600);
  if (secs % 60 === 0) return t("settings.schedule.everyMinutes", { count: secs / 60 });
  return t("settings.schedule.everySeconds", { count: secs });
}

/** RRULE weekday codes indexed like cron and `Date.getUTCDay` (Sunday = 0). */
export const RRULE_DAYS = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"] as const;
export type RruleDay = (typeof RRULE_DAYS)[number];

/** Monday first, the order weekday lists are read and written in. */
export const WEEK_ORDER: readonly RruleDay[] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

export const WORK_WEEK: readonly RruleDay[] = ["MO", "TU", "WE", "TH", "FR"];

/** A recurrence the settings form can show field by field; anything richer stays a rule. */
export interface SimpleRrule {
  freq: "HOURLY" | "DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY";
  interval: number;
  /** BYDAY, Monday first (weekly only). */
  days?: RruleDay[];
  /** BYMONTHDAY, 1-31 (monthly and yearly). */
  monthDay?: number;
  /** BYMONTH, 1-12 (yearly only). */
  month?: number;
  hour?: number;
  minute?: number;
  count?: number;
  /** UNTIL as written: `YYYYMMDD`, `YYYYMMDDTHHMMSS` or `YYYYMMDDTHHMMSSZ`. */
  until?: string;
}

const RULE_KEYS: Record<SimpleRrule["freq"], readonly string[]> = {
  HOURLY: ["BYMINUTE"],
  DAILY: ["BYHOUR", "BYMINUTE"],
  WEEKLY: ["BYDAY", "BYHOUR", "BYMINUTE"],
  MONTHLY: ["BYMONTHDAY", "BYHOUR", "BYMINUTE"],
  YEARLY: ["BYMONTH", "BYMONTHDAY", "BYHOUR", "BYMINUTE"],
};

function bounded(value: string | undefined, min: number, max: number): number | null | undefined {
  if (value === undefined) return undefined;
  if (!/^\d+$/.test(value)) return null;
  const number = Number(value);
  return number >= min && number <= max ? number : null;
}

/**
 * Reads an RRULE value (a leading `RRULE:` is dropped) when it only uses the parts a person
 * picks in the routine form: FREQ, INTERVAL, plain BYDAY codes, one BYMONTHDAY/BYMONTH/BYHOUR/
 * BYMINUTE, COUNT or UNTIL. Null for anything else (ordinal weekdays, lists of hours, BYSETPOS…).
 */
export function readRrule(rule: string): SimpleRrule | null {
  const parts = new Map<string, string>();
  for (const part of rule.trim().replace(/^RRULE:/i, "").split(";")) {
    if (!part) continue;
    const [key, value, ...rest] = part.split("=");
    const name = key?.trim().toUpperCase();
    if (!name || value === undefined || rest.length || parts.has(name)) return null;
    parts.set(name, value.trim().toUpperCase());
  }
  const freq = parts.get("FREQ") as SimpleRrule["freq"] | undefined;
  if (!freq || !(freq in RULE_KEYS)) return null;
  const allowed = new Set(["FREQ", "INTERVAL", "COUNT", "UNTIL", "WKST", ...RULE_KEYS[freq]]);
  if ([...parts.keys()].some((key) => !allowed.has(key))) return null;
  if (parts.has("COUNT") && parts.has("UNTIL")) return null;
  const interval = bounded(parts.get("INTERVAL"), 1, Number.MAX_SAFE_INTEGER);
  const monthDay = bounded(parts.get("BYMONTHDAY"), 1, 31);
  const month = bounded(parts.get("BYMONTH"), 1, 12);
  const hour = bounded(parts.get("BYHOUR"), 0, 23);
  const minute = bounded(parts.get("BYMINUTE"), 0, 59);
  const count = bounded(parts.get("COUNT"), 1, Number.MAX_SAFE_INTEGER);
  if ([interval, monthDay, month, hour, minute, count].includes(null)) return null;
  const until = parts.get("UNTIL");
  if (until !== undefined && !/^\d{8}(T\d{6}Z?)?$/.test(until)) return null;
  let days: RruleDay[] | undefined;
  const byDay = parts.get("BYDAY");
  if (byDay !== undefined) {
    const codes = byDay.split(",");
    if (codes.some((code) => !WEEK_ORDER.includes(code as RruleDay)) || new Set(codes).size !== codes.length) return null;
    days = WEEK_ORDER.filter((day) => codes.includes(day));
  }
  return {
    freq,
    interval: interval ?? 1,
    ...(days ? { days } : {}),
    ...(monthDay != null ? { monthDay } : {}),
    ...(month != null ? { month } : {}),
    ...(hour != null ? { hour } : {}),
    ...(minute != null ? { minute } : {}),
    ...(count != null ? { count } : {}),
    ...(until !== undefined ? { until } : {}),
  };
}

export interface WallClock {
  year: number;
  /** 1-12 */
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
}

/** `YYYY-MM-DDTHH:MM[:SS]` wall-clock fields; null when absent or malformed. */
export function readWallClock(value: string | null | undefined): WallClock | null {
  const match = value?.match(/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?/);
  if (!match) return null;
  const [year, month, day, hour, minute, second] = match.slice(1).map((part) => Number(part ?? 0)) as [number, number, number, number, number, number];
  return { year, month, day, hour, minute, second };
}

const WALL_CLOCK = { year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric", second: "numeric", hourCycle: "h23" } as const;

/** The wall clock `instant` reads in `timeZone`, UTC when the zone is unknown. */
export function zonedWallClock(instant: number, timeZone: string): WallClock {
  let format: Intl.DateTimeFormat;
  try {
    format = new Intl.DateTimeFormat("en-US", { ...WALL_CLOCK, timeZone });
  } catch {
    format = new Intl.DateTimeFormat("en-US", { ...WALL_CLOCK, timeZone: "UTC" });
  }
  const parts = Object.fromEntries(format.formatToParts(instant).map((part) => [part.type, Number(part.value)]));
  // Some engines still print midnight as hour 24 under h23.
  return { year: parts.year!, month: parts.month!, day: parts.day!, hour: parts.hour! % 24, minute: parts.minute!, second: parts.second! };
}

/** The last day of the series: UNTIL's date in the routine's zone (a UTC UNTIL can fall on the next day in UTC). */
export function untilDate(until: string, timeZone: string): { year: number; month: number; day: number } {
  const [year, month, day] = [until.slice(0, 4), until.slice(4, 6), until.slice(6, 8)].map(Number) as [number, number, number];
  if (!until.endsWith("Z")) return { year, month, day };
  const instant = Date.UTC(year, month - 1, day, Number(until.slice(9, 11)), Number(until.slice(11, 13)), Number(until.slice(13, 15)));
  return zonedWallClock(instant, timeZone);
}

/** The RRULE weekday a calendar date (month 1-12) falls on. */
export function weekdayOf(year: number, month: number, day: number): RruleDay {
  return RRULE_DAYS[new Date(Date.UTC(year, month - 1, day)).getUTCDay()]!;
}

function describeRrule(rule: string, start: string | null | undefined, timeZone: string): string {
  const raw = t("settings.schedule.rule", { rule });
  const parsed = readRrule(rule);
  if (!parsed) return raw;
  const dtstart = readWallClock(start);
  // RFC 5545: a BY part that is not given takes its value from DTSTART.
  const hour = parsed.hour ?? dtstart?.hour;
  const minute = parsed.minute ?? dtstart?.minute;
  const every = parsed.interval;
  let when: string;
  switch (parsed.freq) {
    case "HOURLY":
      when = every === 1 && minute !== undefined ? t("settings.schedule.hourlyAt", { minute: String(minute).padStart(2, "0") }) : everyHours(every);
      break;
    case "DAILY":
      if (parsed.count === 1 && parsed.hour === undefined && parsed.minute === undefined) {
        const once = dtstart ? t("settings.schedule.onceOn", { date: calendarDate(dtstart.year, dtstart.month, dtstart.day) }) : t("settings.schedule.once");
        return atTime(once, hour, minute);
      }
      when = atTime(everyDays(every), hour, minute);
      break;
    case "WEEKLY": {
      const days = parsed.days ?? (dtstart ? [weekdayOf(dtstart.year, dtstart.month, dtstart.day)] : undefined);
      const only = (set: readonly RruleDay[]) => every === 1 && days?.length === set.length && set.every((day) => days.includes(day));
      const base = only(WORK_WEEK)
        ? t("settings.schedule.everyWeekday")
        : only(["SA", "SU"])
          ? t("settings.schedule.everyWeekend")
          : days
            ? everyWeeksOn(every, formatList(days.map((day) => weekdayName(RRULE_DAYS.indexOf(day)))))
            : everyWeeks(every);
      when = atTime(base, hour, minute);
      break;
    }
    case "MONTHLY": {
      const day = parsed.monthDay ?? dtstart?.day;
      when = atTime(day === undefined ? everyMonths(every) : everyMonthsOn(every, day), hour, minute);
      break;
    }
    case "YEARLY": {
      const month = parsed.month ?? dtstart?.month;
      const day = parsed.monthDay ?? dtstart?.day;
      if (every !== 1 || month === undefined || day === undefined) return raw;
      when = atTime(t("settings.schedule.everyYearOn", { date: yearDay(month, day) }), hour, minute);
      break;
    }
  }
  if (parsed.count === 1) return t("settings.schedule.timesOnce", { when });
  if (parsed.count !== undefined) return t("settings.schedule.times", { when, count: parsed.count });
  if (parsed.until !== undefined) {
    const last = untilDate(parsed.until, timeZone);
    return t("settings.schedule.until", { when, date: calendarDate(last.year, last.month, last.day) });
  }
  return when;
}

/** "Every weekday at 9:00 AM · Asia/Seoul", "Every 6 hours". Cron and RRULE without a zone run in UTC. */
export function describeSchedule(schedule: RoutineSchedule): string {
  if (schedule.type === "interval") return describeInterval(schedule.secs);
  if (schedule.type === "rrule") {
    const zone = schedule.timezone || "UTC";
    return `${describeRrule(schedule.rule, schedule.start, zone)} · ${zone}`;
  }
  return `${describeCron(schedule.expr)} · ${schedule.timezone || "UTC"}`;
}
