import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import type { Connection, ConnectionList } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { chooseOption } from "../test/select";
import { AgentConnections } from "./AgentConnections";

function connection(partial: Partial<Connection> & Pick<Connection, "id" | "site">): Connection {
  return {
    profile: "agent",
    identity_source: "none",
    status: "active",
    source: "login",
    accounts_on_site: 1,
    created_at: new Date().toISOString(),
    account_space: null,
    grants: [{ agent_id: "chief", access: "allowed" }, { agent_id: "mina", access: "denied" }],
    ...partial,
  };
}

function setup(list: ConnectionList) {
  const fake = fakeClient((call: Call) => {
    if (call.method === "GET" && call.path === "/v1/connections") return json(200, list);
    if (call.method === "PATCH") return json(200, list.connections[0]);
    return json(404, { code: "not_found", message: "not_found" });
  });
  render(
    <MemoryRouter>
      <ClientContext.Provider value={fake.client}>
        <AgentConnections agentId="mina" agentName="Mina" />
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

describe("AgentConnections", () => {
  it("shows the teammate's access per site and saves a change at once", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ browser_available: true, candidates: [], connections: [connection({ id: "c1", site: "github.com", identity: "j•••@gmail.com" }), connection({ id: "c2", site: "bank.example", grants: [] })] });
    const select = await screen.findByLabelText("Mina's access to github.com (j•••@gmail.com)");
    expect(select).toHaveTextContent("Blocked");
    expect(screen.getByLabelText("Mina's access to bank.example")).toHaveTextContent("Ask me first");
    await chooseOption(user, select, "Allowed");
    await vi.waitFor(() => expect(calls.find((call) => call.method === "PATCH")).toMatchObject({ path: "/v1/connections/c1", body: { grants: [{ agent_id: "mina", access: "allowed" }] } }));
    expect(screen.getByRole("link", { name: "Manage connections" })).toHaveAttribute("href", "/connections");
  });

  it("says so when nothing is signed in", async () => {
    setup({ browser_available: true, candidates: [], connections: [] });
    expect(await screen.findByText("No sites are signed in yet.")).toBeInTheDocument();
  });
});
