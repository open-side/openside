import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { ClientContext } from "../context";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { agentSummary } from "../test/agents";
import { fakeClient, json } from "../test/fakeDaemon";
import { SidebarTabs } from "./SidebarTabs";

const agents = [agentSummary("chief", "openside", { home_session_id: "home-chief" }), agentSummary("mina", "Mina", { home_session_id: "home-mina" })];

const tab = (over: Partial<OwnedTab>): OwnedTab => ({ tab_id: "t", title: "Page", url: "https://shop.example/item", opened_by: "agent", owner_session_id: "home-chief", ...over });

function setup(tabs: OwnedTab[]) {
  // Favicons are missing, so rows fall back to the globe icon.
  const { client } = fakeClient(() => json(404, { code: "not_found", message: "none" }));
  return render(
    <ClientContext.Provider value={client}>
      <MemoryRouter>
        <SidebarTabs tabs={tabs} agents={agents} />
      </MemoryRouter>
    </ClientContext.Provider>,
  );
}

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

  it("renders nothing without tabs", () => {
    const { container } = setup([]);
    expect(container).toBeEmptyDOMElement();
  });
});
