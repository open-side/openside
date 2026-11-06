import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Action, AgentSummary, Card, HireDraft } from "../api/types";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { agentSummary } from "../test/agents";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { mergeHireDraft, TeamActionsProvider, useOptionalTeamActions } from "./TeamActions";

afterEach(cleanup);

const chief = agentSummary("chief", "openside");
const mina = agentSummary("mina", "Mina", { home_session_id: "s-mina-home" });

function Trigger({ target }: { target: Action["target"] }) {
  const team = useOptionalTeamActions();
  const location = useLocation();
  const action = { id: "a", label: "Go", style: "secondary", target } as Action;
  return (
    <>
      <button onClick={() => void team?.onCardAction(action, { ticket_id: null } as unknown as Card)}>Go</button>
      <output aria-label="path">{location.pathname}</output>
    </>
  );
}

function setup(target: Action["target"], agents: AgentSummary[] = [chief, mina]) {
  const daemon = fakeClient((call: Call) => (call.path.endsWith("/retire") ? json(200, { ...mina, status: "retired" }) : json(404, { code: "not_found", message: "not found" })));
  const refresh = vi.fn();
  render(
    <ClientContext.Provider value={daemon.client}>
      <AgentsContext.Provider value={{ agents, error: null, offline: false, refresh }}>
        <ToastProvider>
          <MemoryRouter initialEntries={["/sessions/s1"]}>
            <TeamActionsProvider>
              <Trigger target={target} />
            </TeamActionsProvider>
          </MemoryRouter>
        </ToastProvider>
      </AgentsContext.Provider>
    </ClientContext.Provider>,
  );
  return { ...daemon, refresh, path: () => screen.getByRole("status", { name: "path" }).textContent };
}

describe("team card actions", () => {
  it("open_thread opens the teammate's home chat", async () => {
    const user = userEvent.setup();
    const { path } = setup({ type: "open_thread", agent_id: "mina", anchor: null });
    await user.click(screen.getByRole("button", { name: "Go" }));
    expect(path()).toBe("/sessions/s-mina-home");
  });

  it("open_thread for someone not on the roster falls back to Chats", async () => {
    const user = userEvent.setup();
    const { path } = setup({ type: "open_thread", agent_id: "gone", anchor: null });
    await user.click(screen.getByRole("button", { name: "Go" }));
    expect(path()).toBe("/chats");
  });

  it("retire opens the hand-over dialog and retires to the chosen successor", async () => {
    const user = userEvent.setup();
    const daemon = setup({ type: "team_command", agent_id: "mina", command: "retire" });
    await user.click(screen.getByRole("button", { name: "Go" }));
    const dialog = await screen.findByRole("dialog", { name: "Retire Mina?" });
    await user.click(within(dialog).getByRole("button", { name: "Retire" }));
    await waitFor(() => expect(daemon.calls.find((call) => call.method === "POST")).toBeDefined());
    const retire = daemon.calls.find((call) => call.method === "POST")!;
    expect(retire.path).toBe("/v1/agents/mina/retire");
    expect(retire.body).toEqual({ successor: "chief", pause_routines: false, revert_hire: false });
    expect(daemon.refresh).toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Retire Mina?" })).toBeNull());
  });
});

describe("mergeHireDraft", () => {
  const draft = { name: "Mina", initials: "M", color: "#2f6fed", persona: { role: "Watches prices", voice: "", responsibilities: ["Weekly check"], boundaries: [] } } as unknown as HireDraft;

  it("keeps the proposal when nothing was edited", () => {
    expect(mergeHireDraft(draft, { name: "Mina", role: "Watches prices" })).toBe(draft);
    expect(mergeHireDraft(draft, { name: "  " })).toBe(draft);
    expect(mergeHireDraft(draft, undefined)).toBe(draft);
  });

  it("takes a new name and lets the daemon derive initials again", () => {
    expect(mergeHireDraft(draft, { name: " Noa " })).toEqual({ ...draft, name: "Noa", initials: null });
  });

  it("takes a new role into the persona and keeps the rest", () => {
    expect(mergeHireDraft(draft, { role: "Plans trips" })).toEqual({ ...draft, persona: { ...draft.persona, role: "Plans trips" } });
  });
});
