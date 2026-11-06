import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../../api/types";
import { ClientContext } from "../../context";
import { AgentsContext } from "../../hooks/useAgents";
import { ShellContext } from "../../shell";
import { agentSummary } from "../../test/agents";
import { sessionFixture } from "../../test/composerDaemon";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { ToastProvider } from "../../ui";
import { SettingsLayout } from "../SettingsLayout";

const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

const ARCHIVED: Session[] = [
  sessionFixture({ id: "z", title: "Old invoice", archived: true, last_activity_at: ago(5) }),
  // The chief's home chat (agentSummary gives it `s-chief`) can be archived but never deleted.
  sessionFixture({ id: "s-chief", title: "Assistant home", archived: true, last_activity_at: ago(120) }),
];

function Where() {
  return <output data-testid="path">{useLocation().pathname}</output>;
}

function setup(sessions: Session[] = ARCHIVED) {
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) as unknown as MediaQueryList);
  let pool = [...sessions];
  const { client, calls } = fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (call.method === "GET" && url.pathname === "/v1/sessions") {
      return json(200, { sessions: url.searchParams.get("archived") === "true" ? pool : [] });
    }
    const id = decodeURIComponent(url.pathname.split("/").pop() ?? "");
    if (call.method === "PATCH") {
      const body = call.body as { archived?: boolean };
      const session = pool.find((item) => item.id === id)!;
      if (body.archived === false) pool = pool.filter((item) => item.id !== id);
      return json(200, { ...session, ...body });
    }
    if (call.method === "DELETE") {
      pool = pool.filter((item) => item.id !== id);
      return new Response(null, { status: 204 });
    }
    return json(404, { code: "not_found", message: "none" });
  });
  const refreshSessions = vi.fn();
  render(
    <ClientContext.Provider value={client}>
      <AgentsContext.Provider value={{ agents: [agentSummary("chief", "openside")], error: null, offline: false, refresh: vi.fn() }}>
        <ShellContext.Provider value={{ openDrawer: vi.fn(), openSearch: vi.fn(), refreshSessions, sidebarCollapsed: false, toggleSidebar: vi.fn(), needsYou: 0 }}>
          <ToastProvider>
            <MemoryRouter initialEntries={["/settings/archived"]}>
              <Where />
              <Routes>
                <Route path="settings/*" element={<SettingsLayout onSignOut={() => undefined} />} />
                <Route path="/sessions/:id" element={<h1>Thread</h1>} />
              </Routes>
            </MemoryRouter>
          </ToastProvider>
        </ShellContext.Provider>
      </AgentsContext.Provider>
    </ClientContext.Provider>,
  );
  return { calls, refreshSessions };
}

const row = (title: string) => screen.getByRole("link", { name: title }).closest(".ui-row") as HTMLElement;
const writes = (calls: Call[]) => calls.filter((call) => call.method !== "GET");

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Settings › Archived chats", () => {
  it("puts an ⓘ after the title that says what archiving does", async () => {
    setup([]);
    const title = await screen.findByRole("heading", { level: 1, name: "Archived chats" });
    const help = "Archived chats stay out of the sidebar. Open one to read it, or bring it back from its ⋯ menu.";
    expect(title).toHaveAccessibleDescription(help);
    expect(title.querySelector(".settings-title-help")).toHaveAttribute("title", help);
  });

  it("shows one empty card when nothing is archived", async () => {
    setup([]);
    const title = await screen.findByText("No archived chats");
    const empty = title.closest(".ui-empty") as HTMLElement;
    expect(empty).toHaveAttribute("role", "status");
    expect(within(empty).getByText("Chats you archive will appear here.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Old invoice" })).toBeNull();
  });

  it("lists archived chats with their last activity, each opening its thread", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await screen.findByRole("link", { name: "Old invoice" });
    expect(calls.some((call) => call.method === "GET" && call.path.startsWith("/v1/sessions") && call.path.includes("archived=true"))).toBe(true);
    expect(within(row("Old invoice")).getByText("5 minutes ago")).toBeInTheDocument();
    expect(within(row("Assistant home")).getByText("2 hours ago")).toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "Old invoice" }));
    expect(screen.getByTestId("path")).toHaveTextContent("/sessions/z");
  });

  it("offers exactly Unarchive and Delete, and only Unarchive for an agent's home chat", async () => {
    const user = userEvent.setup();
    setup();
    await screen.findByRole("link", { name: "Old invoice" });
    await user.click(screen.getByRole("button", { name: "Options for Old invoice" }));
    let menu = await screen.findByRole("menu", { name: "Options for Old invoice" });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Unarchive chat", "Delete chat"]);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

    await user.click(screen.getByRole("button", { name: "Options for Assistant home" }));
    menu = await screen.findByRole("menu", { name: "Options for Assistant home" });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Unarchive chat"]);
  });

  it("unarchives a chat: PATCH archived false, the row leaves and the sidebar refreshes", async () => {
    const user = userEvent.setup();
    const { calls, refreshSessions } = setup();
    await screen.findByRole("link", { name: "Old invoice" });
    await user.click(screen.getByRole("button", { name: "Options for Old invoice" }));
    await user.click(await screen.findByRole("menuitem", { name: "Unarchive chat" }));
    await waitFor(() => expect(writes(calls)).toEqual([expect.objectContaining({ method: "PATCH", path: "/v1/sessions/z", body: { archived: false } })]));
    expect(await screen.findByText("Conversation unarchived")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("link", { name: "Old invoice" })).toBeNull());
    expect(screen.getByRole("link", { name: "Assistant home" })).toBeInTheDocument();
    expect(refreshSessions).toHaveBeenCalled();
  });

  it("deletes a chat only after the confirmation", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await screen.findByRole("link", { name: "Old invoice" });
    await user.click(screen.getByRole("button", { name: "Options for Old invoice" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete chat" }));
    const confirm = await screen.findByRole("dialog");
    expect(writes(calls)).toEqual([]);
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(writes(calls)).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Options for Old invoice" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete chat" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Delete chat" }));
    await waitFor(() => expect(writes(calls)).toEqual([expect.objectContaining({ method: "DELETE", path: "/v1/sessions/z" })]));
    await waitFor(() => expect(screen.queryByRole("link", { name: "Old invoice" })).toBeNull());
  });
});
