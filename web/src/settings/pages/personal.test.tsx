import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../../context";
import { loadSidebarPrefs, markTabUsed, tabsByUse } from "../../shell";
import { chordProblem, loadBindings } from "../../shortcuts";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { chooseOption } from "../../test/select";
import { ToastProvider } from "../../ui";
import { AccountSettings } from "./Account";
import { AppearanceSettings } from "./Appearance";
import { GeneralSettings } from "./General";
import { NetworkAccess } from "./NetworkAccess";
import { SecuritySettings } from "./Security";

const PREFS = {
  accept_languages: ["en-US", "ko"],
  auto_picture_in_picture: true,
  default_zoom_percent: 100,
  search_engine: { current: 1, options: [{ id: 1, name: "Google" }, { id: 2, name: "DuckDuckGo" }] },
  spell_check: true,
  third_party_cookies: "allow",
};

const POLICY = { default_mode: "guard", allow_origins: [], deny_origins: ["https://ads.example"], rules: [] };

type Override = (call: Call) => Response | undefined;

function daemon(override?: Override) {
  return fakeClient((call) => {
    const mine = override?.(call);
    if (mine) return mine;
    const path = new URL(call.path, "http://daemon.test").pathname;
    if (path === "/v1/browser/prefs" && call.method === "GET") return json(200, { status: "available", prefs: PREFS });
    if (path === "/v1/browser/prefs") {
      const { search_engine, ...rest } = call.body as { search_engine?: number };
      const engines = search_engine == null ? PREFS.search_engine : { ...PREFS.search_engine, current: search_engine };
      return json(200, { status: "available", prefs: { ...PREFS, ...rest, search_engine: engines } });
    }
    if (path === "/v1/settings/preferences" && call.method === "GET") return json(200, { name: "Mina" });
    if (path === "/v1/settings/preferences") return json(200, call.body);
    if (path === "/v1/devices/me") return json(200, { device_id: "d1", name: "Laptop", kind: "person_device", scopes: ["admin"] });
    if (path === "/v1/devices" && call.method === "GET")
      return json(200, {
        devices: [
          { device_id: "d1", name: "Laptop", kind: "person_device", scopes: ["admin"], created_at: "2026-09-01T00:00:00Z" },
          { device_id: "d2", name: "My phone", kind: "person_device", scopes: ["read"], created_at: "2026-10-01T00:00:00Z" },
        ],
      });
    if (path.startsWith("/v1/devices/") && call.method === "PATCH") {
      const name = call.body && typeof call.body === "object" && "name" in call.body ? call.body.name : null;
      return json(200, { device_id: path.slice("/v1/devices/".length), name, kind: "person_device", scopes: ["read"], created_at: null });
    }
    if (path.startsWith("/v1/devices/")) return json(204, undefined);
    if (path === "/v1/connections" && call.method === "GET") return json(200, { connections: [], candidates: [], browser_available: true });
    if (path === "/v1/policy/network" && call.method === "GET") return json(200, { allow: ["huggingface.co"], restart_required: false });
    if (path === "/v1/about") return json(200, { version: "0.4.0", runtime_versions: {}, releases_url: "https://example.test/releases" });
    if (path === "/v1/about/update-check") return json(200, { current: "0.4.0", latest: "0.5.0", update_available: true, url: "https://example.test/0.5.0" });
    if (path === "/v1/health") return json(200, { ok: true, api_version: "1" });
    if (path === "/v1/sync") return json(200, { configured: false });
    if (path === "/v1/policy" && call.method === "GET") return json(200, POLICY);
    if (path === "/v1/policy/validate") return json(200, { valid: true, problems: [] });
    if (path === "/v1/policy") return json(200, call.body);
    return json(404, { code: "not_found", message: "nope" });
  });
}

function show(page: React.ReactNode, override?: Override) {
  const fake = daemon(override);
  render(
    <MemoryRouter>
      <ClientContext.Provider value={fake.client}>
        <ToastProvider>{page}</ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

const patches = (calls: Call[], path: string) => calls.filter((call) => call.method === "PATCH" && call.path === path).map((call) => call.body);

afterEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  Reflect.deleteProperty(navigator, "userAgentData");
});

function reportBrowser(fullVersionList: { brand: string; version: string }[]) {
  Object.defineProperty(navigator, "userAgentData", { configurable: true, value: { getHighEntropyValues: () => Promise.resolve({ fullVersionList }) } });
}

describe("Settings › General", () => {
  it("writes browser preferences to the person's browser profile", async () => {
    const user = userEvent.setup();
    const { calls } = show(<GeneralSettings onSignOut={vi.fn()} />);
    const spell = await screen.findByRole("switch", { name: "Spell check" });
    await waitFor(() => expect(spell).toBeEnabled());
    await user.click(spell);
    await user.click(screen.getByRole("switch", { name: "Auto Picture-in-Picture" }));
    await chooseOption(user, screen.getByRole("combobox", { name: "Default search engine" }), "DuckDuckGo");
    await waitFor(() =>
      expect(patches(calls, "/v1/browser/prefs")).toEqual([{ spell_check: false }, { auto_picture_in_picture: false }, { search_engine: 2 }]),
    );
    expect(screen.getByRole("switch", { name: "Screenshot" })).toBeDisabled();
  });

  it("opens new tabs in Search until Ask is chosen", async () => {
    const user = userEvent.setup();
    show(<GeneralSettings onSignOut={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "New tab mode" })).toHaveTextContent("Search");
    await chooseOption(user, screen.getByRole("combobox", { name: "New tab mode" }), "Ask");
    expect(localStorage.getItem("openside.newTabMode")).toBe("ask");
  });

  it("checks for updates only when asked and links the download", async () => {
    const user = userEvent.setup();
    const { calls } = show(<GeneralSettings onSignOut={vi.fn()} />);
    expect(await screen.findByText(/0\.4\.0/)).toBeInTheDocument();
    expect(calls.some((call) => call.path === "/v1/about/update-check")).toBe(false);
    await user.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByRole("link", { name: /Download/ })).toHaveAttribute("href", "https://example.test/0.5.0");
  });

  it("shows the browser's version and the background service's own version and state", async () => {
    reportBrowser([
      { brand: "Not.A/Brand", version: "99.0.0.0" },
      { brand: "Chromium", version: "155.0.7310.4" },
    ]);
    show(<GeneralSettings onSignOut={vi.fn()} />);
    expect(await screen.findByText("Current version is 155.0.7310.4.")).toBeInTheDocument();
    expect(screen.getByText("Browser")).toBeInTheDocument();
    expect(await screen.findByText("Running normally. Version v0.4.0.")).toBeInTheDocument();
  });

  it("shows a dash when the browser reports no version", async () => {
    show(<GeneralSettings onSignOut={vi.fn()} />);
    expect(await screen.findByText("Running normally. Version v0.4.0.")).toBeInTheDocument();
    expect(screen.getByText("—")).toBeInTheDocument();
  });

  it("signs out and reopens the setup guide", async () => {
    const user = userEvent.setup();
    const onSignOut = vi.fn();
    show(<GeneralSettings onSignOut={onSignOut} />);
    expect(screen.getByRole("link", { name: /Run setup again/ })).toHaveAttribute("href", "/welcome");
    await user.click(screen.getByRole("button", { name: "Sign out" }));
    expect(onSignOut).toHaveBeenCalledOnce();
  });

  it("says when the background service does not answer", async () => {
    show(<GeneralSettings onSignOut={vi.fn()} />, (call) => (call.path === "/v1/health" ? json(503, { code: "unavailable", message: "down" }) : undefined));
    expect(await screen.findByText(/^Not responding\./)).toBeInTheDocument();
  });
});

describe("Settings › Appearance", () => {
  it("applies the theme, tab style and sidebar choices at once", async () => {
    const user = userEvent.setup();
    show(<AppearanceSettings />);
    await chooseOption(user, screen.getByRole("combobox", { name: "Theme" }), "Dark");
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    await user.click(screen.getByRole("radio", { name: /Serif/ }));
    expect(document.documentElement).toHaveAttribute("data-tab-style", "serif");
    await chooseOption(user, screen.getByRole("combobox", { name: "New tab position" }), "At bottom");
    await chooseOption(user, screen.getByRole("combobox", { name: "Recent chats" }), /10/);
    expect(loadSidebarPrefs()).toMatchObject({ newTabPosition: "bottom", recentChats: 10 });
  });

  it("sets the zoom level in the browser profile", async () => {
    const user = userEvent.setup();
    const { calls } = show(<AppearanceSettings />);
    const zoom = await screen.findByRole("combobox", { name: "Zoom level" });
    await waitFor(() => expect(zoom).not.toHaveAttribute("data-disabled"));
    await chooseOption(user, zoom, "125%");
    await waitFor(() => expect(patches(calls, "/v1/browser/prefs")).toEqual([{ default_zoom_percent: 125 }]));
  });

  it("rebinds a shortcut and resets it", async () => {
    const user = userEvent.setup();
    show(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: /Options for Toggle sidebar/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Change shortcut" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("textbox"));
    await user.keyboard("{Control>}{Shift>}k{/Shift}{/Control}");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(loadBindings().sidebar).toEqual({ code: "KeyK", shift: true, alt: false });
    await user.click(screen.getByRole("button", { name: /Options for Toggle sidebar/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Reset to default" }));
    expect(loadBindings().sidebar).toBeUndefined();
  });

  it("refuses keys the browser keeps and ignores keys typed through an input method", async () => {
    const user = userEvent.setup();
    show(<AppearanceSettings />);
    await user.click(screen.getByRole("button", { name: /Options for Toggle sidebar/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Change shortcut" }));
    const dialog = await screen.findByRole("dialog");
    const capture = within(dialog).getByRole("textbox");
    fireEvent.keyDown(capture, { code: "KeyK", key: "k", ctrlKey: true, isComposing: true });
    fireEvent.keyDown(capture, { code: "KeyK", key: "Process", ctrlKey: true, keyCode: 229 });
    expect(within(dialog).getByText("Waiting for keys…")).toBeInTheDocument();
    fireEvent.keyDown(capture, { code: "KeyT", key: "T", ctrlKey: true, shiftKey: true });
    expect(within(dialog).getByRole("alert")).toHaveTextContent("The browser keeps these keys for itself.");
    expect(within(dialog).getByRole("button", { name: "Save" })).toBeDisabled();
    for (const code of ["KeyT", "KeyN", "KeyW", "KeyQ", "Tab"]) expect(chordProblem("sidebar", { code, shift: true, alt: false })).toEqual({ kind: "reserved" });
  });

  it("shows the openside browser's split keys as fixed, with no change action", async () => {
    const user = userEvent.setup();
    show(<AppearanceSettings />);
    expect(screen.getByText("Ctrl+Shift+\\", { selector: ".sr-only" })).toBeInTheDocument();
    expect(screen.getByText("Ctrl+Shift+-", { selector: ".sr-only" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Options for Split tabs (stacked)" }));
    expect(await screen.findByRole("menuitem", { name: "Change shortcut" })).toHaveAttribute("aria-disabled", "true");
    // Only Copy URL is still missing.
    expect(screen.getAllByText(/Not available in openside yet/)).toHaveLength(1);
  });

  it("orders the tab search by last use unless tab order is chosen", async () => {
    const user = userEvent.setup();
    show(<AppearanceSettings />);
    const tabs = [{ tab_id: 1 }, { tab_id: 2 }, { tab_id: 3 }];
    markTabUsed(3, 1_000);
    markTabUsed(1, 2_000);
    expect(tabsByUse(tabs).map((tab) => tab.tab_id)).toEqual([1, 3, 2]);
    expect(loadSidebarPrefs().tabSwitcherOrder).toBe("recent");
    await chooseOption(user, screen.getByRole("combobox", { name: "Tab switcher order" }), "Tab order");
    expect(loadSidebarPrefs().tabSwitcherOrder).toBe("tabs");
  });

  it("says transparency only changes openside's sidebar", () => {
    show(<AppearanceSettings />);
    expect(screen.getByText("Changes openside's sidebar only. The browser's own tab strip can't change.")).toBeInTheDocument();
  });
});

describe("Settings › Account", () => {
  it("saves the name on Enter", async () => {
    const user = userEvent.setup();
    const { calls } = show(<AccountSettings onSignOut={vi.fn()} />);
    const name = await screen.findByRole("textbox", { name: "Your name" });
    await waitFor(() => expect(name).toHaveValue("Mina"));
    await user.clear(name);
    await user.type(name, "Mina Park{Enter}");
    await waitFor(() => expect(patches(calls, "/v1/settings/preferences")).toEqual([{ name: "Mina Park" }]));
  });

  it("removes a device after confirmation", async () => {
    const user = userEvent.setup();
    const { calls } = show(<AccountSettings onSignOut={vi.fn()} />);
    await user.click(await screen.findByRole("button", { name: "Options for My phone" }));
    await user.click(await screen.findByRole("menuitem", { name: "Revoke" }));
    const dialog = await screen.findByRole("dialog", { name: "Revoke My phone?" });
    await user.click(within(dialog).getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/devices/d2")).toBe(true));
  });

  it("marks this device from the daemon's record, not this browser's storage", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.device", "d2");
    show(<AccountSettings onSignOut={vi.fn()} />);
    const laptop = (await screen.findByText("Laptop")).closest("li")!;
    await waitFor(() => expect(within(laptop).getByText("This device")).toBeInTheDocument());
    expect(within(screen.getByText("My phone").closest("li")!).queryByText("This device")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Options for Laptop" }));
    await user.click(await screen.findByRole("menuitem", { name: "Revoke" }));
    expect(await screen.findByRole("dialog", { name: "Revoke Laptop?" })).toHaveTextContent("This is the device you are using");
  });

  it("renames a device from its menu", async () => {
    const user = userEvent.setup();
    const { calls } = show(<AccountSettings onSignOut={vi.fn()} />);
    await user.click(await screen.findByRole("button", { name: "Options for My phone" }));
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog", { name: "Rename My phone" });
    const field = within(dialog).getByRole("textbox", { name: "Device name" });
    await user.clear(field);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("Use 1 to 100 characters.");
    await user.type(field, "  Work phone ");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patches(calls, "/v1/devices/d2")).toEqual([{ name: "Work phone" }]));
  });

  it("says sync is off before what turning it on does", async () => {
    show(<AccountSettings onSignOut={vi.fn()} />);
    expect(await screen.findByText("Off · Keep settings, pinned tabs and passwords the same on your devices")).toBeInTheDocument();
  });

  it("resets: disconnects this device, clears this browser in storage and in the page, then signs out", async () => {
    const user = userEvent.setup();
    const onSignOut = vi.fn();
    localStorage.setItem("openside.theme", "dark");
    localStorage.setItem("openside.look", JSON.stringify({ tabStyle: "serif", transparency: "tinted" }));
    localStorage.setItem("openside.shortcuts", JSON.stringify({ sidebar: { code: "KeyK", shift: true, alt: false } }));
    localStorage.setItem("unrelated", "kept");
    // Earlier cases left the shortcut cache warm; another tab's write is how storage reaches it.
    window.dispatchEvent(new StorageEvent("storage", { key: "openside.shortcuts" }));
    expect(loadBindings().sidebar).toBeDefined();
    const { calls } = show(<AccountSettings onSignOut={onSignOut} />);
    await user.click(screen.getByRole("button", { name: "Reset" }));
    const dialog = await screen.findByRole("dialog", { name: "Reset openside on this computer?" });
    await user.click(within(dialog).getByRole("button", { name: "Reset" }));
    await waitFor(() => expect(onSignOut).toHaveBeenCalledOnce());
    expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/devices/d1")).toBe(true);
    expect(Object.keys(localStorage).filter((key) => key.startsWith("openside."))).toEqual([]);
    expect(localStorage.getItem("unrelated")).toBe("kept");
    expect(loadBindings()).toEqual({});
    expect(document.documentElement).toHaveAttribute("data-theme", "system");
    expect(document.documentElement).toHaveAttribute("data-tab-style", "sans");
  });

  it("does not claim a disconnect when revoking fails, and offers clearing only this browser", async () => {
    const user = userEvent.setup();
    const onSignOut = vi.fn();
    localStorage.setItem("openside.theme", "dark");
    show(<AccountSettings onSignOut={onSignOut} />, (call) => (call.method === "DELETE" ? json(403, { code: "forbidden", message: "This device lacks admin." }) : undefined));
    await user.click(screen.getByRole("button", { name: "Reset" }));
    const dialog = await screen.findByRole("dialog", { name: "Reset openside on this computer?" });
    await user.click(within(dialog).getByRole("button", { name: "Reset" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Couldn't disconnect this device");
    expect(onSignOut).not.toHaveBeenCalled();
    expect(localStorage.getItem("openside.theme")).toBe("dark");
    await user.click(within(dialog).getByRole("button", { name: "Clear this browser only" }));
    expect(onSignOut).toHaveBeenCalledOnce();
    expect(localStorage.getItem("openside.theme")).toBeNull();
  });

  it("clears only this browser when the daemon knows no device for it", async () => {
    const user = userEvent.setup();
    const onSignOut = vi.fn();
    const { calls } = show(<AccountSettings onSignOut={onSignOut} />, (call) => (call.path === "/v1/devices/me" ? json(401, { code: "unauthorized", message: "no" }) : undefined));
    await user.click(screen.getByRole("button", { name: "Reset" }));
    const dialog = await screen.findByRole("dialog", { name: "Reset openside on this computer?" });
    expect(dialog).toHaveTextContent("a reset can only clear openside's settings in this browser");
    await user.click(within(dialog).getByRole("button", { name: "Clear this browser only" }));
    expect(onSignOut).toHaveBeenCalledOnce();
    expect(calls.some((call) => call.method === "DELETE")).toBe(false);
  });
});

describe("Settings › Security & Privacy", () => {
  it("changes third-party cookies in the browser profile", async () => {
    const user = userEvent.setup();
    const { calls } = show(<SecuritySettings />);
    const cookies = await screen.findByRole("combobox", { name: "Third-party cookies" });
    await waitFor(() => expect(cookies).not.toHaveAttribute("data-disabled"));
    await chooseOption(user, cookies, "Block");
    await waitFor(() => expect(patches(calls, "/v1/browser/prefs")).toEqual([{ third_party_cookies: "block" }]));
  });

  it("edits the sites agents are blocked from, checked before saving", async () => {
    const user = userEvent.setup();
    const { calls } = show(<SecuritySettings />);
    expect(await screen.findByText("1 site blocked for agents")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Configure/ }));
    const box = await screen.findByLabelText("Blocked sites");
    await user.type(box, "\nhttps://*.tracker.example");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(calls.find((call) => call.method === "PUT" && call.path === "/v1/policy")?.body).toEqual({
        ...POLICY,
        deny_origins: ["https://ads.example", "https://*.tracker.example"],
      }),
    );
    expect(calls.some((call) => call.path === "/v1/policy/validate")).toBe(true);
  });

  it("deletes the chosen kinds of browsing data over the chosen time range", async () => {
    const user = userEvent.setup();
    const { calls } = show(<SecuritySettings />, (call) => (call.path === "/v1/browser/person/clear-data" ? json(204, undefined) : undefined));
    await user.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete browsing data" });
    expect(within(dialog).getByRole("checkbox", { name: "Browsing history" })).toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: "Saved passwords" })).not.toBeChecked();
    await user.click(within(dialog).getByRole("checkbox", { name: "Cached images and files" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Saved passwords" }));
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Time range" }), "All time");
    await user.click(within(dialog).getByRole("button", { name: "Delete data" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Delete browsing data" })).toBeNull());
    expect(calls.filter((call) => call.path === "/v1/browser/person/clear-data").map((call) => call.body)).toEqual([{ types: ["history", "cookies", "passwords"], since_ms: 0 }]);
    expect(await screen.findByText("Browsing data deleted")).toBeInTheDocument();
  });

  it("clears the last hour by default and needs at least one kind", async () => {
    const user = userEvent.setup();
    const { calls } = show(<SecuritySettings />, (call) => (call.path === "/v1/browser/person/clear-data" ? json(204, undefined) : undefined));
    await user.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete browsing data" });
    for (const name of ["Browsing history", "Cookies and other site data", "Cached images and files"]) await user.click(within(dialog).getByRole("checkbox", { name }));
    expect(within(dialog).getByRole("button", { name: "Delete data" })).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox", { name: "Download history" }));
    const before = Date.now();
    await user.click(within(dialog).getByRole("button", { name: "Delete data" }));
    await waitFor(() => expect(calls.some((call) => call.path === "/v1/browser/person/clear-data")).toBe(true));
    const sent = calls.find((call) => call.path === "/v1/browser/person/clear-data")?.body;
    expect(sent).toMatchObject({ types: ["downloads"] });
    const since = sent && typeof sent === "object" && "since_ms" in sent ? Number(sent.since_ms) : NaN;
    expect(since).toBeGreaterThanOrEqual(before - 3_600_000);
    expect(since).toBeLessThanOrEqual(Date.now() - 3_600_000);
  });

  it("says nothing was deleted when the browser is not connected", async () => {
    const user = userEvent.setup();
    show(<SecuritySettings />, (call) =>
      call.path === "/v1/browser/person/clear-data" ? json(409, { code: "person_browser_not_connected", message: "No openside extension is connected." }) : undefined,
    );
    await user.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete browsing data" });
    await user.click(within(dialog).getByRole("button", { name: "Delete data" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Your browser isn't connected to openside, so nothing was deleted.");
  });

  it("shows signed-in sites without the activity log", async () => {
    const user = userEvent.setup();
    const { calls } = show(<SecuritySettings />);
    await user.click(screen.getByRole("button", { name: /Signed-in sites/ }));
    const dialog = await screen.findByRole("dialog", { name: "Signed-in sites" });
    await waitFor(() => expect(calls.some((call) => call.path === "/v1/connections")).toBe(true));
    expect(within(dialog).queryByRole("group", { name: "Activity filters" })).toBeNull();
    expect(calls.some((call) => call.path.startsWith("/v1/connections/audit"))).toBe(false);
  });
});

describe("Security & Privacy › Network access", () => {
  const SETTINGS_DOC = { config: {}, file_only: [], remote_editable: [], restart_required: false, schema: {} };

  it("tells a browser without local authority where to change the list", async () => {
    show(<NetworkAccess />, (call) => (call.path === "/v1/settings/config" ? json(200, { ...SETTINGS_DOC, remote: true }) : undefined));
    expect(await screen.findByText(/use openside on the computer that runs it/)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Allowed addresses" })).toHaveAttribute("readonly");
  });

  it("maps a refused save to the same note and points the field at its problems and hint", async () => {
    const user = userEvent.setup();
    let valid = false;
    show(<NetworkAccess />, (call) => {
      if (call.path === "/v1/settings/config") return json(200, { ...SETTINGS_DOC, remote: false });
      if (call.path === "/v1/policy/validate") return json(200, valid ? { valid: true, problems: [] } : { valid: false, problems: [{ list: "network_allow", index: 1, message: "not a host" }] });
      if (call.path === "/v1/policy/network" && call.method === "PUT") return json(403, { code: "local_access_required", message: "local" });
      return undefined;
    });
    const field = await screen.findByRole("textbox", { name: "Allowed addresses" });
    await user.type(field, "\nbad host");
    await user.click(screen.getByRole("button", { name: "Save list" }));
    await waitFor(() => expect(field).toHaveAttribute("aria-invalid", "true"));
    const [problems, hint] = (field.getAttribute("aria-describedby") ?? "").split(" ");
    expect(document.getElementById(problems!)).toHaveTextContent("not a host");
    expect(document.getElementById(hint!)).toHaveTextContent("host:port");
    valid = true;
    await user.click(screen.getByRole("button", { name: "Save list" }));
    expect(await screen.findByText(/use openside on the computer that runs it/)).toBeInTheDocument();
  });
});
