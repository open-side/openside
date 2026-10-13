import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { ShellContext } from "../shell";
import { emptySessionView } from "../state/session";
import { fakeClient } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import type { ThreadViewProps } from "./ThreadView";
import { SessionPage } from "./SessionPage";

vi.mock("./ThreadView", () => ({
  ThreadView: ({ header }: ThreadViewProps) => header({ view: { ...emptySessionView, title: "A title I chose", model: "initial:model" }, running: false, waiting: false }),
}));

afterEach(cleanup);

it("returns a deleted ordinary conversation to History without changing its title or displaying the initial model", async () => {
  const user = userEvent.setup();
  const daemon = fakeClient(() => new Response(null, { status: 204 }));
  const refreshSessions = vi.fn();
  render(
    <MemoryRouter initialEntries={["/sessions/s1"]}>
      <ClientContext.Provider value={daemon.client}>
        <AgentsContext.Provider value={{ agents: [], error: null, offline: false, refresh: vi.fn() }}>
          <ShellContext.Provider value={{ openDrawer: vi.fn(), refreshSessions }}>
            <ToastProvider>
              <Routes>
                <Route path="/sessions/:id" element={<SessionPage />} />
                <Route path="/history" element={<h1>History</h1>} />
              </Routes>
            </ToastProvider>
          </ShellContext.Provider>
        </AgentsContext.Provider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  expect(screen.getByRole("heading", { name: "A title I chose" })).toBeInTheDocument();
  expect(screen.queryByText("initial:model")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Conversation options" }));
  await user.click(await screen.findByRole("menuitem", { name: "Delete conversation" }));
  const dialog = await screen.findByRole("dialog", { name: "Delete conversation?" });
  expect(dialog).toHaveTextContent("A title I chose");
  await user.click(within(dialog).getByRole("button", { name: "Delete conversation" }));
  expect(await screen.findByRole("heading", { name: "History" })).toBeInTheDocument();
  expect(refreshSessions).toHaveBeenCalledOnce();
  expect(daemon.calls).toEqual([expect.objectContaining({ method: "DELETE", path: "/v1/sessions/s1" })]);
});
