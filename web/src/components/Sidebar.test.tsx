import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it } from "vitest";
import type { AgentSummary } from "../api/types";
import { isUnread, people, type AgentsState } from "../hooks/useAgents";
import { seen } from "../state/seen";
import { agentSummary } from "../test/agents";
import { rowTime, Sidebar } from "./Sidebar";

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

function setup(agents: AgentSummary[] | null, path = "/history") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Sidebar team={team(agents)} />
    </MemoryRouter>,
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

  it("offers History, Workspace and Settings, and no new chat", () => {
    setup(roster());
    expect(screen.getByRole("link", { name: "History" })).toHaveAttribute("href", "/history");
    expect(screen.getByRole("link", { name: "Workspace" })).toHaveAttribute("href", "/workspace");
    expect(screen.queryByRole("link", { name: "New chat" })).toBeNull();
    expect(screen.queryByRole("searchbox")).toBeNull();
  });

  it("hints at teammates while the assistant is alone", () => {
    setup([agentSummary("chief", "openside")]);
    expect(screen.getByText(/openside can bring in a teammate/)).toBeInTheDocument();
  });
});

describe("row state", () => {
  it("shows live activity, working dots and Needs you", () => {
    setup([
      agentSummary("chief", "openside", { needs_you: true, session_status: "waiting_human" }),
      agentSummary("theo", "Theo", { session_status: "running", activity: "Checking prices", last_activity: "Old preview" }),
      agentSummary("mina", "Mina", { last_activity: "Here is the weekly summary" }),
    ]);
    expect(screen.getByRole("link", { name: "openside, Needs you" })).toBeInTheDocument();
    const theo = screen.getByRole("link", { name: "Theo, Working" });
    expect(within(theo).getByText("Checking prices")).toHaveClass("live");
    expect(theo.querySelector(".avatar-working")).not.toBeNull();
    expect(within(screen.getByRole("link", { name: /^Mina/ })).getByText("Here is the weekly summary")).toBeInTheDocument();
  });

  it("falls back to the role when there is no message yet", () => {
    setup([agentSummary("chief", "openside"), agentSummary("mina", "Mina", { persona: { role: "Watches prices", voice: "", responsibilities: ["x"], boundaries: [] } })]);
    expect(within(screen.getByRole("link", { name: /^Mina/ })).getByText("Watches prices")).toBeInTheDocument();
  });

  it("stamps today with the time, this week with the weekday and older with the date", () => {
    const now = new Date(2026, 9, 12, 15, 0);
    expect(rowTime(new Date(2026, 9, 12, 9, 5).toISOString(), now)).toMatch(/9.*05/);
    expect(rowTime(new Date(2026, 9, 9, 9, 5).toISOString(), now)).not.toMatch(/\d{1,2}:\d{2}/);
    expect(rowTime(new Date(2026, 8, 1, 9, 5).toISOString(), now)).toMatch(/1/);
    expect(rowTime(null, now)).toBe("");
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
