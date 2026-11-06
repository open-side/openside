import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { afterEach, expect, it, vi } from "vitest";
import type { Event } from "../api/types";
import { ClientContext } from "../context";
import { ShellContext } from "../shell";
import { emptySessionView, replaySession, type SessionView } from "../state/session";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { ThreadView } from "./ThreadView";

const shown = vi.hoisted(() => ({ view: null as SessionView | null }));
vi.mock("../hooks/useSessionView", () => ({
  useSessionView: () => ({ view: shown.view ?? { ...emptySessionView, status: "running" }, connection: "live", error: null }),
}));
vi.mock("../hooks/useSessionFiles", () => ({
  useSessionFiles: () => ({ artifacts: [], visuals: {} }),
}));

afterEach(() => {
  shown.view = null;
});

/** Requests other than the one under test never answer, so nothing else changes the screen. */
const quiet = () => new Promise<Response>(() => undefined);

const shell = { openDrawer: vi.fn(), openSearch: vi.fn(), refreshSessions: vi.fn(), needsYou: 0, sidebarCollapsed: false, toggleSidebar: vi.fn() };

function Where() {
  return <output data-testid="where">{useLocation().pathname}</output>;
}

function finishedTurn(): SessionView {
  const events = [
    { kind: "message.queued", message_id: "m1", text: "Hi", mode: "queue", author_kind: "person", from_agent: null },
    { kind: "message.injected", message_id: "m1" },
    { kind: "turn.started" },
    { kind: "assistant.final", text: "Hello." },
    { kind: "message.completed", message_id: "m1", outcome: "completed" },
    { kind: "turn.ended" },
    { kind: "session.settled" },
  ].map((p, i) => ({ session_id: "s1", seq: i + 1, ts: "2026-01-01T00:00:00Z", run_id: null, ...p })) as Event[];
  return replaySession(events);
}

function mountThread(handler: (call: Call) => Response | Promise<Response>) {
  const daemon = fakeClient(handler);
  render(
    <MemoryRouter initialEntries={["/sessions/s1"]}>
      <ClientContext.Provider value={daemon.client}>
        <ShellContext.Provider value={shell}>
          <ToastProvider>
            <ThreadView sessionId="s1" header={() => null} />
            <Where />
          </ToastProvider>
        </ShellContext.Provider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return daemon;
}

it("branches a reply into a new chat from the turn's message and opens it", async () => {
  const user = userEvent.setup();
  shown.view = finishedTurn();
  shell.refreshSessions.mockClear();
  const daemon = mountThread((call) =>
    call.method === "POST" && call.path === "/v1/sessions/s1/fork" ? json(201, { id: "new/chat", title: "Hi" }) : quiet(),
  );
  await user.click(screen.getByRole("button", { name: "Branch from here" }));
  await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent("/sessions/new%2Fchat"));
  expect(daemon.calls.filter((call) => call.path === "/v1/sessions/s1/fork")).toEqual([expect.objectContaining({ method: "POST", body: { from_message: "m1" } })]);
  expect(shell.refreshSessions).toHaveBeenCalled();
});

it("stays put and says why when the reply cannot be branched yet", async () => {
  const user = userEvent.setup();
  shown.view = finishedTurn();
  mountThread((call) =>
    call.path === "/v1/sessions/s1/fork" ? json(409, { code: "message_not_finished", message: "not finished" }) : quiet(),
  );
  await user.click(screen.getByRole("button", { name: "Branch from here" }));
  expect(await screen.findByText("This reply isn’t finished yet. Branch from it once it’s done.")).toBeInTheDocument();
  expect(screen.getByTestId("where")).toHaveTextContent("/sessions/s1");
});

it("keeps a side chat compact: no branch, no live browser card, no computer-use card", () => {
  shown.view = finishedTurn();
  render(
    <MemoryRouter initialEntries={["/sessions/main"]}>
      <ClientContext.Provider value={fakeClient(quiet).client}>
        <ShellContext.Provider value={shell}>
          <ToastProvider>
            <ThreadView sessionId="s1" header={() => null} side />
          </ToastProvider>
        </ShellContext.Provider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  expect(screen.getByText("Hello.")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Branch from here" })).toBeNull();
  expect(document.querySelector(".browser-row, .computer-use-card")).toBeNull();
  expect(document.querySelector("main.conversation")).toHaveAttribute("data-side", "true");
});

it("closes Stop everything as soon as it is confirmed, before the daemon answers", async () => {
  const user = userEvent.setup();
  const calls: Call[] = [];
  // Every request stays pending: the sheet must not wait on the stop to land.
  const daemon = fakeClient((call) => {
    calls.push(call);
    return new Promise<Response>(() => undefined);
  });
  render(
    <MemoryRouter initialEntries={["/sessions/s1"]}>
      <ClientContext.Provider value={daemon.client}>
        <ShellContext.Provider value={shell}>
          <ToastProvider>
            <ThreadView sessionId="s1" header={() => null} />
          </ToastProvider>
        </ShellContext.Provider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  await user.click(screen.getByRole("button", { name: "Stop" }));
  const dialog = await screen.findByRole("dialog", { name: "Stop everything?" });
  await user.click(within(dialog).getByRole("button", { name: "Stop everything" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(calls.some((call) => call.method === "POST" && call.path === "/v1/sessions/s1/stop")).toBe(true);
});
