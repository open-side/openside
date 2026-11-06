import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { describe, expect, it } from "vitest";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { ToastProvider } from "../../ui";
import { AppearanceSettings } from "./Appearance";
import { MiniPopupSettings } from "./MiniPopup";

const PREFS = {
  accept_languages: ["en-US"],
  auto_picture_in_picture: true,
  default_zoom_percent: 100,
  search_engine: { current: 1, options: [{ id: 1, name: "Google" }] },
  spell_check: true,
  third_party_cookies: "allow",
};

function show(state: { status: string; prefs: object | null }, path = "/settings/mini-popup") {
  let current = state;
  const fake = fakeClient((call: Call) => {
    if (call.path === "/v1/browser/prefs" && call.method === "GET") return json(200, current);
    if (call.path === "/v1/browser/prefs" && call.method === "PATCH") {
      current = { status: "available", prefs: { ...current.prefs, ...(call.body as object) } };
      return json(200, current);
    }
    if (call.path === "/v1/browser/person/tabs/command") return json(200, { tab_id: 9 });
    return json(404, { code: "not_found", message: "nope" });
  });
  render(
    <MemoryRouter initialEntries={[path]}>
      <ClientContext.Provider value={fake.client}>
        <ToastProvider>
          <Routes>
            <Route path="/settings/mini-popup" element={<MiniPopupSettings />} />
            <Route path="/settings/appearance" element={<AppearanceSettings />} />
          </Routes>
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

describe("Settings › Mini popup", () => {
  it("binds the shortcut in the person's browser and reads it back", async () => {
    const user = userEvent.setup();
    const { calls } = show({ status: "available", prefs: { ...PREFS, mini_popup: false } });
    expect(screen.getByRole("heading", { name: "Chat with openside anytime" })).toBeInTheDocument();
    expect(screen.getByText("Open openside with Alt+Space and start chatting from anywhere.")).toBeInTheDocument();
    const toggle = screen.getByRole("switch", { name: "Enable Mini popup" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).not.toBeChecked();
    expect(toggle).toHaveAccessibleDescription("Open Mini popup from anywhere with Alt+Space");
    await user.click(toggle);
    await waitFor(() => expect(calls.filter((call) => call.method === "PATCH").map((call) => call.body)).toEqual([{ mini_popup: true }]));
    expect(toggle).toBeChecked();
    expect(screen.queryByRole("button", { name: /Set a shortcut/ })).toBeNull();
  });

  it("stays off while the openside extension is not enabled", async () => {
    show({ status: "available", prefs: { ...PREFS, mini_popup: null } });
    const toggle = screen.getByRole("switch", { name: "Enable Mini popup" });
    await waitFor(() => expect(toggle).toHaveAccessibleDescription("Turn on the openside extension in this browser to use Mini popup."));
    expect(toggle).toBeDisabled();
  });

  it("says to open the browser when it is closed", async () => {
    show({ status: "not_running", prefs: null });
    const toggle = screen.getByRole("switch", { name: "Enable Mini popup" });
    await waitFor(() => expect(toggle).toHaveAccessibleDescription("Open openside's browser to change this."));
    expect(toggle).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Set a shortcut/ })).toBeNull();
  });

  it("sends a stock browser to its extension shortcuts page", async () => {
    const user = userEvent.setup();
    const { calls } = show({ status: "unsupported", prefs: null });
    await user.click(await screen.findByRole("button", { name: /Set a shortcut/ }));
    expect(screen.getByRole("switch", { name: "Enable Mini popup" })).toBeDisabled();
    await waitFor(() =>
      expect(calls.filter((call) => call.path === "/v1/browser/person/tabs/command").map((call) => call.body)).toEqual([{ command: "create", url: "chrome://extensions/shortcuts" }]),
    );
  });

  it("shows the bound key in Appearance and links back to this page", async () => {
    const user = userEvent.setup();
    show({ status: "available", prefs: { ...PREFS, mini_popup: true } }, "/settings/appearance");
    expect(await screen.findByText("Alt+Space", { selector: ".sr-only" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Options for Mini popup" }));
    await user.click(await screen.findByRole("menuitem", { name: "Change shortcut" }));
    expect(await screen.findByRole("switch", { name: "Enable Mini popup" })).toBeChecked();
  });
});
