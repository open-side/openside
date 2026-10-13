const TIME = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: "long" });
const DATE = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const DATE_YEAR = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });

/** A pause this long between two messages earns a time line, as messengers do. */
const GAP_MS = 60 * 60 * 1000;

const dayStart = (at: Date) => new Date(at.getFullYear(), at.getMonth(), at.getDate()).getTime();

/** Whether a separator goes before the message at `at`: the first message, a new day, or an hour's pause. */
export function separatorNeeded(previous: string | undefined, at: string): boolean {
  const current = new Date(at);
  if (Number.isNaN(current.getTime())) return false;
  if (!previous) return true;
  const before = new Date(previous);
  if (Number.isNaN(before.getTime())) return true;
  return dayStart(before) !== dayStart(current) || current.getTime() - before.getTime() >= GAP_MS;
}

/** "Today 14:05", "Yesterday 09:12", "Monday 18:30", "Oct 3, 11:00", "Oct 3, 2025, 11:00". */
export function separatorLabel(at: string, now = new Date()): string {
  const date = new Date(at);
  const days = Math.round((dayStart(now) - dayStart(date)) / 86_400_000);
  const time = TIME.format(date);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Yesterday ${time}`;
  if (days > 1 && days < 7) return `${WEEKDAY.format(date)} ${time}`;
  return `${(date.getFullYear() === now.getFullYear() ? DATE : DATE_YEAR).format(date)}, ${time}`;
}
