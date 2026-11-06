import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { ShellContext } from "../shell";
import { emptySessionView } from "../state/session";
import { sessionFixture } from "../test/composerDaemon";
import { fakeClient, json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import type { ThreadViewProps } from "./ThreadView";
import { SessionPage } from "./SessionPage";

const shown = vi.hoisted(() => ({ folder: undefined as { projectId: string | null; workspace: string | null } | undefined }));

vi.mock("./ThreadView", () => ({
  ThreadView: ({ header }: ThreadViewProps) => header({ view: { ...emptySessionView, title: "A title I chose", model: "initial:model", folder: shown.folder }, running: false, waiting: false }),
}));

afterEach(() => {
  cleanup();
  shown.folder = undefined;
});

function show(answer: Parameters<typeof fakeClient>[0]) {
  const daemon = fakeClient(answer);
  const refreshSessions = vi.fn();
  render(
    <MemoryRouter initialEntries={["/sessions/s1"]}>
      <ClientContext.Provider value={daemon.client}>
        <AgentsContext.Provider value={{ agents: [], error: null, offline: false, refresh: vi.fn() }}>
          <ShellContext.Provider value={{ openDrawer: vi.fn(), openSearch: vi.fn(), refreshSessions, sidebarCollapsed: false, toggleSidebar: vi.fn(), needsYou: 0 }}>
            <ToastProvider>
              <Routes>
                <Route path="/sessions/:id" element={<SessionPage />} />
                <Route path="/chats" element={<h1>Chats</h1>} />
              </Routes>
            </ToastProvider>
          </ShellContext.Provider>
        </AgentsContext.Provider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return { ...daemon, refreshSessions };
}

const PROJECTS = [
  { id: "p1", name: "Website", root: "/home/me/code/website", scratch: false, instructions: "", created_at: "2026-11-06T00:00:00Z", updated_at: "2026-11-06T00:00:00Z" },
  { id: "p2", name: "Notes", root: "/data/projects/p2", scratch: true, instructions: "", created_at: "2026-11-06T00:00:00Z", updated_at: "2026-11-06T00:00:00Z" },
];

it("names the chat's project beside its title, and follows the chat when it moves", async () => {
  const answer = (call: { path: string }) => {
    if (call.path === "/v1/sessions/s1") return json(200, sessionFixture({ project_id: "p1" }));
    if (call.path === "/v1/projects") return json(200, { projects: PROJECTS });
    return json(404, { code: "not_found", message: "none" });
  };
  show(answer);
  expect(await screen.findByText("Website")).toHaveAttribute("class", "thread-header-project-name");
  cleanup();

  shown.folder = { projectId: "p2", workspace: "/data/projects/p2" };
  show(answer);
  expect(await screen.findByText("Notes")).toBeInTheDocument();
  expect(screen.queryByText("Website")).toBeNull();
  cleanup();

  shown.folder = { projectId: null, workspace: "/home/me/elsewhere" };
  const { calls } = show(answer);
  await waitFor(() => expect(calls.some((call) => call.path === "/v1/sessions/s1")).toBe(true));
  expect(screen.queryByText("Website")).toBeNull();
  expect(calls.some((call) => call.path === "/v1/projects")).toBe(false);
});

it("shows no project when the chat and the project list read back empty", async () => {
  const { calls } = show(() => new Response(null, { status: 204 }));
  await waitFor(() => expect(calls.some((call) => call.path === "/v1/sessions/s1")).toBe(true));
  expect(screen.getByRole("heading", { name: "A title I chose" })).toBeInTheDocument();
  expect(document.querySelector(".thread-header-project")).toBeNull();
});

it("returns a deleted ordinary conversation to Chats without changing its title or displaying the initial model", async () => {
  const user = userEvent.setup();
  const { calls, refreshSessions } = show(() => new Response(null, { status: 204 }));
  expect(screen.getByRole("heading", { name: "A title I chose" })).toBeInTheDocument();
  expect(screen.queryByText("initial:model")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Conversation options" }));
  await user.click(await screen.findByRole("menuitem", { name: "Delete conversation" }));
  const dialog = await screen.findByRole("dialog", { name: "Delete conversation?" });
  expect(dialog).toHaveTextContent("A title I chose");
  await user.click(within(dialog).getByRole("button", { name: "Delete conversation" }));
  expect(await screen.findByRole("heading", { name: "Chats" })).toBeInTheDocument();
  expect(refreshSessions).toHaveBeenCalledOnce();
  expect(calls.filter((call) => call.method !== "GET")).toEqual([expect.objectContaining({ method: "DELETE", path: "/v1/sessions/s1" })]);
});
