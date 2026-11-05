import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { modelsDaemon } from "../models/testDaemon";
import { ToastProvider } from "../ui";
import { ManageDialogs } from "./ManageDialogs";

function Shell() {
  const location = useLocation();
  return (
    <>
      <p data-testid="where">{location.pathname}</p>
      <ManageDialogs location={location} onSignOut={vi.fn()} />
    </>
  );
}

function renderAt(path: string) {
  render(
    <ClientContext.Provider value={modelsDaemon().client}>
      <ToastProvider>
        <MemoryRouter initialEntries={[path]}>
          <Shell />
        </MemoryRouter>
      </ToastProvider>
    </ClientContext.Provider>,
  );
}

describe("Models & accounts dialog", () => {
  it("deep-links to /models and returns to Settings with the back control", async () => {
    const user = userEvent.setup();
    renderAt("/models");
    const dialog = await screen.findByRole("dialog", { name: "Models & accounts" });
    expect(await within(dialog).findByRole("region", { name: "Provider Anthropic" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /^Models & accounts/ })).toHaveAttribute("aria-current", "page");
    await user.click(within(dialog).getByRole("button", { name: "Back to settings" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/settings");
  });
});

describe("Settings dialog", () => {
  it("swaps panes from the nav inside the same dialog", async () => {
    const user = userEvent.setup();
    renderAt("/settings");
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    const nav = within(dialog).getByRole("navigation", { name: "Settings sections" });
    expect(within(nav).getByRole("button", { name: /^General/ })).toHaveAttribute("aria-current", "page");
    await user.click(within(nav).getByRole("button", { name: /^Models & accounts/ }));
    expect(screen.getByTestId("where")).toHaveTextContent("/models");
    expect(dialog).toHaveAccessibleName("Models & accounts");
    expect(await within(dialog).findByRole("region", { name: "Provider Anthropic" })).toBeInTheDocument();
  });
});

describe("Workspace dialog", () => {
  it("lists every section on /workspace and goes back to Settings", async () => {
    const user = userEvent.setup();
    renderAt("/workspace");
    const dialog = await screen.findByRole("dialog", { name: "Workspace" });
    const nav = within(dialog).getByRole("navigation", { name: "Workspace sections" });
    expect(within(nav).getAllByRole("link")).toHaveLength(12);
    expect(within(nav).getByRole("link", { name: /^Vault/ })).toHaveAttribute("href", "/vault");
    await user.click(within(dialog).getByRole("button", { name: "Back to settings" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/settings");
  });
});
