import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { RestartNotice } from "./RestartNotice";

function setup(handler: (call: Call, polls: number) => Response | undefined) {
  let polls = 0;
  const fake = fakeClient((call) => {
    if (call.path === "/v1/daemon") polls += 1;
    return handler(call, polls) ?? json(404, { code: "not_found", message: "nope" });
  });
  const restarted = vi.fn();
  render(
    <ClientContext.Provider value={fake.client}>
      <RestartNotice onRestarted={restarted}>Saved settings apply after openside restarts.</RestartNotice>
    </ClientContext.Provider>,
  );
  return { ...fake, restarted };
}

describe("RestartNotice", () => {
  it("confirms, restarts the daemon and reconnects once boot_id changes, riding out the gap", async () => {
    const user = userEvent.setup();
    const { calls, restarted } = setup((call, polls) => {
      if (call.method === "POST" && call.path === "/v1/daemon/restart") return json(202, { method: "re_exec", boot_id: "old" });
      if (call.path === "/v1/daemon") {
        // First poll: the old process still answers; second: the port is closed; third: the new one.
        if (polls === 1) return json(200, { version: "1", boot_id: "old", started_at: "2026-10-01T00:00:00Z", restart_method: "re_exec" });
        if (polls === 2) return json(503, { code: "unavailable", message: "down" });
        return json(200, { version: "1", boot_id: "new", started_at: "2026-10-01T00:01:00Z", restart_method: "re_exec" });
      }
      return undefined;
    });

    expect(screen.getByRole("status")).toHaveTextContent("apply after openside restarts");
    await user.click(screen.getByRole("button", { name: "Restart daemon" }));
    const confirm = await screen.findByRole("dialog", { name: "Restart openside?" });
    expect(confirm).toHaveTextContent("pick up again from their saved history");
    expect(calls.some((call) => call.path === "/v1/daemon/restart")).toBe(false);
    await user.click(within(confirm).getByRole("button", { name: "Restart" }));

    expect(await screen.findByText(/Restarting openside/)).toBeInTheDocument();
    expect(await screen.findByText("openside restarted.", {}, { timeout: 6_000 })).toBeInTheDocument();
    expect(restarted).toHaveBeenCalledTimes(1);
    expect(calls.filter((call) => call.path === "/v1/daemon")).toHaveLength(3);
  }, 10_000);

  it("explains that only the computer can restart when the daemon refuses with local_access_required", async () => {
    const user = userEvent.setup();
    const { restarted } = setup((call) =>
      call.method === "POST" ? json(403, { code: "local_access_required", message: "local" }) : undefined,
    );
    await user.click(screen.getByRole("button", { name: "Restart daemon" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Restart openside?" })).getByRole("button", { name: "Restart" }));
    expect(await screen.findByText(/To restart openside, use openside on the computer that runs it/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restart daemon" })).toBeInTheDocument();
    expect(restarted).not.toHaveBeenCalled();
  });
});
