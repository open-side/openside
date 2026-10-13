import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { describe, expect, it } from "vitest";
import type { DoctorReport } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { DiagnosticsPage } from "./DiagnosticsPage";

const REPORT: DoctorReport = {
  ok: false,
  generated_at: "2026-10-12T08:00:00Z",
  groups: [
    {
      name: "Models",
      checks: [
        { id: "models.default", status: "ok", summary: "Default model is available", next_step: null, fix: null },
        {
          id: "models.keys",
          status: "fail",
          summary: "No provider has a key or login",
          next_step: "Add an API key or sign in to a subscription.",
          fix: { action: "open_models", label: "Open models", hint: "Opens Models & accounts." },
        },
      ],
    },
    {
      name: "Memory",
      checks: [
        {
          id: "memory.embeddings",
          status: "warn",
          summary: "The search model is not installed",
          next_step: null,
          fix: { action: "download_memory_model", label: "Download model", hint: "Downloads the small search model." },
        },
        {
          id: "daemon.config",
          status: "warn",
          summary: "Saved settings differ from the running daemon",
          next_step: null,
          fix: { action: "restart_daemon", label: "Restart daemon", hint: "Applies the saved settings." },
        },
      ],
    },
  ],
};

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>;
}

function setup(handler: (call: Call) => Response | undefined = () => undefined) {
  let runs = 0;
  const fake = fakeClient((call) => {
    if (call.path === "/v1/doctor") {
      runs += 1;
      return json(200, runs === 1 ? REPORT : { ...REPORT, ok: true, groups: [{ name: "Models", checks: [REPORT.groups[0]!.checks[0]!] }] });
    }
    return handler(call) ?? json(404, { code: "not_found", message: "nope" });
  });
  render(
    <MemoryRouter initialEntries={["/diagnostics"]}>
      <ClientContext.Provider value={fake.client}>
        <DiagnosticsPage />
        <Where />
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

describe("DiagnosticsPage", () => {
  it("lists every check by area with its state and counts what needs a look", async () => {
    setup();
    const models = await screen.findByRole("list", { name: "Models" });
    expect(within(models).getAllByRole("listitem")).toHaveLength(2);
    expect(within(models).getByText("Default model is available").closest("article")).toHaveTextContent("OK");
    expect(within(models).getByText("No provider has a key or login").closest("article")).toHaveTextContent("Problem");
    expect(within(models).getByText("Add an API key or sign in to a subscription. Opens Models & accounts.")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Memory" })).toBeInTheDocument();
    expect(screen.getByText("1 problem and 2 warnings to look at.")).toBeInTheDocument();
  });

  it("opens the screen a fix points at", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(await screen.findByRole("button", { name: "Open models: No provider has a key or login" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/models");
  });

  it("starts the model download from its Fix button and goes to the Memory page for the progress", async () => {
    const user = userEvent.setup();
    const { calls } = setup((call) => (call.path === "/v1/memory/model/download" ? json(202, { state: "resolving" }) : undefined));
    await user.click(await screen.findByRole("button", { name: "Download model: The search model is not installed" }));
    expect(await screen.findByTestId("where")).toHaveTextContent("/memory");
    expect(calls.find((call) => call.path === "/v1/memory/model/download")).toMatchObject({ method: "POST", body: {} });
  });

  it("asks before restarting from a Fix button, and checks again on demand", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(await screen.findByRole("button", { name: "Restart daemon: Saved settings differ from the running daemon" }));
    expect(await screen.findByRole("dialog", { name: "Restart openside?" })).toBeInTheDocument();
    expect(calls.some((call) => call.path === "/v1/daemon/restart")).toBe(false);
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));

    await user.click(screen.getByRole("button", { name: "Check again" }));
    expect(await screen.findByText("Everything checks out.")).toBeInTheDocument();
    expect(calls.filter((call) => call.path === "/v1/doctor")).toHaveLength(2);
  });

  it("asks a device without the admin scope for access", async () => {
    const fake = fakeClient((call) => (call.path === "/v1/doctor" ? json(403, { code: "forbidden", message: "no" }) : json(404, { code: "not_found", message: "x" })));
    render(
      <MemoryRouter>
        <ClientContext.Provider value={fake.client}>
          <DiagnosticsPage />
        </ClientContext.Provider>
      </MemoryRouter>,
    );
    expect(await screen.findByRole("region", { name: "Access needed" })).toBeInTheDocument();
  });
});
