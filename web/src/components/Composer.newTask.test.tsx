import "fake-indexeddb/auto";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "../api/types";
import { ClientContext } from "../context";
import { AgentTabsContext, type OwnedTab } from "../hooks/useAgentTabs";
import { AgentsContext } from "../hooks/useAgents";
import { outbox } from "../pwa/outbox";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { Composer } from "./Composer";
import { contextItems, tabLink } from "./ComposerContextMenu";
import { createSessionRequest, DEFAULT_SESSION_OPTIONS } from "./sessionOptions";

afterEach(async () => {
  cleanup();
  for (const entry of outbox.snapshot()) await outbox.discard(entry.message_id);
});

const chief = { id: "chief", name: "openside", model: null, mode: "guard", home_session_id: "chief-home" } as unknown as AgentSummary;
const CATALOG = {
  cached_for_ms: 0,
  default_model: "openai:gpt-6.1",
  fallback_models: [],
  truncated: false,
  providers: [
    {
      provider: "openai",
      pool: { provider: "openai", accounts: [] },
      models: [
        { id: "openai:gpt-6.1", label: "GPT-6.1", available: true, reason: "", accounts: [], in_catalog: true, strengths: "", tier: "reasoning" },
        { id: "openai:gpt-old", label: "GPT-old", available: false, reason: "No key for this provider", accounts: [], in_catalog: true, strengths: "", tier: "reasoning" },
      ],
    },
  ],
};
const TABS: OwnedTab[] = [
  { tab_id: "t1", title: "Flights to Lisbon", url: "https://flights.example/lis", opened_by: "agent", owner_session_id: "chief-home" },
  { tab_id: "t2", title: "Hotel list", url: "https://hotels.example/list", opened_by: "agent", owner_session_id: "chief-home" },
];
const posts = (c: Call) => c.method === "POST" && !c.path.includes("/prewarm");

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>;
}

function setup({
  respond,
  variant = "home",
  running = false,
  agent = chief,
  policyMode = "full",
  deviceKind = "person_device",
}: {
  respond?: (call: Call) => Response | undefined;
  variant?: "home" | "thread";
  running?: boolean;
  agent?: AgentSummary;
  policyMode?: string;
  deviceKind?: string;
} = {}) {
  const onSent = vi.fn();
  const onStop = vi.fn();
  const daemon = fakeClient((call) => {
    const answered = respond?.(call);
    if (answered) return answered;
    if (call.path === "/v1/settings/models") return json(200, CATALOG);
    if (call.path === "/v1/settings") return json(200, { default_model: null, fallback_models: [], policy_mode: policyMode });
    if (call.path === "/v1/devices/me") return json(200, { device_id: "d1", name: "Phone", kind: deviceKind, scopes: ["read", "steer"], request: null });
    if (call.method === "POST" && call.path === "/v1/sessions") return json(201, { id: "fresh" });
    return json(202, { id: "m", status: "queued" });
  });
  render(
    <ClientContext.Provider value={daemon.client}>
      <AgentsContext.Provider value={{ agents: [agent], error: null, offline: false, refresh: () => undefined }}>
        <AgentTabsContext.Provider value={TABS}>
          <MemoryRouter>
            <Where />
            <Routes>
              <Route
                path="*"
                element={
                  variant === "home" ? (
                    <Composer variant="home" sessionId="chief-home" newTask={{ agentId: "chief" }} onSent={onSent} />
                  ) : (
                    <Composer sessionId="chief-home" running={running} onStop={onStop} />
                  )
                }
              />
            </Routes>
          </MemoryRouter>
        </AgentTabsContext.Provider>
      </AgentsContext.Provider>
    </ClientContext.Provider>,
  );
  return { ...daemon, onSent, onStop };
}

describe("new task toolbar", () => {
  it("keeps an untouched task in the assistant's thread", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup();
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Plan a trip");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith(undefined));
    expect(calls.filter(posts).map((c) => c.path)).toEqual(["/v1/sessions/chief-home/messages"]);
  });

  it("starts a new conversation with the chosen effort, profile, speed and model, and never changes the person", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup();
    await user.click(screen.getByRole("button", { name: "Effort: Effort" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "High" }));
    await user.click(screen.getByRole("button", { name: "Effort: High" }));
    await user.click(await screen.findByRole("menuitemradio", { name: /Ultrabrowse/ }));
    await user.click(screen.getByRole("button", { name: "Speed: Speed" }));
    await user.click(await screen.findByRole("menuitemradio", { name: /Fast/ }));
    await user.click(await screen.findByRole("button", { name: "Model: GPT-6.1" }));
    const old = await screen.findByRole("menuitemradio", { name: /GPT-old/ });
    expect(old).toHaveAttribute("aria-disabled", "true");
    expect(old).toHaveTextContent("No key for this provider");
    await user.keyboard("{Escape}");

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Research flights");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    const [create, submit] = calls.filter(posts);
    expect(create).toMatchObject({ path: "/v1/sessions", body: { agent_id: "chief", effort: "high", profile: "ultrabrowse", speed: "fast" } });
    expect(create?.body).not.toHaveProperty("model");
    expect(submit).toMatchObject({ path: "/v1/sessions/fresh/messages", body: { text: "Research flights" } });
    expect(calls.some((c) => c.method === "PATCH")).toBe(false);
    // The toolbar starts over for the next task.
    expect(screen.getByRole("button", { name: "Effort: Effort" })).toBeInTheDocument();
  });

  it("offers only permissions within the agent's ceiling and creates private tasks without an owner", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(screen.getByRole("button", { name: "Add files and options" }));
    const menu = await screen.findByRole("menu", { name: "Add to this task" });
    const full = within(menu).getByRole("menuitemradio", { name: /Full access/ });
    expect(full).toHaveAttribute("aria-disabled", "true");
    expect(full).toHaveTextContent("More than openside may have");
    await user.click(within(menu).getByRole("menuitemradio", { name: "Read only" }));
    await user.click(screen.getByRole("button", { name: "Add files and options" }));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: /Private task/ }));
    await user.keyboard("{Escape}");
    expect(screen.getByRole("button", { name: "Permission: Read only" })).toBeInTheDocument();

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Quietly look this up");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls.filter(posts)).toHaveLength(2));
    expect(calls.filter(posts)[0]?.body).toEqual({ agent_id: null, incognito: true, browser_space: "shared", mode: "read-only" });
  });

  it("keeps a paired device below the daemon's default permission", async () => {
    const user = userEvent.setup();
    setup({ agent: { ...chief, mode: "full" } as AgentSummary, policyMode: "guard" });
    await user.click(screen.getByRole("button", { name: "Permission: Full access" }));
    const menu = await screen.findByRole("menu", { name: "Permission" });
    await waitFor(() => expect(within(menu).getByRole("menuitemradio", { name: /^Full access/ })).toHaveAttribute("aria-disabled", "true"));
    expect(within(menu).getByRole("menuitemradio", { name: /^Full access/ })).toHaveTextContent("Only the computer running openside can allow this");
    expect(within(menu).getByRole("menuitemradio", { name: "Guard" })).not.toHaveAttribute("aria-disabled");
  });

  it("withholds the costly options from an agent client", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ deviceKind: "agent_client" });
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/devices/me")).toBe(true));
    await user.click(screen.getByRole("button", { name: "Effort: Effort" }));
    for (const name of [/^Extra high/, /^Max/, /^Ultrabrowse/]) {
      const item = await screen.findByRole("menuitemradio", { name });
      expect(item).toHaveAttribute("aria-disabled", "true");
      expect(item).toHaveTextContent("Not available to an agent client");
    }
    expect(screen.getByRole("menuitemradio", { name: "High" })).not.toHaveAttribute("aria-disabled");
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Speed: Speed" }));
    expect(await screen.findByRole("menuitemradio", { name: /^Fast/ })).toHaveAttribute("aria-disabled", "true");
  });

  it("filters models from its own search field", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(await screen.findByRole("button", { name: "Model: GPT-6.1" }));
    const search = await screen.findByRole("searchbox", { name: "Search models" });
    await user.click(search);
    await user.type(search, "old");
    const menu = screen.getByRole("menu", { name: "Model" });
    expect(within(menu).getAllByRole("menuitemradio").map((item) => item.textContent)).toEqual([expect.stringMatching(/^GPT-old/)]);
    await user.clear(search);
    await user.type(search, "nothing like this");
    expect(within(menu).queryAllByRole("menuitemradio")).toHaveLength(0);
    expect(within(menu).getByText("No model matches.")).toBeInTheDocument();
  });

  it("shows the daemon's refusal and keeps the words", async () => {
    const user = userEvent.setup();
    setup({ respond: (call) => (call.method === "POST" && call.path === "/v1/sessions" ? json(403, { code: "local_socket_required", message: "x" }) : undefined) });
    await user.click(screen.getByRole("button", { name: "Speed: Speed" }));
    await user.click(await screen.findByRole("menuitemradio", { name: /Fast/ }));
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "Go fast");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Only the computer running openside can make this change");
    expect(field).toHaveValue("Go fast");
  });

  it("does not start a task offline: the words and settings stay and nothing is queued", async () => {
    const user = userEvent.setup();
    const { onSent } = setup({
      respond: (call) => {
        if (call.method === "POST" && call.path === "/v1/sessions") throw new TypeError("Failed to fetch");
        return undefined;
      },
    });
    await user.click(screen.getByRole("button", { name: "Effort: Effort" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Low" }));
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "Later please");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Can’t start a new task while offline");
    expect(field).toHaveValue("Later please");
    expect(screen.getByRole("button", { name: "Effort: Low" })).toBeInTheDocument();
    expect(outbox.snapshot()).toHaveLength(0);
    expect(onSent).not.toHaveBeenCalled();
  });

  it("still queues a plain send to the assistant's thread offline", async () => {
    const user = userEvent.setup();
    setup({
      respond: (call) => {
        if (call.method === "POST" && call.path.endsWith("/messages")) throw new TypeError("Failed to fetch");
        return undefined;
      },
    });
    await user.type(screen.getByRole("textbox", { name: "Message" }), "When you can");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(outbox.snapshot()).toHaveLength(1));
    expect(outbox.snapshot()[0]).toMatchObject({ session_id: "chief-home", text: "When you can" });
    expect(outbox.snapshot()[0]).not.toHaveProperty("create");
  });

  it("shows no request-scoped controls in an existing conversation", () => {
    setup({ variant: "thread" });
    expect(screen.queryByRole("button", { name: /^Effort:/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Model:/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Add files" })).toBeInTheDocument();
  });
});

describe("@ context menu", () => {
  it("lists tab groups, commands and open tabs, and inserts a tab as a link", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread", running: true });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "Compare @");
    const menu = screen.getByRole("listbox", { name: "Add context" });
    expect(within(menu).getAllByRole("group").map((g) => g.getAttribute("aria-labelledby"))).toEqual(["context-groups", "context-commands", "context-tabs"]);
    expect(within(menu).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "openside2 tabs",
      "New taskStart a separate task from Home",
      "Stop the taskAsk before stopping what is running",
      "Flights to Lisbonflights.example",
      "Hotel listhotels.example",
    ]);
    // Arrow keys wrap: up from the first row lands on the last.
    await user.keyboard("{ArrowUp}");
    expect(within(menu).getByRole("option", { name: /Hotel list/ })).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Enter}");
    expect(field).toHaveValue("Compare [Hotel list](https://hotels.example/list) ");
    expect(screen.queryByRole("listbox", { name: "Add context" })).toBeNull();
  });

  it("filters by what follows @, runs commands and closes with Escape", async () => {
    const user = userEvent.setup();
    const { onStop } = setup({ variant: "thread", running: true });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "@stop");
    expect(within(screen.getByRole("listbox", { name: "Add context" })).getAllByRole("option")).toHaveLength(1);
    await user.keyboard("{Enter}");
    expect(onStop).toHaveBeenCalledOnce();
    expect(field).toHaveValue("");

    await user.type(field, "@new");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox", { name: "Add context" })).toBeNull();
    await user.type(field, "{Backspace}w");
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent("/");
  });

  it("never opens for / or inside a word", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread" });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "/ mail me@example");
    expect(screen.queryByRole("listbox", { name: "Add context" })).toBeNull();
    await act(async () => undefined);
  });

  it("picks a row by tap or click as well as by keyboard", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread" });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "See @fli");
    await user.click(screen.getByRole("option", { name: /Flights to Lisbon/ }));
    expect(field).toHaveValue("See [Flights to Lisbon](https://flights.example/lis) ");
    expect(field).toHaveFocus();
  });

  it("names groups after the person whose conversation owns the tabs", () => {
    const rows = contextItems("", TABS, [chief], []);
    expect(rows[0]).toMatchObject({ section: "groups", label: "openside", detail: "2 tabs", insert: "[Flights to Lisbon](https://flights.example/lis) [Hotel list](https://hotels.example/list)" });
  });
});

describe("tabLink", () => {
  it("links only http and https pages", () => {
    expect(tabLink({ title: "Run me", url: "javascript:alert(1)" })).toBe("Run me");
    expect(tabLink({ title: "Inline", url: "data:text/html,<b>x</b>" })).toBe("Inline");
    expect(tabLink({ title: "", url: "file:///etc/passwd" })).toBe("Untitled tab");
    expect(tabLink({ title: "Docs", url: "https://docs.example/a" })).toBe("[Docs](https://docs.example/a)");
  });

  it("keeps parentheses, spaces and brackets from breaking out of the link", () => {
    expect(tabLink({ title: "Foo (bar)", url: "https://en.example/wiki/Foo_(bar)" })).toBe("[Foo (bar)](https://en.example/wiki/Foo_%28bar%29)");
    expect(tabLink({ title: "a](javascript:x) [b", url: "https://x.example/p?q=a b" })).toBe("[a\\](javascript:x) \\[b](https://x.example/p?q=a%20b)");
    expect(tabLink({ title: "back\\slash\nnext", url: "https://x.example/" })).toBe("[back\\\\slash next](https://x.example/)");
  });
});

describe("createSessionRequest", () => {
  it("leaves untouched settings out of the request", () => {
    expect(createSessionRequest("chief", DEFAULT_SESSION_OPTIONS)).toEqual({ agent_id: "chief" });
  });
});
