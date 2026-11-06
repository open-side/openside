import { describe, expect, it } from "vitest";
import type { RoutineSchedule } from "../api/types";
import { describeSchedule, readRrule } from "./format";

const rrule = (rule: string, timezone?: string, start?: string): RoutineSchedule => ({ type: "rrule", rule, ...(timezone ? { timezone } : {}), ...(start ? { start } : {}) });

// Intl prints "9:00 AM" with a narrow no-break space before AM; \s matches it.
describe("describeSchedule", () => {
  it("reads the rules the routine form writes", () => {
    expect(describeSchedule(rrule("FREQ=DAILY;BYHOUR=9;BYMINUTE=0", "Asia/Seoul", "2026-11-06T09:00:00"))).toMatch(/^Every day at 9:00\sAM · Asia\/Seoul$/);
    expect(describeSchedule(rrule("FREQ=DAILY;INTERVAL=3;BYHOUR=21;BYMINUTE=15"))).toMatch(/^Every 3 days at 9:15\sPM · UTC$/);
    expect(describeSchedule(rrule("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0", "Asia/Seoul"))).toMatch(/^Every weekday at 9:00\sAM · Asia\/Seoul$/);
    expect(describeSchedule(rrule("FREQ=WEEKLY;BYDAY=SU,SA;BYHOUR=10;BYMINUTE=0"))).toMatch(/^Every weekend at 10:00\sAM · UTC$/);
    expect(describeSchedule(rrule("FREQ=WEEKLY;INTERVAL=2;BYDAY=FR,MO;BYHOUR=18;BYMINUTE=30"))).toMatch(/^Every 2 weeks on Monday and Friday at 6:30\sPM · UTC$/);
    expect(describeSchedule(rrule("FREQ=MONTHLY;BYMONTHDAY=31;BYHOUR=7;BYMINUTE=5", "Europe/Berlin"))).toMatch(/^Monthly on day 31 at 7:05\sAM · Europe\/Berlin$/);
    expect(describeSchedule(rrule("FREQ=HOURLY;INTERVAL=3", "UTC", "2026-11-06T10:30:00"))).toBe("Every 3 hours · UTC");
  });

  it("takes the time and weekday the rule leaves out from the series start", () => {
    // 2026-11-06 is a Friday.
    expect(describeSchedule(rrule("FREQ=WEEKLY", "UTC", "2026-11-06T08:15:00"))).toMatch(/^Every Friday at 8:15\sAM · UTC$/);
    expect(describeSchedule(rrule("RRULE:FREQ=HOURLY", "UTC", "2026-11-06T10:30:00"))).toBe("Every hour at :30 · UTC");
    expect(describeSchedule(rrule("FREQ=MONTHLY", "UTC", "2026-11-06T08:00:00"))).toMatch(/^Monthly on day 6 at 8:00\sAM · UTC$/);
    expect(describeSchedule(rrule("FREQ=YEARLY", "UTC", "2026-11-06T08:00:00"))).toMatch(/^Every year on November 6 at 8:00\sAM · UTC$/);
  });

  it("names one-time routines and series that end", () => {
    expect(describeSchedule(rrule("FREQ=DAILY;COUNT=1", "Asia/Seoul", "2026-11-06T09:00:00"))).toMatch(/^Once on Nov 6, 2026 at 9:00\sAM · Asia\/Seoul$/);
    expect(describeSchedule(rrule("FREQ=DAILY;BYHOUR=9;BYMINUTE=0;COUNT=10"))).toMatch(/^Every day at 9:00\sAM, 10 times · UTC$/);
    expect(describeSchedule(rrule("FREQ=WEEKLY;BYDAY=TU;BYHOUR=9;BYMINUTE=0;COUNT=1"))).toMatch(/^Every Tuesday at 9:00\sAM, once · UTC$/);
    // 14:59:59Z is the last second of Nov 30 in Seoul.
    expect(describeSchedule(rrule("FREQ=DAILY;BYHOUR=9;BYMINUTE=0;UNTIL=20261130T145959Z", "Asia/Seoul"))).toMatch(/until Nov 30, 2026 · Asia\/Seoul$/);
    // In New York the same day ends after midnight UTC.
    expect(describeSchedule(rrule("FREQ=DAILY;UNTIL=20261201T045959Z", "America/New_York", "2026-11-06T09:00:00"))).toMatch(/until Nov 30, 2026 · America\/New_York$/);
  });

  it("shows any other rule as written", () => {
    expect(describeSchedule(rrule("FREQ=MONTHLY;BYDAY=1MO;BYHOUR=8;BYMINUTE=0"))).toBe("Custom: FREQ=MONTHLY;BYDAY=1MO;BYHOUR=8;BYMINUTE=0 · UTC");
    expect(describeSchedule(rrule("FREQ=DAILY;BYHOUR=9,17"))).toBe("Custom: FREQ=DAILY;BYHOUR=9,17 · UTC");
    expect(describeSchedule(rrule("FREQ=MINUTELY"))).toBe("Custom: FREQ=MINUTELY · UTC");
  });

  it("keeps describing cron and interval schedules", () => {
    expect(describeSchedule({ type: "cron", expr: "0 8 * * *", timezone: "Europe/Berlin" })).toMatch(/^Every day at 8:00\sAM · Europe\/Berlin$/);
    expect(describeSchedule({ type: "cron", expr: "30 9 * * 1-5" })).toMatch(/^Every weekday at 9:30\sAM · UTC$/);
    expect(describeSchedule({ type: "cron", expr: "0 9 * * 1" })).toMatch(/^Every Monday at 9:00\sAM · UTC$/);
    expect(describeSchedule({ type: "cron", expr: "*/5 * * * *" })).toBe("Cron */5 * * * * · UTC");
    expect(describeSchedule({ type: "cron", expr: "@weekly" })).toMatch(/^Every Sunday at 12:00\sAM · UTC$/);
    expect(describeSchedule({ type: "cron", expr: "@monthly" })).toMatch(/^Monthly on day 1 at 12:00\sAM · UTC$/);
    expect(describeSchedule({ type: "interval", secs: 3600 })).toBe("Every hour");
    expect(describeSchedule({ type: "interval", secs: 7200 })).toBe("Every 2 hours");
    expect(describeSchedule({ type: "interval", secs: 86_400 })).toBe("Every day");
    expect(describeSchedule({ type: "interval", secs: 900 })).toBe("Every 15 minutes");
  });
});

describe("readRrule", () => {
  it("reads simple rules and refuses the rest", () => {
    expect(readRrule("freq=weekly;byday=fr,mo;interval=2;until=20261231T145959Z")).toEqual({ freq: "WEEKLY", interval: 2, days: ["MO", "FR"], until: "20261231T145959Z" });
    expect(readRrule("FREQ=SECONDLY")).toBeNull();
    expect(readRrule("FREQ=DAILY;COUNT=2;UNTIL=20261231")).toBeNull();
    expect(readRrule("FREQ=DAILY;BYDAY=MO")).toBeNull();
    expect(readRrule("FREQ=WEEKLY;BYDAY=MO,MO")).toBeNull();
    expect(readRrule("FREQ=DAILY;INTERVAL=0")).toBeNull();
  });
});
