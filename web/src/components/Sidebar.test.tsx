import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PersonBookmark, Session } from "../api/types";
import { ClientContext } from "../context";
import type { AgentsState } from "../hooks/useAgents";
import { PersonBrowserContext, type PersonBrowser } from "../hooks/usePersonBrowser";
import { setLocale } from "../i18n/core";
import { saveSidebarPrefs, ShellContext, SIDEBAR_PREFS_KEY } from "../shell";
import { agentSummary } from "../test/agents";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { Sidebar } from "./Sidebar";
import { TAB_DRAG_TYPE } from "./SidebarTabs";

const session = (id: string, over: Partial<Session> = {}): Session =>
  ({ id, title: `Chat ${id}`, activity: "", agent_id: null, archived: false, browser_space: "shared", incognito: false, last_seq: 1, mode: "guard", model: "m", pinned: false, queue: [], status: "idle", unread: false, confirmation: "ask_first", ...over }) as Session;

const team: AgentsState = { agents: [agentSummary("chief", "openside", { home_session_id: "home" })], error: null, offline: false, refresh: () => {} };
const toggleSidebar = vi.fn();
const openSearch = vi.fn();
const refreshSessions = vi.fn();

function browser(over: Partial<PersonBrowser> = {}): PersonBrowser {
  return { connected: true, focusedWindow: 1, tabs: [], groups: [], bookmarks: [], bookmarkBar: "1", command: vi.fn(() => Promise.resolve()), refreshBookmarks: vi.fn(), ...over };
}

interface Options {
  sessions?: Session[];
  path?: string;
  onClose?: () => void;
  person?: PersonBrowser;
  respond?: (call: Call) => Response | undefined;
  roster?: AgentsState;
}

function setup({ sessions = [], path = "/", onClose, person = browser(), respond, roster = team }: Options = {}) {
  const { client, calls } = fakeClient((call) => respond?.(call) ?? (call.path === "/v1/settings/preferences" ? json(200, { name: "Work" }) : json(404, { code: "not_found", message: "none" })));
  const view = render(
    <ClientContext.Provider value={client}>
      <ShellContext.Provider value={{ openDrawer: () => undefined, openSearch, refreshSessions, sidebarCollapsed: false, toggleSidebar, needsYou: 0 }}>
        <PersonBrowserContext.Provider value={person}>
          <ToastProvider>
            <MemoryRouter initialEntries={[path]}>
              <Sidebar team={roster} tabs={[]} sessions={sessions} pageLabel={() => null} onClose={onClose} />
              <Path />
            </MemoryRouter>
          </ToastProvider>
        </PersonBrowserContext.Provider>
      </ShellContext.Provider>
    </ClientContext.Provider>,
  );
  return { ...view, calls, person };
}

function Path() {
  return <output data-testid="path">{useLocation().pathname}</output>;
}

const chatRows = () => within(screen.getByRole("region", { name: "Chats" })).getAllByRole("link").map((link) => link.textContent);

beforeEach(() => {
  toggleSidebar.mockClear();
  openSearch.mockClear();
  refreshSessions.mockClear();
});
afterEach(() => localStorage.clear());

describe("Sidebar", () => {
  it("shows Bookmarks, Chats and Tabs in order, with no team or persona navigation", () => {
    setup();
    expect(screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent)).toEqual(["Bookmarks", "Chats", "Tabs"]);
    expect(screen.queryByRole("link", { name: /Workspace|Team/ })).toBeNull();
  });

  it("lists New Chat, the three most recent chats and Show all", async () => {
    const user = userEvent.setup();
    setup({ sessions: [session("a"), session("b"), session("c"), session("d"), session("x", { archived: true })] });
    expect(chatRows()).toEqual(["New Chat", "Chat a", "Chat b", "Chat c", "Show all"]);
    await user.click(screen.getByRole("link", { name: "Show all" }));
    expect(screen.getByTestId("path")).toHaveTextContent("/chats");
    await user.click(screen.getByRole("link", { name: "New Chat" }));
    expect(screen.getByTestId("path")).toHaveTextContent(/^\/$/);
  });

  it("names untitled chats in the UI language, leaving named ones as they are", () => {
    setLocale("ko");
    try {
      setup({ sessions: [session("a", { title: "New session" }), session("b", { title: "" }), session("c", { title: "Acme plans" })] });
      const rows = within(screen.getByRole("region", { name: "채팅" })).getAllByRole("link").map((link) => link.textContent);
      expect(rows).toEqual(["새 채팅", "새 채팅", "새 채팅", "Acme plans", "모두 보기"]);
    } finally {
      setLocale("en");
    }
  });

  it("keeps Show all with no chats yet, so the Chats tab and its archived chats stay reachable", async () => {
    const user = userEvent.setup();
    setup({ sessions: [session("x", { archived: true })], path: "/" });
    expect(chatRows()).toEqual(["New Chat", "Show all"]);
    expect(screen.getByRole("link", { name: "Show all" })).not.toHaveAttribute("aria-current");
    await user.click(screen.getByRole("link", { name: "Show all" }));
    expect(screen.getByTestId("path")).toHaveTextContent("/chats");
    expect(screen.getByRole("link", { name: "Show all" })).toHaveAttribute("aria-current", "page");
  });

  it("follows the recent-chats choice from Appearance, also when it changes while shown", () => {
    saveSidebarPrefs({ recentChats: 5 });
    setup({ sessions: ["a", "b", "c", "d", "e", "f"].map((id) => session(id)) });
    expect(chatRows()).toHaveLength(7);
    fireEvent(window, new StorageEvent("storage", { key: SIDEBAR_PREFS_KEY }));
    expect(chatRows()).toHaveLength(7);
    act(() => void saveSidebarPrefs({ recentChats: 10, bookmarks: false }));
    expect(chatRows()).toHaveLength(8);
    expect(screen.queryByRole("region", { name: "Bookmarks" })).toBeNull();
  });

  it("marks the open chat, a working chat and one that needs the person", () => {
    setup({ sessions: [session("a"), session("b", { status: "running" }), session("c", { status: "waiting_human" })], path: "/sessions/a" });
    expect(screen.getByRole("link", { name: "Chat a" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Chat b, Working" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Chat c, Needs you" })).toBeInTheDocument();
  });

  it("keeps Archive on a working or waiting chat beside its mark", () => {
    setup({ sessions: [session("b", { status: "running" }), session("c", { status: "waiting_human" })] });
    expect(screen.getByRole("button", { name: "Archive Chat b" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Archive Chat c" })).toBeInTheDocument();
  });

  it("offers Delete only once the roster has loaded, and never for an agent's home", async () => {
    const first = setup({ sessions: [session("a")], roster: { ...team, agents: null } });
    fireEvent.contextMenu(screen.getByRole("link", { name: "Chat a" }));
    await screen.findByRole("menu", { name: "Options for Chat a" });
    expect(screen.queryByRole("menuitem", { name: "Delete chat" })).toBeNull();
    first.unmount();
    setup({ sessions: [session("home", { title: "Home chat" })] });
    fireEvent.contextMenu(screen.getByRole("link", { name: "Home chat" }));
    await screen.findByRole("menu", { name: "Options for Home chat" });
    expect(screen.queryByRole("menuitem", { name: "Delete chat" })).toBeNull();
  });

  it("archives a chat from its hover button and leaves it when it was open", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ sessions: [session("a")], path: "/sessions/a", respond: (call) => (call.method === "PATCH" ? json(200, session("a", { archived: true })) : undefined) });
    await user.click(screen.getByRole("button", { name: "Archive Chat a" }));
    await waitFor(() => expect(calls.find((call) => call.method === "PATCH")?.body).toEqual({ archived: true }));
    expect(refreshSessions).toHaveBeenCalled();
    expect(screen.getByTestId("path")).toHaveTextContent(/^\/$/);
  });

  it("opens a chat's menu on right-click and pins it", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ sessions: [session("a")], respond: (call) => (call.method === "PATCH" ? json(200, session("a", { pinned: true })) : undefined) });
    fireEvent.contextMenu(screen.getByRole("link", { name: "Chat a" }));
    const menu = await screen.findByRole("menu", { name: "Options for Chat a" });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Pin chat", "Rename chat", "Archive chat", "Delete chat", "Bookmark chat", "Mark as unread", "Copy session ID", "Open in new tab"]);
    await user.click(within(menu).getByRole("menuitem", { name: "Pin chat" }));
    await waitFor(() => expect(calls.find((call) => call.method === "PATCH")?.body).toEqual({ pinned: true }));
  });

  it("bookmarks a chat into the browser's Bookmarks bar", async () => {
    const user = userEvent.setup();
    const { calls, person } = setup({ sessions: [session("a")], respond: (call) => (call.method === "POST" ? json(200, { id: "9", parent_id: "1", title: "Chat a", url: "x", children: [] }) : undefined) });
    fireEvent.contextMenu(screen.getByRole("link", { name: "Chat a" }));
    await user.click(await screen.findByRole("menuitem", { name: "Bookmark chat" }));
    await waitFor(() => expect(calls.find((call) => call.method === "POST")?.body).toEqual({ parent_id: "1", title: "Chat a", url: `${window.location.origin}/sessions/a` }));
    expect(person.refreshBookmarks).toHaveBeenCalled();
  });

  it("moves between rows with the arrow keys", async () => {
    const user = userEvent.setup();
    setup({ sessions: [session("a")] });
    screen.getByRole("link", { name: "New Chat" }).focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("link", { name: "Chat a" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("link", { name: "Show all" })).toHaveFocus();
    await user.keyboard("{End}");
    expect(screen.getByRole("button", { name: "New tab" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("link", { name: "New Chat" })).toHaveFocus();
  });

  it("shows a drop box until a bookmark exists, and saves a tab dropped on it", async () => {
    const { calls, person } = setup({ respond: (call) => (call.method === "POST" ? json(200, { id: "9", parent_id: "1", title: "Docs", url: "https://docs.example/", children: [] }) : undefined) });
    const section = screen.getByRole("region", { name: "Bookmarks" });
    expect(within(section).getByText("Drag tabs here to bookmark them")).toBeInTheDocument();
    const data = { [TAB_DRAG_TYPE]: JSON.stringify({ title: "Docs", url: "https://docs.example/" }) };
    const dataTransfer = { types: [TAB_DRAG_TYPE], getData: (type: string) => data[type as keyof typeof data] ?? "", dropEffect: "none" };
    fireEvent.dragOver(section, { dataTransfer });
    fireEvent.drop(section, { dataTransfer });
    await waitFor(() => expect(calls.find((call) => call.method === "POST")?.body).toEqual({ parent_id: "1", title: "Docs", url: "https://docs.example/" }));
    expect(person.refreshBookmarks).toHaveBeenCalled();
  });

  it("refuses to save a dropped address that is not a web page", async () => {
    for (const url of ["javascript:alert(1)", "data:text/html,hi", "file:///etc/passwd", "blob:https://a.example/1", "https://user:pw@a.example/"]) {
      const { calls, unmount } = setup();
      const section = screen.getByRole("region", { name: "Bookmarks" });
      const data = { [TAB_DRAG_TYPE]: JSON.stringify({ title: "Bad", url }), "text/uri-list": url };
      const dataTransfer = { types: [TAB_DRAG_TYPE, "text/uri-list"], getData: (type: string) => data[type as keyof typeof data] ?? "", dropEffect: "none" };
      fireEvent.drop(section, { dataTransfer });
      expect(await screen.findByText("Only web pages can be bookmarked.")).toBeInTheDocument();
      expect(calls.some((call) => call.method === "POST"), url).toBe(false);
      unmount();
    }
    const { calls } = setup();
    const link = "javascript:alert(1)";
    fireEvent.drop(screen.getByRole("region", { name: "Bookmarks" }), { dataTransfer: { types: ["text/uri-list"], getData: (type: string) => (type === "text/uri-list" ? link : ""), dropEffect: "none" } });
    expect(await screen.findByText("Only web pages can be bookmarked.")).toBeInTheDocument();
    expect(calls.some((call) => call.method === "POST")).toBe(false);
  });

  it("opens a bookmark, closes the drawer, and says so when the browser cannot switch", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const bookmarks: PersonBookmark[] = [{ id: "5", parent_id: "1", title: "Docs", url: "https://docs.example/", children: [] }];
    const tabs = [{ tab_id: 4, window_id: 1, index: 0, title: "Docs", url: "https://docs.example/", favicon_url: null, active: false, pinned: false, audible: false, muted: false, discarded: false, group_id: null }];
    const command = vi.fn(() => Promise.reject(new Error("gone")));
    setup({ onClose, person: browser({ bookmarks, tabs, command }) });
    await user.click(within(screen.getByRole("region", { name: "Bookmarks" })).getByRole("button", { name: "Docs" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(command).toHaveBeenCalledWith({ command: "activate", tab_id: 4 });
    expect(await screen.findByText("Request failed")).toBeInTheDocument();
  });

  it("lists bookmarks from the browser and removes one", async () => {
    const user = userEvent.setup();
    const bookmarks: PersonBookmark[] = [{ id: "5", parent_id: "1", title: "Docs", url: "https://docs.example/", children: [] }];
    const { calls } = setup({ person: browser({ bookmarks }), respond: (call) => (call.method === "DELETE" ? new Response(null, { status: 204 }) : undefined) });
    expect(within(screen.getByRole("region", { name: "Bookmarks" })).getByRole("button", { name: "Docs" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove bookmark Docs" }));
    await waitFor(() => expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/browser/person/bookmarks/5")).toBe(true));
  });

  it("collapses on desktop and closes in the drawer", async () => {
    const user = userEvent.setup();
    const first = setup();
    await user.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(toggleSidebar).toHaveBeenCalledTimes(1);
    first.unmount();
    const onClose = vi.fn();
    setup({ onClose });
    await user.click(screen.getByRole("button", { name: "Close menu" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("account menu", () => {
  it("lists Aside's items in order under the person's name and opens Settings", async () => {
    const user = userEvent.setup();
    setup();
    const trigger = await screen.findByRole("button", { name: "Account: Work" });
    await user.click(trigger);
    const menu = await screen.findByRole("menu", { name: "Account" });
    expect(within(menu).getByText("Profiles")).toBeInTheDocument();
    expect(within(menu).getAllByRole("menuitem").map((item) => item.querySelector(".ui-menu-label")?.textContent)).toEqual([
      "Work",
      "New profile",
      "Bookmarks",
      "Downloads",
      "Extensions",
      "History",
      "Developers",
      "Settings",
      "New Tab",
      "Incognito Window",
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: /^Settings/ }));
    expect(screen.getByTestId("path")).toHaveTextContent("/settings");
  });

  it("opens the profile's options from its … and incognito through the browser", async () => {
    const user = userEvent.setup();
    const { person } = setup();
    await user.click(await screen.findByRole("button", { name: "Account: Work" }));
    await user.click(await screen.findByRole("menuitem", { name: /^Incognito Window/ }));
    expect(person.command).toHaveBeenCalledWith({ command: "new_incognito_window" });
    await user.click(screen.getByRole("button", { name: "Account: Work" }));
    await user.click(await screen.findByRole("menuitem", { name: "Work" }));
    const options = await screen.findByRole("menu", { name: "Work" });
    expect(within(options).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Edit profile", "Delete profile"]);
    // Submenu popups sit under `pointer-events: none` in jsdom, so choose the row from the keyboard.
    within(options).getByRole("menuitem", { name: "Edit profile" }).focus();
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("path")).toHaveTextContent("/settings/account");
  });

  it("opens browser pages through the person's browser, and says so when it is not connected", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ respond: (call) => (call.path === "/v1/browser/person/tabs/command" ? json(409, { code: "person_browser_not_connected", message: "no" }) : undefined) });
    await user.click(await screen.findByRole("button", { name: "Account: Work" }));
    await user.click(await screen.findByRole("menuitem", { name: "New profile" }));
    await waitFor(() => expect(calls.find((call) => call.path === "/v1/browser/person/tabs/command")?.body).toEqual({ command: "create", url: "chrome://profile-picker" }));
    expect(await screen.findByText("Your browser isn’t connected to openside")).toBeInTheDocument();
  });

  it("shows Developers with Performance, Task manager, Developer tools and Remote debugging, and opens a native tool", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ respond: (call) => (call.path === "/v1/browser/person/native-tools" ? json(409, { code: "browser_not_running", message: "no" }) : undefined) });
    await user.click(await screen.findByRole("button", { name: "Account: Work" }));
    await user.click(await screen.findByRole("menuitem", { name: "Developers" }));
    const sub = await screen.findByRole("menu", { name: "Developers" });
    expect(within(sub).getAllByRole("menuitem").map((item) => item.querySelector(".ui-menu-label")?.textContent)).toEqual(["Performance", "Task manager", "Developer tools", "Advanced"]);
    // Submenu popups sit under `pointer-events: none` in jsdom, so choose the row from the keyboard.
    within(sub).getByRole("menuitem", { name: "Task manager" }).focus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(calls.find((call) => call.path === "/v1/browser/person/native-tools")?.body).toEqual({ tool: "task_manager" }));
    expect(await screen.findByText("Open your openside browser first.")).toBeInTheDocument();
  });

  it("opens the search palette from the bar", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: "Search" }));
    expect(openSearch).toHaveBeenCalledTimes(1);
  });
});
