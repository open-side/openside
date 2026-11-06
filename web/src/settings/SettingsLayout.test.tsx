import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { PHONE } from "../shortcuts";
import { fakeClient, json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { LEGACY_SETTINGS_PATHS, SETTINGS_PAGES, settingsBreadcrumb } from "./nav";
import { SettingsLayout } from "./SettingsLayout";

function Where() {
  return <output aria-label="path">{useLocation().pathname}</output>;
}

function setup(path: string, { phone = false } = {}) {
  vi.stubGlobal("matchMedia", (query: string) => ({ matches: phone && query === PHONE, addEventListener() {}, removeEventListener() {} }) as unknown as MediaQueryList);
  const fake = fakeClient(() => json(404, { code: "not_found", message: "nope" }));
  render(
    <MemoryRouter initialEntries={[path]}>
      <ClientContext.Provider value={fake.client}>
        <ToastProvider>
          <Routes>
            <Route path="settings/*" element={<SettingsLayout onSignOut={() => undefined} />} />
          </Routes>
          <Where />
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return { ...fake, path: () => screen.getByRole("status", { name: "path" }).textContent };
}

const nav = () => screen.getByRole("navigation", { name: "Settings sections" });
const labels = (scope: HTMLElement) => [...scope.querySelectorAll(".settings-nav-row")].map((row) => row.querySelector(".settings-nav-label")?.textContent);

afterEach(() => vi.unstubAllGlobals());

describe("Settings nav", () => {
  it("lists Aside's tree: four groups, Memory folded with Context Awareness beside it, then the footer", () => {
    setup("/settings/projects");
    const groups = within(nav()).getAllByRole("group");
    expect(groups.map((group) => group.getAttribute("aria-label"))).toEqual(["Personal", "Agent", "Features", "Archived"]);
    expect(groups.map(labels)).toEqual([
      ["General", "Appearance", "Account", "Security & Privacy"],
      ["Agents", "Projects", "Models", "Plugins & MCPs", "Memory", "Context Awareness"],
      ["Passwords", "Routines", "Channels", "Developers", "Mini popup", "Lasso"],
      ["Archived chats"],
    ]);
    expect(labels(nav()).slice(-4)).toEqual(["Send feedback", "Extensions", "Docs", "Community"]);
    expect(within(groups[1]!).getAllByText("New")).toHaveLength(1);
    expect(within(groups[2]!).getAllByText("New")).toHaveLength(1);
    expect(screen.getByRole("link", { name: "Projects" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { level: 1, name: "Projects" })).toBeInTheDocument();
  });

  it("opens Memory on its pages, marks the open one and drops the duplicate Context Awareness row", () => {
    setup("/settings/memory/history");
    const agent = within(nav()).getByRole("group", { name: "Agent" });
    expect(labels(agent)).toEqual(["Agents", "Projects", "Models", "Plugins & MCPs", "Memory", "Overview", "History", "Context Awareness", "Configure"]);
    expect(within(agent).getByRole("link", { name: "History" })).toHaveAttribute("aria-current", "page");
    expect(within(agent).getByRole("link", { name: "Memory" })).not.toHaveAttribute("aria-current");
    expect(within(agent).getByRole("link", { name: "Configure" })).toHaveAttribute("href", "/settings/memory/configure");
  });

  it("filters rows by title and keywords and says when nothing matches", async () => {
    const user = userEvent.setup();
    setup("/settings/projects");
    const search = within(nav()).getByRole("searchbox", { name: "Search settings" });
    expect(search).toHaveAttribute("placeholder", "Search..");
    await user.type(search, "qr code");
    expect(labels(nav())).toEqual(["Developers"]);
    await user.clear(search);
    await user.type(search, "embeddings");
    expect(labels(nav())).toEqual(["Memory", "Configure"]);
    await user.clear(search);
    await user.type(search, "nothing like this");
    expect(labels(nav())).toEqual([]);
    expect(within(nav()).getByText("No sections match “nothing like this”.")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(search).toHaveValue("");
  });

  it("moves through rows with the arrow keys and opens the first match with Enter", async () => {
    const user = userEvent.setup();
    const { path } = setup("/settings/projects");
    const search = within(nav()).getByRole("searchbox", { name: "Search settings" });
    await user.click(search);
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("link", { name: "General" })).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(screen.getByRole("link", { name: "Account" })).toHaveFocus();
    await user.keyboard("{ArrowUp}{ArrowUp}{ArrowUp}");
    expect(search).toHaveFocus();
    await user.type(search, "passkeys{Enter}");
    expect(path()).toBe("/settings/passwords");
  });

  it("links the footer out, and says so when the browser's own page cannot be opened", async () => {
    const user = userEvent.setup();
    const { calls } = setup("/settings/projects");
    expect(screen.getByRole("link", { name: "Docs" })).toHaveAttribute("target", "_blank");
    expect(screen.getByRole("link", { name: "Send feedback" }).getAttribute("href")).toMatch(/^https:\/\/github\.com\/.+\/issues\/new$/);
    await user.click(screen.getByRole("button", { name: "Extensions" }));
    expect(calls.some((call) => call.path === "/v1/browser/person/tabs/command")).toBe(true);
    expect(await screen.findByText(/chrome:\/\/extensions/)).toBeInTheDocument();
  });

  it("moves focus to the new page's title when a row is picked, but leaves the first render and a search alone", async () => {
    const user = userEvent.setup();
    const { path } = setup("/settings/projects");
    expect(document.body).toHaveFocus();

    await user.click(within(nav()).getByRole("link", { name: "Lasso" }));
    expect(screen.getByRole("heading", { level: 1, name: "Lasso" })).toHaveFocus();

    const search = within(nav()).getByRole("searchbox", { name: "Search settings" });
    await user.type(search, "mini{Enter}");
    expect(path()).toBe("/settings/mini-popup");
    expect(search).toHaveFocus();

    await user.clear(search);
    await user.keyboard("{ArrowDown}");
    expect(within(nav()).getByRole("link", { name: "General" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(path()).toBe("/settings/general");
    expect(screen.getByRole("heading", { level: 1, name: "General" })).toHaveFocus();
  });
});

describe("Settings routes", () => {
  it("opens General for /settings and for an unknown page on desktop", () => {
    const { path } = setup("/settings/nope");
    expect(path()).toBe("/settings/general");
  });

  it("is the page list at /settings on a phone, and each page leads back to it", async () => {
    const user = userEvent.setup();
    const { path } = setup("/settings", { phone: true });
    expect(path()).toBe("/settings");
    await user.click(within(nav()).getByRole("link", { name: "Lasso" }));
    expect(screen.queryByRole("navigation", { name: "Settings sections" })).toBeNull();
    expect(screen.getByRole("heading", { level: 1, name: "Lasso" })).toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "Back to settings" }));
    expect(path()).toBe("/settings");
  });

  it("names every page in the breadcrumb and sends every old path to a real page", () => {
    expect(settingsBreadcrumb("/settings/security")).toEqual(["Settings", "Security & Privacy"]);
    expect(settingsBreadcrumb("/settings/memory/history")).toEqual(["Settings", "Memory"]);
    expect(settingsBreadcrumb("/settings/memory/configure")).toEqual(["Settings", "Memory", "Configure"]);
    expect(settingsBreadcrumb("/sessions/s1")).toBeNull();
    for (const [, page] of LEGACY_SETTINGS_PATHS) expect(SETTINGS_PAGES).toContain(page);
  });
});
