import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { describe, expect, it } from "vitest";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { agentSummary } from "../test/agents";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { HomePage } from "./HomePage";

/** An event stream that replays `events` and stays open, as the daemon's does. */
function stream(session: string, events: Record<string, unknown>[]): Response {
  const encoder = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      events.forEach((event, index) => {
        const full = { session_id: session, seq: index + 1, ts: "2026-10-12T00:00:00Z", run_id: null, ...event };
        controller.enqueue(encoder.encode(`id: ${index + 1}\ndata: ${JSON.stringify(full)}\n\n`));
      });
    },
  });
  return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
}

const chief = agentSummary("chief", "openside", { home_session_id: "s-chief" });
/** While set, the conversation search cannot reach the daemon. */
let searchDown = false;

function setup(events: Record<string, unknown>[] = []) {
  const daemon = fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (url.pathname === "/v1/sessions/s-chief/events") return stream("s-chief", events);
    if (call.method === "POST" && url.pathname === "/v1/sessions/s-chief/messages") return json(202, { id: "m", status: "queued" });
    if (call.method === "GET" && url.pathname === "/v1/sessions") {
      if (searchDown) throw new TypeError("Failed to fetch");
      const q = url.searchParams.get("q") ?? "";
      const all = [
        { id: "s-flight", title: "Cheaper flight to Lisbon", status: "idle", activity: "", created_at: "2026-10-12T00:00:00Z", updated_at: "2026-10-12T00:00:00Z", pinned: false, archived: false },
        { id: "s-form", title: "Visa form", status: "waiting_human", activity: "", created_at: "2026-10-12T00:00:00Z", updated_at: "2026-10-12T00:00:00Z", pinned: false, archived: false },
      ];
      return json(200, { sessions: all.filter((session) => session.title.toLowerCase().includes(q.toLowerCase())) });
    }
    return json(404, { code: "not_found", message: "not found" });
  });
  render(
    <ClientContext.Provider value={daemon.client}>
      <AgentsContext.Provider value={{ agents: [chief], error: null, offline: false, refresh: () => {} }}>
        <ToastProvider>
          <MemoryRouter initialEntries={["/"]}>
            <Routes>
              <Route index element={<HomePage />} />
              <Route path="/agents/:id" element={<p>assistant thread</p>} />
              <Route path="/sessions/:id" element={<p>session page</p>} />
            </Routes>
          </MemoryRouter>
        </ToastProvider>
      </AgentsContext.Provider>
    </ClientContext.Provider>,
  );
  return daemon;
}

describe("HomePage", () => {
  it("hands the task to the assistant's thread, then opens it", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    expect(screen.getByRole("heading", { name: "What should we do?" })).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Find a cheaper flight");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("assistant thread")).toBeInTheDocument();
    expect(calls.find((call) => call.method === "POST" && call.path === "/v1/sessions/s-chief/messages")?.body).toMatchObject({ text: "Find a cheaper flight" });
  });

  it("shows background Tasks as cards that lead into the thread", async () => {
    setup([
      { kind: "subagent.started", subagent_id: "t1", background: true, model: "m", parent_tool_call_id: "c1", task: "Compare prices for the stroller", tools: [] },
      { kind: "subagent.started", subagent_id: "d1", background: false, model: "m", parent_tool_call_id: "c2", task: "Inline delegate", tools: [] },
    ]);
    const section = await screen.findByRole("region", { name: "Recent tasks" });
    const card = within(section).getByRole("link", { name: /Compare prices for the stroller/ });
    expect(card).toHaveAttribute("href", "/agents/chief");
    expect(within(card).getByText("Working")).toBeInTheDocument();
    expect(screen.queryByText("Inline delegate")).toBeNull();
  });

  it("has no Recent tasks section without Tasks", async () => {
    setup();
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toBeInTheDocument());
    expect(screen.queryByRole("region", { name: "Recent tasks" })).toBeNull();
  });
});

describe("HomePage modes and suggestions", () => {
  it("shows the openside mark and starts in Ask with the task composer", async () => {
    setup();
    expect(screen.getByRole("img", { name: "openside" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toBeInTheDocument());
  });

  it("fills the composer from a suggested task without sending it", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const section = screen.getByRole("region", { name: "Suggested tasks" });
    expect(within(section).getAllByRole("button")).toHaveLength(3);
    await user.click(within(section).getByRole("button", { name: /Fill in a form/ }));
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("Fill in this form with my details and let me review it before you submit: ");
    expect(calls.some((call) => call.method === "POST" && call.path.endsWith("/messages"))).toBe(false);
  });

  it("switches a suggestion pick from Search back to Ask", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("tab", { name: "Search" }));
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull();
    await user.click(screen.getByRole("button", { name: /Compare two offers/ }));
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("Compare these two offers on price, delivery and returns, and tell me which is better: ");
  });

  it("searches conversations through the daemon and opens the top match on Enter", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(screen.getByRole("tab", { name: "Search" }));
    const field = screen.getByRole("searchbox", { name: "Search conversations" });
    expect(field).toHaveFocus();
    expect(calls.some((call) => call.path.startsWith("/v1/sessions?") || call.path === "/v1/sessions")).toBe(false);
    await user.type(field, "visa");
    const results = await screen.findByRole("region", { name: "Search results" });
    const match = await within(results).findByRole("link", { name: /Visa form/ });
    expect(match).toHaveAttribute("href", "/sessions/s-form");
    expect(within(results).queryByText("Cheaper flight to Lisbon")).toBeNull();
    expect(calls.some((call) => call.path === "/v1/sessions?q=visa")).toBe(true);
    await user.type(field, "{Enter}");
    expect(await screen.findByText("session page")).toBeInTheDocument();
  });

  it("says so when nothing matches", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("tab", { name: "Search" }));
    await user.type(screen.getByRole("searchbox", { name: "Search conversations" }), "zebra");
    expect(await screen.findByText("No conversations match “zebra”.")).toBeInTheDocument();
  });
});

describe("HomePage search failures and keyboard", () => {
  it("says when search cannot reach openside, and Retry searches again", async () => {
    const user = userEvent.setup();
    searchDown = true;
    try {
      setup();
      await user.click(screen.getByRole("tab", { name: "Search" }));
      await user.type(screen.getByRole("searchbox", { name: "Search conversations" }), "visa");
      expect(await screen.findByText("Can’t reach openside, so search is unavailable.")).toBeInTheDocument();
      searchDown = false;
      await user.click(screen.getByRole("button", { name: "Retry" }));
      expect(await screen.findByRole("link", { name: /Visa form/ })).toHaveAttribute("href", "/sessions/s-form");
      expect(screen.queryByText("Can’t reach openside, so search is unavailable.")).toBeNull();
    } finally {
      searchDown = false;
    }
  });

  it("keeps focus on the mode toggle while arrowing between modes", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("tab", { name: "Ask" }));
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("searchbox", { name: "Search conversations" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Search" })).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveFocus();
    expect(screen.getByRole("textbox", { name: "Message" })).toBeInTheDocument();
  });

  it("focuses the search field when Search is clicked", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("tab", { name: "Search" }));
    expect(screen.getByRole("searchbox", { name: "Search conversations" })).toHaveFocus();
  });
});
