import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../api/types";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { ShellContext } from "../shell";
import { agentSummary } from "../test/agents";
import { sessionFixture } from "../test/composerDaemon";
import { fakeClient, json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { CHATS_VIEW_KEY, ChatsPage } from "./ChatsPage";

// The composer has its own tests; here it only has to be on the page.
vi.mock("../components/Composer", () => ({ Composer: () => <div data-testid="composer" /> }));

const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

const ACTIVE: Session[] = [
  sessionFixture({ id: "s-chief", title: "Assistant home", pinned: true, last_activity_at: ago(1) }),
  sessionFixture({ id: "a", title: "Busan trip", unread: true, last_activity_at: ago(5) }),
  sessionFixture({ id: "b", title: "Quarterly report", status: "running", last_activity_at: ago(30) }),
  sessionFixture({ id: "c", title: "Apartment search", last_activity_at: ago(120), project_id: "p1" }),
];
const ARCHIVED: Session[] = [sessionFixture({ id: "z", title: "Old invoice", archived: true, last_activity_at: ago(4000) })];

function Where() {
  return <output data-testid="path">{useLocation().pathname + useLocation().search}</output>;
}

function setup(path = "/chats", { failDelete = false, projects = false } = {}) {
  const { client, calls } = fakeClient((call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (call.method === "GET" && url.pathname === "/v1/sessions") {
      const q = (url.searchParams.get("q") ?? "").toLowerCase();
      const pool = url.searchParams.get("archived") === "true" ? ARCHIVED : ACTIVE;
      return json(200, { sessions: pool.filter((session) => session.title.toLowerCase().includes(q)) });
    }
    if (projects && call.method === "GET" && url.pathname === "/v1/projects") {
      return json(200, { projects: [{ id: "p1", name: "Move", root: "/home/me/move", scratch: false, instructions: "", created_at: ago(9000), updated_at: ago(9000) }] });
    }
    if (call.method === "PATCH") return json(200, ACTIVE[0]);
    if (call.method === "DELETE") return failDelete && url.pathname.endsWith("/c") ? json(409, { code: "busy", message: "Still running" }) : new Response(null, { status: 204 });
    return json(404, { code: "not_found", message: "none" });
  });
  const refreshSessions = vi.fn();
  render(
    <ClientContext.Provider value={client}>
      <AgentsContext.Provider value={{ agents: [agentSummary("chief", "openside")], error: null, offline: false, refresh: vi.fn() }}>
        <ShellContext.Provider value={{ openDrawer: vi.fn(), openSearch: vi.fn(), refreshSessions, sidebarCollapsed: false, toggleSidebar: vi.fn(), needsYou: 0 }}>
          <ToastProvider>
            <MemoryRouter initialEntries={[path]}>
              <Where />
              <Routes>
                <Route path="/chats" element={<ChatsPage />} />
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

const list = () => screen.getByRole("list", { name: /Chats|Archived chats/ });
const rows = () =>
  within(list())
    .getAllByRole("listitem")
    .map((row) => row.querySelector(".chats-title-text")?.textContent);
const check = (title: string) => screen.getByRole("checkbox", { name: `Select ${title}` });

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("ChatsPage", () => {
  it("shows the composer, the List | Card switch and every chat, pinned first, with status and time", async () => {
    setup();
    expect(screen.getByTestId("composer")).toBeInTheDocument();
    await waitFor(() => expect(rows()).toEqual(["Assistant home", "Busan trip", "Quarterly report", "Apartment search"]));
    expect(screen.getByRole("radio", { name: "List" })).toHaveAttribute("aria-checked", "true");
    const busan = check("Busan trip").closest("li")!;
    expect(busan).toHaveClass("is-unread");
    expect(within(busan).getByRole("img", { name: "Unread" })).toBeInTheDocument();
    expect(within(busan).getByText("5 minutes ago")).toBeInTheDocument();
    expect(within(check("Quarterly report").closest("li")!).getByRole("img", { name: "Working" })).toBeInTheDocument();
    expect(within(check("Assistant home").closest("li")!).getByRole("img", { name: "Pinned" })).toBeInTheDocument();
  });

  it("switches to cards and remembers the choice", async () => {
    const user = userEvent.setup();
    setup();
    await waitFor(() => expect(rows()).toHaveLength(4));
    await user.click(screen.getByRole("radio", { name: "Card" }));
    expect(screen.getByRole("radio", { name: "Card" })).toHaveAttribute("aria-checked", "true");
    expect(list()).toHaveClass("chats-cards");
    expect(localStorage.getItem(CHATS_VIEW_KEY)).toBe("card");
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("radio", { name: "List" })).toHaveAttribute("aria-checked", "true");
  });

  it("searches, filters to unread, pinned or archived, and sorts with pinned kept first", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await waitFor(() => expect(rows()).toHaveLength(4));
    await user.click(screen.getByRole("button", { name: "Search chats" }));
    await user.type(screen.getByPlaceholderText("Search chats"), "busan");
    await waitFor(() => expect(rows()).toEqual(["Busan trip"]));
    expect(calls.some((call) => call.path === "/v1/sessions?q=busan")).toBe(true);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(rows()).toHaveLength(4));

    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Unread" }));
    expect(rows()).toEqual(["Busan trip"]);
    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Pinned" }));
    expect(rows()).toEqual(["Assistant home"]);
    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "All chats" }));

    await user.click(screen.getByRole("button", { name: "Sort" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Title" }));
    expect(rows()).toEqual(["Assistant home", "Apartment search", "Busan trip", "Quarterly report"]);
    await user.click(screen.getByRole("button", { name: "Sort" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Oldest first" }));
    expect(rows()).toEqual(["Assistant home", "Apartment search", "Quarterly report", "Busan trip"]);

    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Archived" }));
    await waitFor(() => expect(rows()).toEqual(["Old invoice"]));
    expect(screen.getByTestId("path")).toHaveTextContent("/chats?show=archived");
  });

  it("opens the archived chats from ?show=archived and says when there are none to show", async () => {
    const user = userEvent.setup();
    setup("/chats?show=archived");
    await waitFor(() => expect(rows()).toEqual(["Old invoice"]));
    await user.click(screen.getByRole("button", { name: "Search chats" }));
    await user.type(screen.getByPlaceholderText("Search chats"), "nothing like it");
    expect(await screen.findByText("No chats found")).toBeInTheDocument();
  });

  it("keeps one project's chats with the filter, alongside the other filters", async () => {
    const user = userEvent.setup();
    setup("/chats?show=unread", { projects: true });
    await waitFor(() => expect(rows()).toEqual(["Busan trip"]));
    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Move" }));
    expect(screen.getByTestId("path")).toHaveTextContent("/chats?show=unread&project=p1");
    await waitFor(() => expect(screen.queryByRole("listitem")).toBeNull());

    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "All chats" }));
    await waitFor(() => expect(rows()).toEqual(["Apartment search"]));
    await user.click(screen.getByRole("button", { name: "Filter" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Move" }));
    await waitFor(() => expect(rows()).toHaveLength(4));
    expect(screen.getByTestId("path")).toHaveTextContent(/^\/chats$/);
  });

  it("unarchives from an archived chat's menu and in bulk", async () => {
    const user = userEvent.setup();
    const { calls, refreshSessions } = setup("/chats?show=archived");
    await waitFor(() => expect(rows()).toEqual(["Old invoice"]));
    await user.click(screen.getByRole("button", { name: "Options for Old invoice" }));
    expect(screen.queryByRole("menuitem", { name: "Archive chat" })).toBeNull();
    await user.click(await screen.findByRole("menuitem", { name: "Unarchive chat" }));
    await waitFor(() => expect(calls.filter((call) => call.method === "PATCH").map((call) => [call.path, call.body])).toEqual([["/v1/sessions/z", { archived: false }]]));
    expect(await screen.findByText("Conversation unarchived")).toBeInTheDocument();
    expect(refreshSessions).toHaveBeenCalled();

    await user.click(check("Old invoice"));
    await user.click(within(screen.getByRole("toolbar", { name: "Selected chats" })).getByRole("button", { name: "Unarchive" }));
    await waitFor(() => expect(calls.filter((call) => call.method === "PATCH").map((call) => [call.path, call.body])).toContainEqual(["/v1/sessions/z", { archived: false }]));
    expect(await screen.findByText("1 chat unarchived")).toBeInTheDocument();
  });

  it("archives an active chat from its menu", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await waitFor(() => expect(rows()).toHaveLength(4));
    await user.click(screen.getByRole("button", { name: "Options for Busan trip" }));
    expect(screen.queryByRole("menuitem", { name: "Unarchive chat" })).toBeNull();
    await user.click(await screen.findByRole("menuitem", { name: "Archive chat" }));
    await waitFor(() => expect(calls.filter((call) => call.method === "PATCH").map((call) => [call.path, call.body])).toEqual([["/v1/sessions/a", { archived: true }]]));
    expect(await screen.findByText("Conversation archived")).toBeInTheDocument();
    expect(screen.getByTestId("path")).toHaveTextContent("/chats");
  });

  it("selects a range with Shift, marks the unread ones read and archives the rest in bulk", async () => {
    const user = userEvent.setup();
    const { calls, refreshSessions } = setup();
    await waitFor(() => expect(rows()).toHaveLength(4));
    await user.click(check("Busan trip"));
    await user.keyboard("{Shift>}");
    await user.click(check("Apartment search"));
    await user.keyboard("{/Shift}");
    const bar = screen.getByRole("toolbar", { name: "Selected chats" });
    expect(within(bar).getByText("3 selected")).toBeInTheDocument();
    await user.click(within(bar).getByRole("button", { name: "Mark as read" }));
    await waitFor(() => expect(calls.filter((call) => call.method === "PATCH").map((call) => [call.path, call.body])).toEqual([["/v1/sessions/a", { read: true }]]));
    expect(await screen.findByText("1 chat marked as read")).toBeInTheDocument();
    expect(screen.queryByRole("toolbar", { name: "Selected chats" })).toBeNull();

    await user.click(check("Busan trip"));
    await user.click(check("Quarterly report"));
    await user.click(within(screen.getByRole("toolbar", { name: "Selected chats" })).getByRole("button", { name: "Archive" }));
    await waitFor(() =>
      expect(calls.filter((call) => call.method === "PATCH").map((call) => [call.path, call.body])).toEqual([
        ["/v1/sessions/a", { read: true }],
        ["/v1/sessions/a", { archived: true }],
        ["/v1/sessions/b", { archived: true }],
      ]),
    );
    expect(await screen.findByText("2 chats archived")).toBeInTheDocument();
    expect(refreshSessions).toHaveBeenCalled();
  });

  it("deletes the selection after confirming, keeps an agent's home chat, and reports a failure", async () => {
    const user = userEvent.setup();
    const { calls } = setup("/chats", { failDelete: true });
    await waitFor(() => expect(rows()).toHaveLength(4));
    await user.click(screen.getAllByRole("checkbox")[0]!);
    await user.click(within(screen.getByRole("toolbar", { name: "Selected chats" })).getByRole("button", { name: "Select all" }));
    expect(screen.getByText("4 selected")).toBeInTheDocument();
    await user.click(within(screen.getByRole("toolbar", { name: "Selected chats" })).getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete 3 chats?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls.filter((call) => call.method === "DELETE").map((call) => call.path)).toEqual(["/v1/sessions/a", "/v1/sessions/b", "/v1/sessions/c"]));
    expect(await screen.findByText("2 chats deleted")).toBeInTheDocument();
    expect(await screen.findByText("1 chat couldn’t be changed: Still running")).toBeInTheDocument();
  });

  it("clears the selection with Escape", async () => {
    const user = userEvent.setup();
    setup();
    await waitFor(() => expect(rows()).toHaveLength(4));
    await user.click(check("Busan trip"));
    expect(screen.getByRole("toolbar", { name: "Selected chats" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("toolbar", { name: "Selected chats" })).toBeNull();
    expect(check("Busan trip")).not.toBeChecked();
  });
});
