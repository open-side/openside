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

describe("Models & accounts dialog", () => {
  it("deep-links to /models and returns to Settings with the back control", async () => {
    const user = userEvent.setup();
    render(
      <ClientContext.Provider value={modelsDaemon().client}>
        <ToastProvider>
          <MemoryRouter initialEntries={["/models"]}>
            <Shell />
          </MemoryRouter>
        </ToastProvider>
      </ClientContext.Provider>,
    );
    const dialog = await screen.findByRole("dialog", { name: "Models & accounts" });
    expect(await within(dialog).findByRole("region", { name: "Provider Anthropic" })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Back to settings" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/settings");
  });
});
