import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PersonTab, PersonTabGroup } from "../api/types";
import { ClientContext } from "../context";
import { AgentTabsSettledContext, type OwnedTab } from "../hooks/useAgentTabs";
import { AGENT_TAB_GROUP_TITLE, PersonBrowserContext, type PersonBrowser } from "../hooks/usePersonBrowser";
import { fakeClient, json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { COLLAPSED_TAB_GROUPS_KEY, SidebarTabs } from "./SidebarTabs";

const owned = (over: Partial<OwnedTab>): OwnedTab => ({ tab_id: "t", title: "Page", url: "https://shop.example/item", opened_by: "agent", owner_session_id: "home-chief", ...over });

let nextId = 1;
const personTab = (over: Partial<PersonTab>): PersonTab => {
  const id = nextId++;
  return { tab_id: id, window_id: 1, index: id, title: `Tab ${id}`, url: `https://site${id}.example/`, favicon_url: null, active: false, pinned: false, audible: false, muted: false, discarded: false, group_id: null, ...over };
};

function browser(over: Partial<PersonBrowser> = {}): PersonBrowser {
  return { connected: true, focusedWindow: 1, tabs: [], groups: [], bookmarks: [], bookmarkBar: "1", command: vi.fn(() => Promise.resolve()), refreshBookmarks: vi.fn(), ...over };
}

function setup(agentTabs: OwnedTab[], person: PersonBrowser = browser(), { settled = true, onPicked }: { settled?: boolean; onPicked?: () => void } = {}) {
  // Favicons are missing, so rows fall back to the globe icon.
  const { client, calls } = fakeClient(() => json(404, { code: "not_found", message: "none" }));
  const view = render(
    <ClientContext.Provider value={client}>
      <PersonBrowserContext.Provider value={person}>
        <AgentTabsSettledContext.Provider value={settled}>
          <ToastProvider>
            <MemoryRouter>
              <SidebarTabs agentTabs={agentTabs} pageLabel={(path) => (path === "/settings/models" ? "Settings › Models" : null)} onPicked={onPicked} />
            </MemoryRouter>
          </ToastProvider>
        </AgentTabsSettledContext.Provider>
      </PersonBrowserContext.Provider>
    </ClientContext.Provider>,
  );
  return { ...view, person, calls };
}

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("SidebarTabs", () => {
  it("starts with New tab, which opens a tab in the person's browser", async () => {
    const user = userEvent.setup();
    const { person } = setup([]);
    const section = screen.getByRole("region", { name: "Tabs" });
    await user.click(within(section).getByRole("button", { name: "New tab" }));
    expect(person.command).toHaveBeenCalledWith({ command: "create" });
  });

  it("opens a fresh openside tab itself while the browser is not connected", async () => {
    const user = userEvent.setup();
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    const { person } = setup([], browser({ connected: false }));
    await user.click(screen.getByRole("button", { name: "New tab" }));
    expect(open).toHaveBeenCalledWith("/", "_blank", "noopener");
    expect(person.command).not.toHaveBeenCalled();
  });

  it("lists agent tabs under Agent Tabs, each opening its chat with the browser sheet (an agent's home too)", () => {
    setup([owned({ tab_id: "a", title: "Cart", owner_session_id: "home-chief" }), owned({ tab_id: "b", title: "Report", owner_session_id: "s9", url: "https://air.example/r" })]);
    expect(screen.getByRole("button", { name: "Agent Tabs, 2" })).toHaveAttribute("aria-expanded", "true");
    const links = within(screen.getByRole("list", { name: "Agent Tabs" })).getAllByRole("link");
    expect(links.map((link) => link.getAttribute("href"))).toEqual(["/sessions/home-chief?browser=open", "/sessions/s9?browser=open"]);
    expect(links[1]).toHaveAttribute("title", "Report");
    expect(links[1]).toHaveAccessibleDescription("https://air.example/r");
  });

  it("lists the person's tabs, leaving out the Agent Tabs group the ledger already shows", () => {
    const groups: PersonTabGroup[] = [{ group_id: 7, window_id: 1, title: AGENT_TAB_GROUP_TITLE, color: "grey", collapsed: false }];
    setup(
      [],
      browser({
        tabs: [personTab({ title: "Docs", active: true }), personTab({ title: "Agent page", group_id: 7 }), personTab({ title: "", url: `${window.location.origin}/settings/models` })],
        groups,
      }),
    );
    const rows = within(screen.getByRole("region", { name: "Tabs" })).getAllByRole("button").filter((button) => button.classList.contains("sidebar-row"));
    // New tabs go at the top by default (Settings › Appearance › New tab position), so the newest is first.
    expect(rows.map((row) => row.textContent)).toEqual(["New tab", "Settings›Models", "Docs"]);
    expect(rows[2]).toHaveAttribute("aria-current", "page");
    expect(rows[1]).not.toHaveAttribute("aria-current");
  });

  it("keeps the browser's Agent Tabs group until the agents' tab list has answered", () => {
    const groups: PersonTabGroup[] = [{ group_id: 7, window_id: 1, title: AGENT_TAB_GROUP_TITLE, color: "grey", collapsed: false }];
    setup([], browser({ tabs: [personTab({ title: "Agent page", group_id: 7 })], groups }), { settled: false });
    expect(screen.getByRole("button", { name: "Agent page" })).toBeInTheDocument();
  });

  it("closes the phone drawer after switching tabs or opening a new one", async () => {
    const user = userEvent.setup();
    const onPicked = vi.fn();
    setup([], browser({ tabs: [personTab({ title: "Docs" })] }), { onPicked });
    await user.click(screen.getByRole("button", { name: "Docs" }));
    expect(onPicked).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "New tab" }));
    expect(onPicked).toHaveBeenCalledTimes(2);
  });

  it("offers Add to bookmarks only for web pages", async () => {
    setup([], browser({ tabs: [personTab({ title: "Flags", url: "chrome://flags" })] }));
    fireEvent.contextMenu(screen.getByRole("button", { name: "Flags" }));
    expect(await screen.findByRole("menuitem", { name: "Add tab to bookmarks" })).toHaveAttribute("aria-disabled", "true");
  });

  it("returns focus to the row when its menu closes with Escape", async () => {
    const user = userEvent.setup();
    setup([], browser({ tabs: [personTab({ title: "Docs" })] }));
    const row = screen.getByRole("button", { name: "Docs" });
    row.focus();
    // Shift+F10 and the context-menu key reach the page as a contextmenu event on the focused row.
    fireEvent.contextMenu(row);
    await screen.findByRole("menu", { name: "Options for Docs" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(row).toHaveFocus();
  });

  it("switches to a tab on click and closes it from its × button", async () => {
    const user = userEvent.setup();
    const tab = personTab({ title: "Docs" });
    const { person } = setup([], browser({ tabs: [tab] }));
    await user.click(screen.getByRole("button", { name: "Docs" }));
    expect(person.command).toHaveBeenLastCalledWith({ command: "activate", tab_id: tab.tab_id });
    await user.click(screen.getByRole("button", { name: "Close Docs" }));
    expect(person.command).toHaveBeenLastCalledWith({ command: "close", tab_ids: [tab.tab_id] });
  });

  it("groups the person's tab groups with a count and folds them", async () => {
    const user = userEvent.setup();
    const groups: PersonTabGroup[] = [{ group_id: 3, window_id: 1, title: "Trip", color: "blue", collapsed: false }];
    setup([], browser({ tabs: [personTab({ title: "Flights", group_id: 3 }), personTab({ title: "Hotels", group_id: 3 })], groups }));
    const toggle = screen.getByRole("button", { name: "Trip, 2" });
    expect(within(screen.getByRole("list", { name: "Trip" })).getAllByRole("button", { name: /^(Flights|Hotels)$/ })).toHaveLength(2);
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("list", { name: "Trip" })).toBeNull();
  });

  it("keeps a folded group folded across remounts and forgets groups that are gone", async () => {
    const user = userEvent.setup();
    const first = setup([owned({ tab_id: "a" })]);
    await user.click(screen.getByRole("button", { name: "Agent Tabs, 1" }));
    first.unmount();
    const second = setup([owned({ tab_id: "a" })]);
    expect(screen.getByRole("button", { name: "Agent Tabs, 1" })).toHaveAttribute("aria-expanded", "false");
    second.unmount();
    // No tabs at all (a failed poll) prunes nothing.
    setup([], browser({ groups: [] })).unmount();
    expect(JSON.parse(localStorage.getItem(COLLAPSED_TAB_GROUPS_KEY) ?? "[]")).toEqual(["agent"]);
    // Agent tabs gone while another group is listed: the fold is forgotten.
    setup([], browser({ tabs: [personTab({ group_id: 3 })], groups: [{ group_id: 3, window_id: 1, title: "Trip", color: "blue", collapsed: false }] })).unmount();
    expect(localStorage.getItem(COLLAPSED_TAB_GROUPS_KEY)).toBeNull();
  });
});
