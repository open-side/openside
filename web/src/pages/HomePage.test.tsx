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

function setup(events: Record<string, unknown>[] = []) {
  const daemon = fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (url.pathname === "/v1/sessions/s-chief/events") return stream("s-chief", events);
    if (call.method === "POST" && url.pathname === "/v1/sessions/s-chief/messages") return json(202, { id: "m", status: "queued" });
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
