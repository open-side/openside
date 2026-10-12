import type { RoutineSchedule } from "../api/types";

const DATE_TIME = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
const DATE = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
const RELATIVE = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

/** "Oct 12, 2026, 7:01 AM" from an RFC 3339 string or Unix milliseconds. */
export function formatDateTime(value: string | number): string {
  return DATE_TIME.format(new Date(value));
}

export function formatDate(value: string | number): string {
  return DATE.format(new Date(value));
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
    if (Math.abs(delta) >= size) return RELATIVE.format(Math.round(delta / size), unit);
  }
  return RELATIVE.format(0, "minute");
}

const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const ALIASES: Record<string, string> = {
  "@hourly": "Every hour",
  "@daily": "Every day at 00:00",
  "@midnight": "Every day at 00:00",
  "@weekly": "Every Sunday at 00:00",
  "@monthly": "On the 1st of every month at 00:00",
  "@yearly": "Every January 1 at 00:00",
  "@annually": "Every January 1 at 00:00",
};

/**
 * Plain-language cron for the common shapes (daily, weekdays, one weekday, hourly); anything
 * else is shown as the expression itself, which is what the person typed.
 */
function describeCron(expr: string): string {
  const alias = ALIASES[expr.trim()];
  if (alias) return alias;
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 5) return `Cron ${expr}`;
  const [minute, hour, day, month, weekday] = parts as [string, string, string, string, string];
  const numeric = /^\d+$/;
  if (!numeric.test(minute) || day !== "*" || month !== "*") return `Cron ${expr}`;
  if (hour === "*" && weekday === "*") return `Every hour at :${minute.padStart(2, "0")}`;
  if (!numeric.test(hour)) return `Cron ${expr}`;
  const time = `${hour.padStart(2, "0")}:${minute.padStart(2, "0")}`;
  if (weekday === "*") return `Every day at ${time}`;
  if (weekday === "1-5") return `Weekdays at ${time}`;
  if (weekday === "0,6" || weekday === "6,0") return `Weekends at ${time}`;
  const single = WEEKDAYS[Number(weekday) % 7];
  if (numeric.test(weekday) && single) return `Every ${single} at ${time}`;
  return `Cron ${expr}`;
}

function describeInterval(secs: number): string {
  for (const [unit, size] of [
    ["day", 86_400],
    ["hour", 3_600],
    ["minute", 60],
  ] as const) {
    if (secs % size === 0) {
      const count = secs / size;
      return count === 1 ? `Every ${unit}` : `Every ${count} ${unit}s`;
    }
  }
  return secs === 1 ? "Every second" : `Every ${secs} seconds`;
}

/** "Weekdays at 08:00 · Asia/Seoul", "Every 6 hours". Cron without a zone runs in UTC. */
export function describeSchedule(schedule: RoutineSchedule): string {
  if (schedule.type === "interval") return describeInterval(schedule.secs);
  return `${describeCron(schedule.expr)} · ${schedule.timezone || "UTC"}`;
}
