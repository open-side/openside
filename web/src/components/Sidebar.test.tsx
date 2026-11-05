import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "../api/types";
import { ClientContext } from "../context";
import { isUnread, people, type AgentsState } from "../hooks/useAgents";
import { ShellContext } from "../shell";
import { seen } from "../state/seen";
import { agentSummary } from "../test/agents";
import { fakeClient, json } from "../test/fakeDaemon";
import { Sidebar } from "./Sidebar";

const roster = (): AgentSummary[] => [
  agentSummary("mina", "Mina", { last_message_at: "2026-10-12T08:00:00Z" }),
  agentSummary("theo", "Theo", { last_message_at: "2026-10-12T10:00:00Z" }),
  agentSummary("chief", "openside", { last_message_at: "2026-10-01T08:00:00Z" }),
  agentSummary("ada", "Ada", { pinned: true, last_message_at: "2026-10-02T08:00:00Z" }),
  agentSummary("old", "Old", { status: "retired" }),
  agentSummary("quiet", "Quiet", { hidden: true }),
];

function team(agents: AgentSummary[] | null): AgentsState {
  return { agents, error: null, offline: false, refresh: () => {} };
}

const toggleSidebar = vi.fn();

function setup(agents: AgentSummary[] | null, path = "/history", onClose?: () => void) {
  // Pinned tabs and agent tabs are absent: those sections render nothing.
  const { client } = fakeClient(() => json(404, { code: "not_found", message: "none" }));
  return render(
    <ClientContext.Provider value={client}>
      <ShellContext.Provider value={{ openDrawer: () => undefined, refreshSessions: () => undefined, sidebarCollapsed: false, toggleSidebar }}>
        <MemoryRouter initialEntries={[path]}>
          <Sidebar team={team(agents)} tabs={[]} onClose={onClose} />
        </MemoryRouter>
      </ShellContext.Provider>
    </ClientContext.Provider>,
  );
}

const names = () => within(screen.getByRole("list", { name: "Team" })).getAllByRole("link").map((link) => link.querySelector(".person-name")?.textContent);

beforeEach(() => seen.reset());

describe("people order", () => {
  it("puts the assistant first, then pinned teammates, then the latest conversation; retired and hidden stay out", () => {
    const { chief, teammates, hidden } = people(roster());
    expect(chief?.id).toBe("chief");
    expect(teammates.map((agent) => agent.id)).toEqual(["ada", "theo", "mina"]);
    expect(hidden.map((agent) => agent.id)).toEqual(["quiet"]);
  });

  it("renders the same order and links each person to their thread", () => {
    setup(roster());
    expect(names()).toEqual(["openside", "Ada", "Theo", "Mina"]);
    expect(within(screen.getByRole("list", { name: "Team" })).getByRole("link", { name: /^openside/ }).querySelector(".avatar svg")).not.toBeNull();
    expect(screen.getByRole("link", { name: /^Theo/ })).toHaveAttribute("href", "/agents/theo");
    expect(screen.queryByText("Old")).toBeNull();
  });

  it("folds hidden teammates behind a disclosure", async () => {
    const user = userEvent.setup();
    setup(roster());
    expect(screen.queryByRole("link", { name: /^Quiet/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Hidden · 1" }));
    expect(within(screen.getByRole("list", { name: "Hidden teammates" })).getByRole("link", { name: /^Quiet/ })).toBeInTheDocument();
  });

  it("offers New task, Show all, History, Workspace and Settings, and no new chat", () => {
    setup(roster());
    expect(screen.getByRole("link", { name: "New task" })).toHaveAttribute("href", "/");
    expect(screen.getByRole("link", { name: "Show all" })).toHaveAttribute("href", "/history");
    expect(screen.getByRole("heading", { name: "Chats" })).toBeInTheDocument();
    const nav = screen.getByRole("navigation", { name: "Sections" });
    expect(within(nav).getByRole("link", { name: "History" })).toHaveAttribute("href", "/history");
    expect(within(nav).getByRole("link", { name: "Workspace" })).toHaveAttribute("href", "/workspace");
    expect(screen.queryByRole("link", { name: "New chat" })).toBeNull();
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(screen.queryByRole("region", { name: "Pinned" })).toBeNull();
  });

  it("marks New task current on Home", () => {
    setup(roster(), "/");
    expect(screen.getByRole("link", { name: "New task" })).toHaveAttribute("aria-current", "page");
  });

  it("collapses on desktop and closes in the drawer", async () => {
    const user = userEvent.setup();
    const { unmount } = setup(roster());
    await user.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(toggleSidebar).toHaveBeenCalledTimes(1);
    unmount();
    const onClose = vi.fn();
    setup(roster(), "/history", onClose);
    expect(screen.queryByRole("button", { name: "Collapse sidebar" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Close menu" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("row state", () => {
  it("shows a live dot while working and Needs you; activity goes to the tooltip", () => {
    setup([
      agentSummary("chief", "openside", { needs_you: true, session_status: "waiting_human" }),
      agentSummary("theo", "Theo", { session_status: "running", activity: "Checking prices", last_activity: "Old preview" }),
      agentSummary("mina", "Mina", { last_activity: "Here is the weekly summary" }),
    ]);
    expect(within(screen.getByRole("link", { name: "openside, Needs you" })).getByText("Needs you")).toHaveClass("row-badge");
    const theo = screen.getByRole("link", { name: "Theo, Working" });
    expect(theo.querySelector(".live-dot")).not.toBeNull();
    expect(theo).toHaveAttribute("title", "Theo · Checking prices");
    expect(screen.getByRole("link", { name: /^Mina/ })).toHaveAttribute("title", "Mina · Here is the weekly summary");
    expect(screen.queryByText("Here is the weekly summary")).toBeNull();
  });

  it("falls back to the role when there is no message yet", () => {
    setup([agentSummary("chief", "openside"), agentSummary("mina", "Mina", { persona: { role: "Watches prices", voice: "", responsibilities: ["x"], boundaries: [] } })]);
    expect(screen.getByRole("link", { name: /^Mina/ })).toHaveAttribute("title", "Mina · Watches prices");
  });
});

describe("unread", () => {
  it("treats everything as read on a fresh device, then flags newer events", () => {
    seen.seed({ chief: 4, mina: 2 });
    const mina = agentSummary("mina", "Mina", { last_seq: 2 });
    expect(isUnread(mina, seen.cursors(), null)).toBe(false);
    expect(isUnread({ ...mina, last_seq: 3 }, seen.cursors(), null)).toBe(true);
    // The open thread is being read right now.
    expect(isUnread({ ...mina, last_seq: 3 }, seen.cursors(), "mina")).toBe(false);
    seen.mark("mina", 3);
    expect(isUnread({ ...mina, last_seq: 3 }, seen.cursors(), null)).toBe(false);
  });

  it("never moves a cursor backwards", () => {
    seen.seed({});
    seen.mark("mina", 5);
    seen.mark("mina", 2);
    expect(seen.cursors().mina).toBe(5);
  });

  it("shows a teammate hired after the first roster as unread", () => {
    seen.seed({ chief: 1 });
    expect(isUnread(agentSummary("new", "New", { last_seq: 1 }), seen.cursors(), null)).toBe(true);
  });

  it("marks the row with a dot and updates when the thread is read", () => {
    seen.seed({ chief: 1, mina: 1 });
    setup([agentSummary("chief", "openside"), agentSummary("mina", "Mina", { last_seq: 3 })]);
    expect(screen.getByRole("link", { name: "Mina, Unread" }).querySelector(".unread-dot")).not.toBeNull();
    act(() => seen.mark("mina", 3));
    expect(screen.getByRole("link", { name: "Mina" }).querySelector(".unread-dot")).toBeNull();
  });

  it("does not flag the thread that is open", () => {
    seen.seed({ chief: 1, mina: 1 });
    setup([agentSummary("chief", "openside"), agentSummary("mina", "Mina", { last_seq: 3 })], "/agents/mina");
    expect(screen.getByRole("link", { name: "Mina" })).toHaveAttribute("aria-current", "page");
  });
});
