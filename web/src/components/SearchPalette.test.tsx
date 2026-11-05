import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { describe, expect, it } from "vitest";
import type { AgentSummary, Session } from "../api/types";
import { ClientContext } from "../context";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { AgentsContext } from "../hooks/useAgents";
import { agentSummary } from "../test/agents";
import { fakeClient, json } from "../test/fakeDaemon";
import { createClient } from "../api/client";
import { relativeTime, SearchPalette } from "./SearchPalette";

function session(id: string, title: string): Session {
  return {
    id,
    title,
    activity: "",
    agent_id: null,
    archived: false,
    browser_space: "shared",
    incognito: false,
    last_seq: 1,
    mode: "guard",
    model: "m",
    pinned: false,
    queue: [],
    status: "idle",
  };
}

const TABS: OwnedTab[] = [
  { tab_id: "t1", title: "Flight search", url: "https://flights.example/q", opened_by: "agent", owner_session_id: "s-mina" },
  { tab_id: "t2", title: "Docs", url: "https://docs.example/", opened_by: "agent", owner_session_id: "s-old" },
];

const AGENTS: AgentSummary[] = [
  agentSummary("chief", "openside", { last_message_at: "2026-10-12T08:00:00Z" }),
  agentSummary("mina", "Mina", { last_message_at: new Date(Date.now() - 5 * 60_000).toISOString() }),
];

function Where() {
  return <output aria-label="path">{useLocation().pathname}</output>;
}

function setup({ tabs = TABS, open = true } = {}) {
  const { client, calls } = fakeClient((call) => {
    const url = new URL(call.path, "http://daemon.test");
    // No cached favicons: tab rows fall back to the globe.
    if (url.pathname !== "/v1/sessions") return json(404, { code: "not_found", message: "none" });
    const q = url.searchParams.get("q") ?? "";
    const all = [session("s1", "Trip to Busan"), session("s2", "Quarterly report")];
    return json(200, { sessions: all.filter((entry) => entry.title.toLowerCase().includes(q.toLowerCase())) });
  });
  let closed = 0;
  render(
    <ClientContext.Provider value={client}>
      <AgentsContext.Provider value={{ agents: AGENTS, error: null, offline: false, refresh: () => {} }}>
        <MemoryRouter initialEntries={["/"]}>
          <Where />
          <SearchPalette open={open} onClose={() => (closed += 1)} tabs={tabs} />
        </MemoryRouter>
      </AgentsContext.Provider>
    </ClientContext.Provider>,
  );
  // The modal palette hides the page from the accessibility tree, the path readout included.
  return { calls, closed: () => closed, path: () => screen.getByRole("status", { name: "path", hidden: true }).textContent };
}

const options = () => screen.getAllByRole("option");
function option(index: number): HTMLElement {
  const found = options()[index];
  if (!found) throw new Error(`no result at ${index}`);
  return found;
}

/** The field has focus and the daemon's conversations have arrived. */
async function settled() {
  await waitFor(() => expect(screen.getByRole("combobox")).toHaveFocus());
  await screen.findByText("Trip to Busan");
}

describe("SearchPalette", () => {
  it("opens with the field focused, tabs first, chats with real times, then conversations; the first result is highlighted", async () => {
    const { calls } = setup();
    const field = screen.getByRole("combobox", { name: "Search tabs and chats" });
    await waitFor(() => expect(field).toHaveFocus());
    await screen.findByText("Trip to Busan");
    const groups = screen.getAllByRole("group").map((group) => within(group).getAllByRole("option").length);
    expect(groups).toEqual([2, 2, 2]);
    expect(screen.getByText("Open tabs")).toBeInTheDocument();
    expect(options()[0]).toHaveAttribute("aria-selected", "true");
    expect(field).toHaveAttribute("aria-activedescendant", option(0).id);
    expect(within(option(0)).getByText("Switch")).toBeInTheDocument();
    expect(within(option(2)).getByText("Mina")).toBeInTheDocument();
    expect(within(option(2)).getByText("5 min ago")).toBeInTheDocument();
    // Conversations carry no timestamp, so none is shown.
    expect(within(option(4)).queryByText(/ago|Yesterday|Just now/)).toBeNull();
    expect(screen.getByText("Ctrl+Shift+K")).toBeInTheDocument();
    expect(calls.filter((call) => call.path.startsWith("/v1/sessions")).map((call) => call.path)).toEqual(["/v1/sessions?agent=none"]);
  });

  it("filters every section, asks the daemon's conversation search, and says when nothing matches", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await settled();
    await user.type(screen.getByRole("combobox"), "busan");
    await waitFor(() => expect(calls.some((call) => call.path === "/v1/sessions?q=busan&agent=none")).toBe(true));
    await waitFor(() => expect(options().map((option) => option.textContent)).toEqual(["Trip to Busan"]));
    expect(options()[0]).toHaveAttribute("aria-selected", "true");
    await user.clear(screen.getByRole("combobox"));
    await user.type(screen.getByRole("combobox"), "zzz");
    expect(await screen.findByText("Nothing matches “zzz”.")).toBeInTheDocument();
  });

  it("moves with the arrow keys, wraps, and opens the choice with Enter", async () => {
    const user = userEvent.setup();
    const { path, closed } = setup();
    await settled();
    await user.keyboard("{ArrowUp}");
    expect(options()[5]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(options()[2]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Enter}");
    expect(path()).toBe("/agents/mina");
    expect(closed()).toBe(1);
  });

  it("switches to a tab's conversation with the browser open, and opens a row on click", async () => {
    const user = userEvent.setup();
    const { path } = setup();
    await settled();
    await user.keyboard("{Enter}");
    expect(path()).toBe("/agents/mina");
    await user.click(screen.getByText("Docs"));
    expect(path()).toBe("/sessions/s-old");
  });

  it("closes on Escape", async () => {
    const user = userEvent.setup();
    const { closed } = setup();
    await settled();
    await user.keyboard("{Escape}");
    expect(closed()).toBe(1);
  });

  it("omits a section with nothing in it", async () => {
    setup({ tabs: [] });
    await settled();
    expect(screen.queryByText("Open tabs")).toBeNull();
    expect(options()[0]).toHaveTextContent("Mina");
  });

  it("renders nothing while closed", () => {
    const { calls } = setup({ open: false });
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(calls).toHaveLength(0);
  });

  it("aborts a conversation search the person typed past, and the one still out when the palette closes", async () => {
    const user = userEvent.setup();
    const requests: { q: string; signal: AbortSignal }[] = [];
    const client = createClient({
      token: () => "test-token",
      fetch: (input, init) => {
        const url = new URL(String(input), "http://daemon.test");
        if (url.pathname !== "/v1/sessions" || !init?.signal) return Promise.resolve(json(404, { code: "not_found", message: "none" }));
        const signal = init.signal;
        requests.push({ q: url.searchParams.get("q") ?? "", signal });
        // The daemon never answers: only an abort settles the request.
        const { promise, reject } = Promise.withResolvers<Response>();
        signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
        return promise;
      },
    });
    const view = render(
      <ClientContext.Provider value={client}>
        <AgentsContext.Provider value={{ agents: AGENTS, error: null, offline: false, refresh: () => {} }}>
          <MemoryRouter>
            <SearchPalette open onClose={() => {}} tabs={[]} />
          </MemoryRouter>
        </AgentsContext.Provider>
      </ClientContext.Provider>,
    );
    await waitFor(() => expect(requests.map((request) => request.q)).toEqual([""]));
    await user.type(screen.getByRole("combobox"), "bu");
    await waitFor(() => expect(requests.map((request) => request.q)).toEqual(["", "bu"]));
    expect(requests[0]?.signal.aborted).toBe(true);
    expect(requests[1]?.signal.aborted).toBe(false);
    expect(screen.getByText("Searching…")).toBeInTheDocument();
    view.rerender(
      <ClientContext.Provider value={client}>
        <AgentsContext.Provider value={{ agents: AGENTS, error: null, offline: false, refresh: () => {} }}>
          <MemoryRouter>
            <SearchPalette open={false} onClose={() => {}} tabs={[]} />
          </MemoryRouter>
        </AgentsContext.Provider>
      </ClientContext.Provider>,
    );
    expect(requests[1]?.signal.aborted).toBe(true);
    expect(requests).toHaveLength(2);
  });
});

describe("relativeTime", () => {
  const now = new Date(2026, 9, 12, 12, 0);
  it("reads like a chat list and refuses missing times", () => {
    expect(relativeTime(new Date(2026, 9, 12, 11, 59, 40).toISOString(), now)).toBe("Just now");
    expect(relativeTime(new Date(2026, 9, 12, 11, 15).toISOString(), now)).toBe("45 min ago");
    expect(relativeTime(new Date(2026, 9, 12, 2, 0).toISOString(), now)).toBe("10 h ago");
    expect(relativeTime(new Date(2026, 9, 11, 2, 0).toISOString(), now)).toBe("Yesterday");
    expect(relativeTime(new Date(2026, 9, 8, 2, 0).toISOString(), now)).toBe("4 days ago");
    expect(relativeTime("", now)).toBeNull();
    expect(relativeTime("not a date", now)).toBeNull();
  });

  it("reads a time ahead of this clock as just now", () => {
    expect(relativeTime(new Date(2026, 9, 12, 12, 5).toISOString(), now)).toBe("Just now");
    expect(relativeTime(new Date(2026, 9, 14, 9, 0).toISOString(), now)).toBe("Just now");
  });

  it("counts calendar days across a daylight-saving change", () => {
    const zone = process.env.TZ;
    process.env.TZ = "America/New_York";
    try {
      // Clocks went forward at 2:00 on Mar 8 2026, so Mar 8 was 23 hours long.
      const morning = new Date(2026, 2, 9, 0, 30);
      expect(relativeTime(new Date(2026, 2, 8, 0, 15).toISOString(), morning)).toBe("23 h ago");
      expect(relativeTime(new Date(2026, 2, 7, 23, 45).toISOString(), morning)).toBe("23 h ago");
      // 24 h 15 min of wall time, yet two calendar days back: a fixed 24-hour day would say Yesterday.
      expect(relativeTime(new Date(2026, 2, 7, 23, 15).toISOString(), morning)).toBe("2 days ago");
      const later = new Date(2026, 2, 10, 0, 30);
      expect(relativeTime(new Date(2026, 2, 9, 0, 15).toISOString(), later)).toBe("Yesterday");
      expect(relativeTime(new Date(2026, 2, 8, 0, 45).toISOString(), later)).toBe("2 days ago");
      // Nov 1 2026 was 25 hours long when clocks went back.
      const fall = new Date(2026, 10, 2, 23, 30);
      expect(relativeTime(new Date(2026, 10, 1, 0, 0).toISOString(), fall)).toBe("Yesterday");
      expect(relativeTime(new Date(2026, 9, 31, 23, 59).toISOString(), fall)).toBe("2 days ago");
    } finally {
      if (zone === undefined) delete process.env.TZ;
      else process.env.TZ = zone;
    }
  });
});
