import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Routine } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { RoutinesPage } from "./RoutinesPage";

function daemon() {
  const routines: Routine[] = [
    {
      id: "r1",
      name: "Morning digest",
      schedule: { type: "cron", expr: "0 8 * * *", timezone: "Europe/Berlin" },
      action: { type: "new_session", prompt: "Summarize my inbox" },
      enabled: true,
      catch_up: false,
      created_at: "2026-10-01T00:00:00Z",
      next_run_at: "2026-10-13T06:00:00Z",
    },
  ];
  let nextId = 2;
  return fakeClient((call: Call) => {
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
      const body = call.body as Pick<Routine, "name" | "schedule" | "action">;
      const routine: Routine = { ...body, id: `r${nextId++}`, enabled: true, catch_up: false, created_at: "2026-10-12T00:00:00Z" };
      routines.push(routine);
      return json(201, routine);
    }
    const routine = routines.find((r) => r.id === id);
    if (!routine) return json(404, { code: "not_found", message: "nope" });
    if (verb === "run") return json(200, { id: 7, routine_id: id, scheduled_at: "2026-10-12T00:00:00Z", status: "started" });
    if (verb === "runs") {
      return json(200, {
        runs: [{ id: 6, routine_id: id, scheduled_at: "2026-10-11T06:00:00Z", status: "skipped_overlap", detail: "previous run active" }],
      });
    }
    if (call.method === "PATCH") {
      Object.assign(routine, call.body);
      return json(200, routine);
    }
    if (call.method === "DELETE") {
      routines.splice(routines.indexOf(routine), 1);
      return json(200, routine);
    }
    return json(200, routine);
  });
}

function setup() {
  const fake = daemon();
  render(
    <ClientContext.Provider value={fake.client}>
      <ToastProvider>
        <RoutinesPage />
      </ToastProvider>
    </ClientContext.Provider>,
  );
  return fake;
}

describe("RoutinesPage", () => {
  it("runs now, pauses, shows run history and deletes", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const list = await screen.findByRole("list", { name: "Routines" });
    const row = within(list).getByText("Morning digest").closest("article") as HTMLElement;
    expect(row).toHaveAttribute("data-routine", "r1");
    expect(within(row).getByText("cron 0 8 * * * (Europe/Berlin)")).toBeInTheDocument();

    await user.click(within(row).getByRole("button", { name: "Run now" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Morning digest: run started");
    expect(calls.some((c) => c.method === "POST" && c.path === "/v1/routines/r1/run")).toBe(true);

    await user.click(within(row).getByRole("button", { name: "Pause" }));
    expect(await within(row).findByText("Paused")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ enabled: false });
    expect(within(row).getByRole("button", { name: "Resume" })).toBeInTheDocument();

    await user.click(within(row).getByRole("button", { name: "Runs" }));
    const runs = await within(row).findByRole("list", { name: "Runs of Morning digest" });
    expect(within(runs).getByText("skipped_overlap")).toBeInTheDocument();
    expect(within(runs).getByText(/previous run active/)).toBeInTheDocument();

    await user.click(within(row).getByRole("button", { name: "Delete" }));
    const confirm = await screen.findByRole("dialog", { name: "Delete routine?" });
    await user.click(within(confirm).getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("No routines yet")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/routines/r1")).toBe(true);
  });

  it("creates an interval routine and one from a suggestion", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await screen.findByRole("list", { name: "Routines" });
    await user.click(screen.getByRole("button", { name: "New routine" }));
    const form = await screen.findByRole("form", { name: "New routine" });
    await user.type(within(form).getByLabelText("Name"), "Check build");
    await user.click(within(form).getByRole("radio", { name: "Fixed interval" }));
    const every = within(form).getByLabelText("Every (seconds)");
    await user.clear(every);
    await user.type(every, "900");
    await user.type(within(form).getByLabelText("Prompt for a new conversation"), "Is CI green?");
    await user.click(within(form).getByRole("button", { name: "Create" }));
    expect(await screen.findByText("Check build")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST" && c.path === "/v1/routines")?.body).toEqual({
      name: "Check build",
      schedule: { type: "interval", secs: 900 },
      action: { type: "new_session", prompt: "Is CI green?" },
    });

    const suggestions = await screen.findByRole("region", { name: "Suggested routines" });
    expect(within(suggestions).getByText(/seen 4 times/)).toBeInTheDocument();
    await user.click(within(suggestions).getByRole("button", { name: "Use" }));
    const draft = await screen.findByRole("form", { name: "Create routine from Weekly report" });
    expect(within(draft).getByLabelText("Cron (minute hour day month weekday)")).toHaveValue("0 9 * * 1");
    await user.click(within(draft).getByRole("button", { name: "Create" }));
    const list = await screen.findByRole("list", { name: "Routines" });
    expect(await within(list).findByText("Weekly report")).toBeInTheDocument();
    expect(calls.filter((c) => c.method === "POST" && c.path === "/v1/routines").at(-1)?.body).toEqual({
      name: "Weekly report",
      schedule: { type: "cron", expr: "0 9 * * 1", timezone: null },
      action: { type: "new_session", prompt: "Draft the weekly report" },
    });
  });
});
