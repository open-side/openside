import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Routine } from "../../api/types";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { chooseOption } from "../../test/select";
import { ToastProvider } from "../../ui";
import { formFromSchedule, untilStamp } from "./routineSchedule";
import { RoutinesSettings } from "./Routines";

const inTwoHours = () => new Date(Date.now() + 2 * 3_600_000 + 60_000).toISOString();

function daemon() {
  const routines: Routine[] = [
    {
      id: "r1",
      name: "Morning digest",
      schedule: { type: "rrule", rule: "FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9;BYMINUTE=0;UNTIL=20261231T145959Z", timezone: "Asia/Seoul", start: "2026-11-06T09:00:00" },
      action: { type: "new_session", prompt: "Summarize my inbox" },
      enabled: true,
      catch_up: false,
      owner_agent_id: null,
      created_at: "2026-11-01T00:00:00Z",
      next_run_at: inTwoHours(),
    },
    {
      id: "r2",
      name: "Old digest",
      schedule: { type: "cron", expr: "0 8 * * *", timezone: "Europe/Berlin" },
      action: { type: "new_session", prompt: "Old prompt" },
      enabled: false,
      catch_up: false,
      owner_agent_id: null,
      created_at: "2026-10-01T00:00:00Z",
    },
    {
      id: "r3",
      name: "Launch reminder",
      schedule: { type: "rrule", rule: "FREQ=DAILY;COUNT=1", timezone: "Asia/Seoul", start: "2026-11-01T10:00:00" },
      action: { type: "new_session", prompt: "Remind me about the launch" },
      enabled: true,
      catch_up: false,
      owner_agent_id: null,
      created_at: "2026-10-30T00:00:00Z",
    },
    {
      id: "r4",
      name: "Check in",
      schedule: { type: "interval", secs: 3600 },
      action: { type: "heartbeat", session_id: "s1", prompt: "Any news?" },
      enabled: false,
      paused_reason: "target_missing",
      catch_up: false,
      owner_agent_id: null,
      created_at: "2026-10-02T00:00:00Z",
    },
  ];
  const state = { failPatch: false, failCreate: false };
  let nextId = 5;
  const fake = fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    const [, , , id, verb] = url.pathname.split("/");
    if (url.pathname === "/v1/routines" && call.method === "GET") return json(200, { routines });
    if (url.pathname === "/v1/routines/suggestions") {
      return json(200, {
        suggestions: [
          {
            name: "Weekly report",
            prompt: "Draft the weekly report",
            schedule: { type: "cron", expr: "0 9 * * 1" },
            occurrences: 4,
            session_ids: ["s1"],
            first_seen: "2026-09-01T09:00:00Z",
            last_seen: "2026-09-22T09:00:00Z",
          },
        ],
      });
    }
    if (url.pathname === "/v1/routines" && call.method === "POST") {
      if (state.failCreate) return json(400, { code: "invalid_request", message: "recurrence rule never fires" });
      const body = call.body as Pick<Routine, "name" | "schedule" | "action">;
      const routine: Routine = { ...body, id: `r${nextId++}`, enabled: true, catch_up: false, owner_agent_id: null, created_at: "2026-11-06T00:00:00Z", next_run_at: inTwoHours() };
      routines.push(routine);
      return json(201, routine);
    }
    const routine = routines.find((candidate) => candidate.id === id);
    if (!routine) return json(404, { code: "not_found", message: "nope" });
    if (verb === "run") return json(200, { id: 7, routine_id: id, scheduled_at: "2026-11-06T00:00:00Z", status: "started" });
    if (verb === "runs") {
      return json(200, {
        runs: [
          { id: 6, routine_id: id, scheduled_at: "2026-11-05T00:00:00Z", status: "skipped_overlap", detail: "previous run active" },
          { id: 5, routine_id: id, scheduled_at: "2026-11-04T00:00:00Z", status: "completed", finished_at: "2026-11-04T00:05:00Z", session_id: "s9" },
        ],
      });
    }
    if (call.method === "PATCH") {
      if (state.failPatch) return json(500, { code: "internal", message: "database is busy" });
      Object.assign(routine, call.body);
      return json(200, routine);
    }
    if (call.method === "DELETE") {
      routines.splice(routines.indexOf(routine), 1);
      return json(200, routine);
    }
    return json(200, routine);
  });
  return { ...fake, state };
}

function setup() {
  const fake = daemon();
  render(
    <MemoryRouter>
      <ClientContext.Provider value={fake.client}>
        <ToastProvider>
          <RoutinesSettings />
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

const row = async (name: string) => {
  const list = await screen.findByRole("list", { name: "Routines" });
  return within(list).getByText(name).closest("li")!;
};

async function openMenu(user: UserEvent, name: string, item: string) {
  await user.click(within(await row(name)).getByRole("button", { name: `Options for ${name}` }));
  await user.click(await screen.findByRole("menuitem", { name: item }));
}

const set = (element: HTMLElement, value: string) => fireEvent.change(element, { target: { value } });

async function retype(user: UserEvent, element: HTMLElement, value: string) {
  await user.clear(element);
  await user.type(element, value);
}

async function newRoutine(user: UserEvent, name: string, prompt: string) {
  await user.click(await screen.findByRole("button", { name: "Create" }));
  const dialog = await screen.findByRole("dialog", { name: "New routine" });
  await user.type(within(dialog).getByLabelText("Name"), name);
  await user.type(within(dialog).getByLabelText("Prompt"), prompt);
  return dialog;
}

const posted = (calls: Call[]) => calls.filter((call) => call.method === "POST" && call.path === "/v1/routines").at(-1)?.body;
const patches = (calls: Call[], id: string) => calls.filter((call) => call.method === "PATCH" && call.path === `/v1/routines/${id}`).map((call) => call.body);

// New routines default to the browser's zone; pin it so request bodies are exact.
const resolvedOptions = Intl.DateTimeFormat.prototype.resolvedOptions;
beforeEach(() => {
  vi.spyOn(Intl.DateTimeFormat.prototype, "resolvedOptions").mockImplementation(function (this: Intl.DateTimeFormat) {
    return { ...resolvedOptions.call(this), timeZone: "Asia/Seoul" };
  });
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("Settings › Routines", () => {
  it("lists routines with their schedule, next run, Paused and Ended, then + Create", async () => {
    setup();
    expect(await screen.findByRole("region", { name: "My routines" })).toBeInTheDocument();
    const digest = await row("Morning digest");
    expect(within(digest).getByText(/^Every Monday and Friday at 9:00\sAM, until Dec 31, 2026 · Asia\/Seoul · next in 2 hours$/)).toBeInTheDocument();
    expect(within(digest).getByRole("switch", { name: "Enable Morning digest" })).toBeChecked();

    const old = await row("Old digest");
    expect(within(old).getByText("Paused")).toBeInTheDocument();
    expect(within(old).getByText(/^Every day at 8:00\sAM · Europe\/Berlin$/)).toBeInTheDocument();
    expect(within(old).getByRole("switch", { name: "Enable Old digest" })).not.toBeChecked();

    const launch = await row("Launch reminder");
    expect(within(launch).getByText("Ended")).toBeInTheDocument();
    expect(within(launch).getByText(/^Once on Nov 1, 2026 at 10:00\sAM · Asia\/Seoul$/)).toBeInTheDocument();

    expect(within(await row("Check in")).getByText("Paused: conversation gone")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create" })).toBeInTheDocument();
    expect(screen.queryByText(/Free plan|Upgrade/)).toBeNull();
  });

  it("creates a daily routine", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const dialog = await newRoutine(user, "Standup notes", "Collect yesterday's notes");
    expect(within(dialog).getByRole("combobox", { name: "Repeat" })).toHaveTextContent("Daily");
    expect(within(dialog).getByRole("combobox", { name: "Time zone" })).toHaveTextContent("Asia/Seoul");
    set(within(dialog).getByLabelText("Starts"), "2026-11-09");
    set(within(dialog).getByLabelText("Time"), "09:30");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Standup notes scheduled.");
    expect(posted(calls)).toEqual({
      name: "Standup notes",
      schedule: { type: "rrule", rule: "FREQ=DAILY;BYHOUR=9;BYMINUTE=30", timezone: "Asia/Seoul", start: "2026-11-09T09:30:00" },
      action: { type: "new_session", prompt: "Collect yesterday's notes" },
    });
    expect(await row("Standup notes")).toBeInTheDocument();
  });

  it("creates a weekly routine on chosen days that ends after a number of runs", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const dialog = await newRoutine(user, "Gym", "Book a slot");
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Repeat" }), "Weekly");
    await retype(user, within(dialog).getByLabelText("Repeat every"), "2");
    const days = within(dialog).getByRole("group", { name: "Repeat on" });
    for (const day of ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]) {
      const chip = within(days).getByRole("button", { name: day });
      const want = day === "Monday" || day === "Thursday";
      if ((chip.getAttribute("aria-pressed") === "true") !== want) await user.click(chip);
    }
    set(within(dialog).getByLabelText("Starts"), "2026-11-09");
    set(within(dialog).getByLabelText("Time"), "18:00");
    await user.click(within(within(dialog).getByRole("radiogroup", { name: "Ends" })).getByRole("radio", { name: "After" }));
    await retype(user, within(dialog).getByLabelText("Number of runs"), "4");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() =>
      expect(posted(calls)).toEqual({
        name: "Gym",
        schedule: { type: "rrule", rule: "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH;BYHOUR=18;BYMINUTE=0;COUNT=4", timezone: "Asia/Seoul", start: "2026-11-09T18:00:00" },
        action: { type: "new_session", prompt: "Book a slot" },
      }),
    );
  });

  it("creates a one-time routine in another time zone", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const dialog = await newRoutine(user, "Call the bank", "Remind me to call the bank");
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Repeat" }), "Once");
    expect(within(dialog).queryByRole("radiogroup", { name: "Ends" })).toBeNull();
    expect(within(dialog).queryByLabelText("Repeat every")).toBeNull();
    set(within(dialog).getByLabelText("Date"), "2026-11-20");
    set(within(dialog).getByLabelText("Time"), "07:45");
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Time zone" }), "Europe/Berlin");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() =>
      expect(posted(calls)).toEqual({
        name: "Call the bank",
        schedule: { type: "rrule", rule: "FREQ=DAILY;COUNT=1", timezone: "Europe/Berlin", start: "2026-11-20T07:45:00" },
        action: { type: "new_session", prompt: "Remind me to call the bank" },
      }),
    );
  });

  it("creates a monthly routine that ends on a date, written as UNTIL in UTC", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const dialog = await newRoutine(user, "Rent", "Pay the rent");
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Repeat" }), "Monthly");
    await retype(user, within(dialog).getByLabelText("Day of the month"), "31");
    set(within(dialog).getByLabelText("Starts"), "2026-11-30");
    set(within(dialog).getByLabelText("Time"), "07:00");
    await user.click(within(dialog).getByRole("radio", { name: "On date" }));
    set(within(dialog).getByLabelText("Last day"), "2027-01-31");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() =>
      expect(posted(calls)).toEqual({
        name: "Rent",
        schedule: { type: "rrule", rule: "FREQ=MONTHLY;BYMONTHDAY=31;BYHOUR=7;BYMINUTE=0;UNTIL=20270131T145959Z", timezone: "Asia/Seoul", start: "2026-11-30T07:00:00" },
        action: { type: "new_session", prompt: "Pay the rent" },
      }),
    );
  });

  it("creates a custom RRULE routine and shows the daemon's refusal in the dialog", async () => {
    const user = userEvent.setup();
    const fake = setup();
    const dialog = await newRoutine(user, "Board meeting", "Prepare the board deck");
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Repeat" }), "Custom");
    expect(within(dialog).queryByRole("radiogroup", { name: "Ends" })).toBeNull();
    await user.type(within(dialog).getByLabelText("Recurrence rule (RRULE)"), " FREQ=MONTHLY;BYDAY=1MO;BYHOUR=8;BYMINUTE=0 ");
    set(within(dialog).getByLabelText("Starts"), "2026-12-01");
    set(within(dialog).getByLabelText("Time"), "08:00");

    fake.state.failCreate = true;
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(dialog).findByText("recurrence rule never fires")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "New routine" })).toBeInTheDocument();

    fake.state.failCreate = false;
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New routine" })).toBeNull());
    expect(posted(fake.calls)).toEqual({
      name: "Board meeting",
      schedule: { type: "rrule", rule: "FREQ=MONTHLY;BYDAY=1MO;BYHOUR=8;BYMINUTE=0", timezone: "Asia/Seoul", start: "2026-12-01T08:00:00" },
      action: { type: "new_session", prompt: "Prepare the board deck" },
    });
  });

  it("opens an RRULE routine field by field and sends only what changed", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await openMenu(user, "Morning digest", "Edit");
    let dialog = await screen.findByRole("dialog", { name: "Edit routine" });
    expect(within(dialog).getByRole("combobox", { name: "Repeat" })).toHaveTextContent("Weekly");
    expect(within(dialog).getByLabelText("Repeat every")).toHaveValue(1);
    const days = within(dialog).getByRole("group", { name: "Repeat on" });
    expect(within(days).getByRole("button", { name: "Monday" })).toHaveAttribute("aria-pressed", "true");
    expect(within(days).getByRole("button", { name: "Friday" })).toHaveAttribute("aria-pressed", "true");
    expect(within(days).getByRole("button", { name: "Tuesday" })).toHaveAttribute("aria-pressed", "false");
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-11-06");
    expect(within(dialog).getByLabelText("Time")).toHaveValue("09:00");
    expect(within(dialog).getByRole("radio", { name: "On date" })).toHaveAttribute("aria-checked", "true");
    expect(within(dialog).getByLabelText("Last day")).toHaveValue("2026-12-31");
    expect(within(dialog).getByRole("combobox", { name: "Time zone" })).toHaveTextContent("Asia/Seoul");

    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit routine" })).toBeNull());
    expect(patches(calls, "r1")).toEqual([]);

    await openMenu(user, "Morning digest", "Edit");
    dialog = await screen.findByRole("dialog", { name: "Edit routine" });
    set(within(dialog).getByLabelText("Time"), "10:00");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(patches(calls, "r1")).toEqual([
        { schedule: { type: "rrule", rule: "FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=10;BYMINUTE=0;UNTIL=20261231T145959Z", timezone: "Asia/Seoul", start: "2026-11-06T10:00:00" } },
      ]),
    );
    expect(await screen.findByRole("status")).toHaveTextContent("Morning digest saved.");
  });

  it("keeps a cron schedule on edit until another repeat is chosen", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await openMenu(user, "Old digest", "Edit");
    let dialog = await screen.findByRole("dialog", { name: "Edit routine" });
    expect(within(dialog).getByRole("combobox", { name: "Repeat" })).toHaveTextContent("Custom schedule");
    expect(within(dialog).getByText(/^Every day at 8:00\sAM · Europe\/Berlin$/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText("Time")).toBeNull();
    await retype(user, within(dialog).getByLabelText("Name"), "Berlin digest");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patches(calls, "r2")).toEqual([{ name: "Berlin digest" }]));

    await openMenu(user, "Berlin digest", "Edit");
    dialog = await screen.findByRole("dialog", { name: "Edit routine" });
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Repeat" }), "Daily");
    expect(within(dialog).getByRole("combobox", { name: "Time zone" })).toHaveTextContent("Europe/Berlin");
    set(within(dialog).getByLabelText("Starts"), "2026-11-07");
    set(within(dialog).getByLabelText("Time"), "08:00");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(patches(calls, "r2").at(-1)).toEqual({
        schedule: { type: "rrule", rule: "FREQ=DAILY;BYHOUR=8;BYMINUTE=0", timezone: "Europe/Berlin", start: "2026-11-07T08:00:00" },
      }),
    );
  });

  it("edits the prompt of a routine that writes into an existing chat without changing its target", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await openMenu(user, "Check in", "Edit");
    const dialog = await screen.findByRole("dialog", { name: "Edit routine" });
    expect(within(dialog).getByLabelText("Prompt")).toHaveAccessibleDescription("Sent into the same existing chat each time it runs.");
    await retype(user, within(dialog).getByLabelText("Prompt"), "Anything new?");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patches(calls, "r4")).toEqual([{ action: { type: "heartbeat", session_id: "s1", prompt: "Anything new?" } }]));
  });

  it("switches a routine off, and rolls the switch back when the daemon refuses", async () => {
    const user = userEvent.setup();
    const fake = setup();
    const toggle = within(await row("Morning digest")).getByRole("switch", { name: "Enable Morning digest" });

    fake.state.failPatch = true;
    await user.click(toggle);
    expect(await screen.findByText("database is busy")).toBeInTheDocument();
    await waitFor(async () => expect(within(await row("Morning digest")).getByRole("switch", { name: "Enable Morning digest" })).toBeChecked());
    expect(patches(fake.calls, "r1")).toEqual([{ enabled: false }]);

    fake.state.failPatch = false;
    await user.click(within(await row("Morning digest")).getByRole("switch", { name: "Enable Morning digest" }));
    expect(await within(await row("Morning digest")).findByText("Paused")).toBeInTheDocument();
    expect(within(await row("Morning digest")).getByRole("switch", { name: "Enable Morning digest" })).not.toBeChecked();
    expect(patches(fake.calls, "r1")).toEqual([{ enabled: false }, { enabled: false }]);
  });

  it("runs a routine now", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await openMenu(user, "Morning digest", "Run now");
    expect(await screen.findByRole("status")).toHaveTextContent("Morning digest: run started");
    expect(calls.some((call) => call.method === "POST" && call.path === "/v1/routines/r1/run")).toBe(true);
  });

  it("shows a routine's run history with links to the chats", async () => {
    const user = userEvent.setup();
    setup();
    await openMenu(user, "Morning digest", "History");
    const dialog = await screen.findByRole("dialog", { name: "Runs of Morning digest" });
    const runs = await within(dialog).findByRole("list", { name: "Runs of Morning digest" });
    const [skipped, completed] = within(runs).getAllByRole("listitem");
    expect(skipped).toHaveTextContent("Skipped: still running");
    expect(skipped).toHaveTextContent("previous run active");
    expect(completed).toHaveTextContent("Completed");
    expect(within(completed!).getByRole("link", { name: "Open conversation" })).toHaveAttribute("href", "/sessions/s9");
  });

  it("deletes a routine after confirming", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await openMenu(user, "Launch reminder", "Delete");
    const confirm = await screen.findByRole("dialog", { name: "Delete routine?" });
    expect(confirm).toHaveTextContent("Launch reminder stops running.");
    await user.click(within(confirm).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByText("Launch reminder")).toBeNull());
    expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/routines/r3")).toBe(true);
  });

  it("creates a routine from a suggestion, keeping its guessed schedule", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const suggestions = await screen.findByRole("region", { name: /Suggestions/ });
    expect(within(suggestions).getByText(/seen 4 times/)).toBeInTheDocument();
    await user.click(within(suggestions).getByRole("button", { name: "Use" }));
    const dialog = await screen.findByRole("dialog", { name: "New routine" });
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Weekly report");
    expect(within(dialog).getByRole("combobox", { name: "Repeat" })).toHaveTextContent("Custom schedule");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() =>
      expect(posted(calls)).toEqual({
        name: "Weekly report",
        schedule: { type: "cron", expr: "0 9 * * 1" },
        action: { type: "new_session", prompt: "Draft the weekly report" },
      }),
    );
  });
});

describe("routine schedule form", () => {
  it("writes UNTIL as the last second of the chosen day in the routine's zone", () => {
    expect(untilStamp("2026-11-30", "Asia/Seoul")).toBe("20261130T145959Z");
    expect(untilStamp("2026-11-30", "America/New_York")).toBe("20261201T045959Z");
    // Summer time in New York is UTC-4.
    expect(untilStamp("2026-07-04", "America/New_York")).toBe("20260705T035959Z");
  });

  it("reads hourly, weekday and unknown rules back", () => {
    expect(formFromSchedule({ type: "rrule", rule: "FREQ=HOURLY;INTERVAL=4", timezone: "UTC", start: "2026-11-06T08:30:00" })).toMatchObject({ repeat: "hourly", interval: "4", date: "2026-11-06", time: "08:30" });
    expect(formFromSchedule({ type: "rrule", rule: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0;COUNT=20", timezone: "UTC" })).toMatchObject({ repeat: "weekdays", time: "09:00", ends: "after", count: "20" });
    expect(formFromSchedule({ type: "rrule", rule: "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=1", timezone: "UTC", start: "2026-03-01T09:00:00" })).toMatchObject({ repeat: "custom", rule: "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=1" });
  });
});
