import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import type { Session } from "../api/types";
import { ClientContext } from "../context";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { agentSummary } from "../test/agents";
import { fakeClient, json } from "../test/fakeDaemon";
import { COLLAPSED_TAB_GROUPS_KEY, SidebarTabs } from "./SidebarTabs";

const agents = [agentSummary("chief", "openside", { home_session_id: "home-chief" }), agentSummary("mina", "Mina", { home_session_id: "home-mina" })];

const tab = (over: Partial<OwnedTab>): OwnedTab => ({ tab_id: "t", title: "Page", url: "https://shop.example/item", opened_by: "agent", owner_session_id: "home-chief", ...over });

function setup(tabs: OwnedTab[], sessions?: Session[]) {
  // Favicons are missing, so rows fall back to the globe icon.
  const { client } = fakeClient(() => json(404, { code: "not_found", message: "none" }));
  return render(
    <ClientContext.Provider value={client}>
      <MemoryRouter>
        <SidebarTabs tabs={tabs} agents={agents} sessions={sessions} />
      </MemoryRouter>
    </ClientContext.Provider>,
  );
}

afterEach(() => localStorage.clear());

describe("SidebarTabs", () => {
  it("links each tab to its person's thread or its conversation, with the browser sheet open", () => {
    setup([
      tab({ tab_id: "a", title: "Cart", owner_session_id: "home-chief" }),
      tab({ tab_id: "b", title: "Flights", url: "https://air.example/", owner_session_id: "home-mina" }),
      tab({ tab_id: "c", title: "Report", owner_session_id: "s9", opened_by: "external" }),
    ]);
    const section = screen.getByRole("region", { name: "Tabs" });
    const links = within(section).getAllByRole("link");
    expect(links.map((link) => link.getAttribute("href"))).toEqual(["/agents/chief?browser=open", "/agents/mina?browser=open", "/sessions/s9?browser=open"]);
    expect(within(links[1]!).getByText("Flights")).toBeInTheDocument();
    expect(within(links[1]!).getByText("air.example")).toBeInTheDocument();
  });

  it("titles each row with the full page title and describes it with the URL", () => {
    setup([tab({ tab_id: "a", title: "A very long page title that the rail truncates", url: "https://shop.example/item?id=7" })]);
    const link = screen.getByRole("link");
    expect(link).toHaveAttribute("title", "A very long page title that the rail truncates");
    expect(link).toHaveAccessibleDescription("https://shop.example/item?id=7");
  });

  it("files a tab from a teammate's other session under that teammate", () => {
    setup(
      [tab({ tab_id: "a", title: "Cart", owner_session_id: "home-chief" }), tab({ tab_id: "b", title: "Flights", owner_session_id: "s7" }), tab({ tab_id: "c", title: "Report", owner_session_id: "s9" })],
      [{ id: "s7", agent_id: "mina" } as Session, { id: "s9", agent_id: null } as Session],
    );
    expect(screen.getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(["openside, 1", "Mina, 1", "Other conversations, 1"]);
    // Grouped under Mina, but the tab still opens the session that owns it, not her home thread.
    expect(within(screen.getByRole("list", { name: "Mina" })).getByRole("link")).toHaveAttribute("href", "/sessions/s7?browser=open");
  });

  it("keeps a folded group folded across remounts and forgets groups that are gone", async () => {
    const user = userEvent.setup();
    const both = [tab({ tab_id: "a", owner_session_id: "home-chief" }), tab({ tab_id: "b", owner_session_id: "home-mina" })];
    const first = setup(both);
    await user.click(screen.getByRole("button", { name: "Mina, 1" }));
    first.unmount();
    const second = setup(both);
    expect(screen.getByRole("button", { name: "Mina, 1" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("list", { name: "Mina" })).toBeNull();
    second.unmount();
    // No tabs at all (a failed poll) prunes nothing.
    setup([]).unmount();
    expect(JSON.parse(localStorage.getItem(COLLAPSED_TAB_GROUPS_KEY) ?? "[]")).toEqual(["mina"]);
    // Mina's tabs closed: her fold is forgotten, so a later group of hers starts open.
    setup([tab({ tab_id: "a", owner_session_id: "home-chief" })]).unmount();
    expect(localStorage.getItem(COLLAPSED_TAB_GROUPS_KEY)).toBeNull();
    setup(both);
    expect(screen.getByRole("button", { name: "Mina, 1" })).toHaveAttribute("aria-expanded", "true");
  });

  it("groups tabs by teammate with a count, and folds a group away", async () => {
    const user = userEvent.setup();
    setup([
      tab({ tab_id: "a", title: "Cart", owner_session_id: "home-chief" }),
      tab({ tab_id: "b", title: "Flights", owner_session_id: "home-mina" }),
      tab({ tab_id: "c", title: "Hotels", owner_session_id: "home-mina" }),
      tab({ tab_id: "d", title: "Report", owner_session_id: "s9" }),
    ]);
    expect(screen.getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(["openside, 1", "Mina, 2", "Other conversations, 1"]);
    expect(within(screen.getByRole("list", { name: "Mina" })).getAllByRole("link").map((link) => link.textContent)).toEqual(["Flightsshop.example", "Hotelsshop.example"]);
    const mina = screen.getByRole("button", { name: "Mina, 2" });
    await user.click(mina);
    expect(mina).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("list", { name: "Mina" })).toBeNull();
    expect(screen.getByRole("list", { name: "openside" })).toBeInTheDocument();
  });

  it("calls a lone group Agent tabs", () => {
    setup([tab({ tab_id: "a", owner_session_id: "home-mina" }), tab({ tab_id: "b", owner_session_id: "home-mina" })]);
    expect(screen.getByRole("button", { name: "Agent tabs, 2" })).toHaveAttribute("aria-expanded", "true");
    expect(within(screen.getByRole("list", { name: "Agent tabs" })).getAllByRole("link")).toHaveLength(2);
  });

  it("renders nothing without tabs", () => {
    const { container } = setup([]);
    expect(container).toBeEmptyDOMElement();
  });
});
