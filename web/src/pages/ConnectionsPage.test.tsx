import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { describe, expect, it, vi } from "vitest";
import type { AgentSummary, Connection, ConnectionAuditRecord, ConnectionList, ReloginResult } from "../api/types";
import { AgentsContext } from "../hooks/useAgents";
import { ClientContext } from "../context";
import { ToastProvider } from "../ui";
import { chooseOption } from "../test/select";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ConnectionsPage } from "./ConnectionsPage";

const NOW = Date.now();
const ago = (ms: number) => new Date(NOW - ms).toISOString();

const AGENTS = [
  { id: "chief", kind: "chief", name: "openside", initials: "o", color: "#2f6fed", status: "active" },
  { id: "mina", kind: "teammate", name: "Mina", initials: "M", color: "#a34dff", status: "active" },
  { id: "old", kind: "teammate", name: "Retired", initials: "R", color: "#888888", status: "retired" },
] as unknown as AgentSummary[];

function connection(partial: Partial<Connection> & Pick<Connection, "id" | "site">): Connection {
  return {
    profile: "agent",
    identity_source: "none",
    status: "active",
    source: "login",
    accounts_on_site: 1,
    created_at: ago(86_400_000),
    account_space: null,
    grants: [{ agent_id: "chief", access: "allowed" }],
    ...partial,
  };
}

const SEED: ConnectionList = {
  browser_available: true,
  connections: [
    connection({ id: "c1", site: "github.com", identity: "j•••@gmail.com", identity_source: "probe", used_at: ago(2 * 3_600_000), used_by: "mina", grants: [{ agent_id: "chief", access: "allowed" }, { agent_id: "mina", access: "denied" }], vault_item_id: "github" }),
    connection({ id: "c2", site: "bank.example", status: "needs_login", source: "vault" }),
    connection({ id: "c3", site: "news.example", status: "unverified", source: "import", identity: "Jane D." }),
    connection({ id: "c4", site: "mail.example", status: "expired", account_space: "space1", source: "takeover", identity: "w•••@mail.example" }),
  ],
  candidates: [connection({ id: "k1", site: "shop.example", status: "candidate", source: "sweep" })],
};

const RECORDS: ConnectionAuditRecord[] = [
  { at: ago(60_000), connection_id: "c1", site: "github.com", agent_id: "mina", action: "fetch", detail: "GET /user" },
  { at: ago(120_000), connection_id: "c2", site: "bank.example", agent_id: "chief", action: "needs_login" },
];

function daemon(options: { forbidden?: boolean; browser?: boolean; relogin?: ReloginResult; fail?: string } = {}) {
  const list: ConnectionList = structuredClone(SEED);
  list.browser_available = options.browser ?? true;
  return fakeClient((call: Call) => {
    if (options.forbidden) return json(403, { code: "forbidden", message: "forbidden" });
    const url = new URL(call.path, "http://daemon.test");
    const key = `${call.method} ${url.pathname}`;
    if (options.fail && key !== "GET /v1/connections" && key !== "GET /v1/connections/audit") return json(409, { code: options.fail, message: options.fail });
    switch (key) {
      case "GET /v1/connections":
        return json(200, list);
      case "GET /v1/connections/audit": {
        const only = url.searchParams.get("connection");
        return json(200, { records: RECORDS.filter((record) => !only || record.connection_id === only) });
      }
      case "POST /v1/connections/c2/relogin":
        return json(200, options.relogin ?? { status: "signed_in" });
      case "POST /v1/connections/c3/check":
        return json(200, { ...list.connections[2], status: "active" });
      case "POST /v1/connections/spaces":
        return json(200, { space_id: "space2", connection_id: "c9", result: options.relogin ?? { status: "signed_in" } });
    }
    if (key.startsWith("POST /v1/connections/candidates/")) return new Response(null, { status: 204 });
    if (key.endsWith("/revoke")) return new Response(null, { status: 204 });
    if (call.method === "PATCH") return json(200, list.connections[0]);
    return json(404, { code: "not_found", message: "not_found" });
  });
}

function Where() {
  const location = useLocation();
  return <output data-testid="where">{location.pathname + location.search}</output>;
}

function setup(options: Parameters<typeof daemon>[0] = {}) {
  const fake = daemon(options);
  render(
    <MemoryRouter initialEntries={["/connections"]}>
      <ClientContext.Provider value={fake.client}>
        <AgentsContext.Provider value={{ agents: AGENTS, error: null, offline: false, refresh: () => undefined }}>
          <ToastProvider>
            <ConnectionsPage />
            <Where />
          </ToastProvider>
        </AgentsContext.Provider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

async function card(site: string) {
  const list = await screen.findByRole("list", { name: "Connections" });
  return within(list).getByText(site).closest("article") as HTMLElement;
}

describe("ConnectionsPage", () => {
  it("shows each site with its masked account, status, last use and per-agent access", async () => {
    setup();
    const github = await card("github.com");
    expect(within(github).getByText("j•••@gmail.com")).toBeInTheDocument();
    expect(within(github).getByText("Active")).toBeInTheDocument();
    expect(within(github).getByText(/Last used 2 hours ago by Mina/)).toBeInTheDocument();
    expect(within(github).getByText("Signed in during a chat")).toBeInTheDocument();
    expect(within(github).getByRole("button", { name: /1 allowed · 1 blocked/ })).toBeInTheDocument();

    const bank = await card("bank.example");
    expect(within(bank).getByText("Account not identified yet")).toBeInTheDocument();
    expect(within(bank).getByText("Sign in again", { selector: ".badge" })).toBeInTheDocument();
    expect(within(bank).getByText("Not used by an agent yet")).toBeInTheDocument();
    expect(within(bank).getByRole("button", { name: /1 allowed · 1 ask first/ })).toBeInTheDocument();

    expect(within(await card("news.example")).getByText("Not checked")).toBeInTheDocument();
    const mail = await card("mail.example");
    expect(within(mail).getByText("Expired")).toBeInTheDocument();
    expect(within(mail).getByText("Separate browser profile")).toBeInTheDocument();
  });

  it("signs in again silently when the vault can finish it", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(within(await card("bank.example")).getByRole("button", { name: "Sign in again" }));
    await vi.waitFor(() => expect(calls.some((call) => call.path === "/v1/connections/c2/relogin")).toBe(true));
    expect(await screen.findByText("Signed in to bank.example.")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent("/connections");
  });

  it("jumps to the sign-in ticket when the person has to finish it", async () => {
    const user = userEvent.setup();
    setup({ relogin: { status: "needs_you", ticket: { session_id: "s_home", ticket_id: "t_1" } } });
    await user.click(within(await card("bank.example")).getByRole("button", { name: "Sign in again" }));
    await vi.waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent("/sessions/s_home?ticket=t_1"));
  });

  it("signs out only after confirmation and can delete the saved login too", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const github = await card("github.com");
    await user.click(within(github).getByRole("button", { name: "Options for github.com" }));
    await user.click(await screen.findByRole("menuitem", { name: /Sign out/ }));
    const dialog = await screen.findByRole("form", { name: "Sign out" });
    expect(calls.some((call) => call.path.endsWith("/revoke"))).toBe(false);
    expect(screen.getByText(/Every account signed in to github.com there is signed out/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("checkbox", { name: "Also delete the saved login from the Vault" }));
    await user.click(within(dialog).getByRole("button", { name: "Sign out" }));
    await vi.waitFor(() => expect(calls.find((call) => call.path === "/v1/connections/c1/revoke")?.body).toEqual({ sign_out: true, delete_vault_item: true }));
  });

  it("forgets a connection without touching the browser", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(within(await card("bank.example")).getByRole("button", { name: "Options for bank.example" }));
    await user.click(await screen.findByRole("menuitem", { name: /Forget/ }));
    const dialog = await screen.findByRole("form", { name: "Forget" });
    expect(within(dialog).queryByRole("checkbox")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Forget" }));
    await vi.waitFor(() => expect(calls.find((call) => call.path === "/v1/connections/c2/revoke")?.body).toEqual({ sign_out: false, delete_vault_item: false }));
  });

  it("removes a separate profile with one choice", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(within(await card("mail.example")).getByRole("button", { name: "Options for mail.example" }));
    expect(screen.queryByRole("menuitem", { name: /Forget/ })).toBeNull();
    await user.click(await screen.findByRole("menuitem", { name: /Remove account/ }));
    await user.click(within(await screen.findByRole("form", { name: "Remove account" })).getByRole("button", { name: "Remove" }));
    await vi.waitFor(() => expect(calls.find((call) => call.path === "/v1/connections/c4/revoke")?.body).toMatchObject({ sign_out: true }));
  });

  it("changes an agent's access from the access dialog", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(within(await card("github.com")).getByRole("button", { name: /1 allowed · 1 blocked/ }));
    const dialog = await screen.findByRole("dialog", { name: "Agent access" });
    expect(within(dialog).queryByText("Retired")).toBeNull();
    expect(within(dialog).getByLabelText("Mina's access to github.com")).toHaveTextContent("Blocked");
    await chooseOption(user, within(dialog).getByLabelText("Mina's access to github.com"), "Ask me first");
    await vi.waitFor(() => expect(calls.find((call) => call.method === "PATCH")).toMatchObject({ path: "/v1/connections/c1", body: { grants: [{ agent_id: "mina", access: "ask" }] } }));
  });

  it("keeps possibly-signed-in sites collapsed until asked, then checks or dismisses them", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await card("github.com");
    expect(screen.queryByText("shop.example")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Possibly signed in (1)" }));
    const list = await screen.findByRole("list", { name: "Possibly signed in" });
    await user.click(within(list).getByRole("button", { name: "Dismiss" }));
    await vi.waitFor(() => expect(calls.some((call) => call.path === "/v1/connections/candidates/k1/dismiss")).toBe(true));
  });

  it("filters the activity list to one connection from its menu", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const audit = await screen.findByRole("list", { name: "Activity records" });
    expect(within(audit).getAllByRole("listitem")).toHaveLength(2);
    expect(within(audit).getByText("GET /user")).toBeInTheDocument();
    window.HTMLElement.prototype.scrollIntoView = vi.fn();
    await user.click(within(await card("bank.example")).getByRole("button", { name: "Options for bank.example" }));
    await user.click(await screen.findByRole("menuitem", { name: "Activity" }));
    await vi.waitFor(() => expect(calls.at(-1)?.path).toBe("/v1/connections/audit?connection=c2&limit=100"));
    await vi.waitFor(() => expect(within(screen.getByRole("list", { name: "Activity records" })).getAllByRole("listitem")).toHaveLength(1));
  });

  it("creates a separate account profile and follows its sign-in ticket", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ relogin: { status: "needs_you", ticket: { session_id: "s_home", ticket_id: "t_2" } } });
    await user.click(within(await card("github.com")).getByRole("button", { name: "Options for github.com" }));
    await user.click(await screen.findByRole("menuitem", { name: /Add another account/ }));
    const form = await screen.findByRole("form", { name: "Add another account" });
    expect(within(form).getByLabelText("Site")).toHaveValue("github.com");
    expect(within(form).getByText(/200–400 MB/)).toBeInTheDocument();
    await user.click(within(form).getByRole("button", { name: "Create and sign in" }));
    await vi.waitFor(() => expect(calls.find((call) => call.path === "/v1/connections/spaces")?.body).toEqual({ site: "github.com" }));
    await vi.waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent("/sessions/s_home?ticket=t_2"));
  });

  it("explains errors in plain words", async () => {
    const user = userEvent.setup();
    setup({ fail: "connection_browser_unavailable" });
    await user.click(within(await card("bank.example")).getByRole("button", { name: "Sign in again" }));
    expect(await screen.findByText(/agent browser is not running/)).toBeInTheDocument();
  });

  it("disables checks and sign-ins while the agent browser is unavailable", async () => {
    setup({ browser: false });
    expect(await screen.findByText(/agent browser is not available/)).toBeInTheDocument();
    expect(within(await card("bank.example")).getByRole("button", { name: "Sign in again" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Add another account" })).toBeNull();
  });

  it("lets a device without the admin scope ask for it", async () => {
    setup({ forbidden: true });
    expect((await screen.findAllByRole("button", { name: "Ask for access" })).length).toBeGreaterThan(0);
  });
});
