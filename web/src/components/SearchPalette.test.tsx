import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createClient } from "../api/client";
import type { PersonTab } from "../api/types";
import { ClientContext } from "../context";
import { AgentTabsSettledContext, type OwnedTab } from "../hooks/useAgentTabs";
import { PersonBrowserContext, type PersonBrowser } from "../hooks/usePersonBrowser";
import { markTabUsed, saveSidebarPrefs } from "../shell";
import { sessionFixture } from "../test/composerDaemon";
import { fakeClient, json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { relativeTime, SearchPalette } from "./SearchPalette";

const minutesAgo = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

const SESSIONS = [sessionFixture({ id: "s1", title: "Trip to Busan", last_activity_at: minutesAgo(7) }), sessionFixture({ id: "s2", title: "Quarterly report", last_activity_at: minutesAgo(90) })];

const AGENT_TABS: OwnedTab[] = [{ tab_id: "t1", title: "Flight search", url: "https://flights.example/q", opened_by: "agent", owner_session_id: "s-mina" }];

const personTab = (over: Partial<PersonTab> & Pick<PersonTab, "tab_id" | "index">): PersonTab => ({
  window_id: 1,
  title: `Tab ${over.tab_id}`,
  url: `https://site${over.tab_id}.example/`,
  favicon_url: null,
  active: false,
  pinned: false,
  audible: false,
  muted: false,
  discarded: false,
  group_id: null,
  ...over,
});

// The third tab shows openside itself, so it is named by its breadcrumb.
const PERSON_TABS = [personTab({ tab_id: 11, index: 0, title: "Mail" }), personTab({ tab_id: 12, index: 1, title: "News", active: true }), personTab({ tab_id: 13, index: 2, url: `${window.location.origin}/settings/models`, title: "openside" })];

function browser(over: Partial<PersonBrowser> = {}): PersonBrowser {
  return { connected: true, focusedWindow: 1, tabs: PERSON_TABS, groups: [], bookmarks: [], bookmarkBar: "1", command: vi.fn(() => Promise.resolve()), refreshBookmarks: vi.fn(), ...over };
}

function Where() {
  return <output aria-label="path">{useLocation().pathname}</output>;
}

function setup({ tabs = AGENT_TABS, person = browser(), open = true, respond }: { tabs?: OwnedTab[]; person?: PersonBrowser; open?: boolean; respond?: (pathname: string) => Response | undefined } = {}) {
  const { client, calls } = fakeClient((call) => {
    const url = new URL(call.path, "http://daemon.test");
    const q = (url.searchParams.get("q") ?? "").toLowerCase();
    const answer = respond?.(url.pathname);
    if (answer) return answer;
    switch (url.pathname) {
      case "/v1/sessions":
        return json(200, { sessions: SESSIONS.filter((entry) => entry.title.toLowerCase().includes(q)) });
      case "/v1/browser/person/recently-closed":
        return json(200, { entries: [{ kind: "tab", session_id: "c1", tab_count: 1, title: "Sign in to GitHub", url: "https://github.example/login", closed_at: minutesAgo(3) }] });
      case "/v1/browser/person/recently-closed/c1/restore":
        return json(200, { ok: true });
      case "/v1/browser/person/history":
        return json(200, { items: [{ title: "Busan weather", url: "https://weather.example/busan", visit_count: 3, last_visit_at: minutesAgo(30) }].filter((item) => item.title.toLowerCase().includes(q)) });
      case "/v1/browser/person/bookmarks":
        return json(200, { roots: [{ id: "f", title: "Bar", children: [{ id: "b1", title: "Busan guide", url: "https://guide.example/busan", children: [] }] }] });
      case "/v1/browser/person/extensions":
        return json(200, { extensions: [{ id: "x1", name: "Busan dictionary", kind: "extension", enabled: false, description: "", version: "1" }] });
      default:
        // No cached favicons: rows fall back to the globe.
        return json(404, { code: "not_found", message: "none" });
    }
  });
  let closed = 0;
  render(
    <ClientContext.Provider value={client}>
      <PersonBrowserContext.Provider value={person}>
        <AgentTabsSettledContext.Provider value>
          <ToastProvider>
            <MemoryRouter initialEntries={["/"]}>
              <Where />
              <SearchPalette open={open} onClose={() => (closed += 1)} tabs={tabs} pageLabel={(path) => (path === "/settings/models" ? "Settings › Models" : null)} />
            </MemoryRouter>
          </ToastProvider>
        </AgentTabsSettledContext.Provider>
      </PersonBrowserContext.Provider>
    </ClientContext.Provider>,
  );
  // The modal palette hides the page from the accessibility tree, the path readout included.
  return { calls, person, closed: () => closed, path: () => screen.getByRole("status", { name: "path", hidden: true }).textContent };
}

const options = () => screen.getAllByRole("option");
// The breadcrumb separator is a drawn glyph without spaces around it.
const titles = () => options().map((option) => option.querySelector(".search-palette-title")?.textContent?.replace("›", " › "));
function option(index: number): HTMLElement {
  const found = options()[index];
  if (!found) throw new Error(`no result at ${index}`);
  return found;
}
const groups = () => screen.getAllByRole("group").map((group) => [group.querySelector(".search-palette-heading")?.textContent, within(group).getAllByRole("option").length]);

/** The field has focus and the daemon's chats and recently closed tabs have arrived. */
async function settled() {
  await waitFor(() => expect(screen.getByRole("combobox")).toHaveFocus());
  await screen.findByText("Trip to Busan");
  await screen.findByText("Sign in to GitHub");
}

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("SearchPalette", () => {
  it("opens on Open Tabs (the current tab first), Recent Chat with times and Recently Closed; the first row is highlighted", async () => {
    const { calls } = setup();
    const field = screen.getByRole("combobox", { name: "Search tabs, chats, history, bookmarks or extensions" });
    await settled();
    expect(groups()).toEqual([
      ["Open Tabs", 4],
      ["Recent Chat", 2],
      ["Recently Closed", 1],
    ]);
    expect(titles().slice(0, 4)).toEqual(["News", "Flight search", "Mail", "Settings › Models"]);
    expect(option(0)).toHaveAttribute("aria-selected", "true");
    expect(field).toHaveAttribute("aria-activedescendant", option(0).id);
    expect(within(option(0)).getByText("Switch to Tab")).toBeInTheDocument();
    expect(within(option(4)).getByText("7 minutes ago")).toBeInTheDocument();
    expect(within(option(6)).getByText("Reopen")).toBeInTheDocument();
    expect(screen.getByText("Ctrl+Shift+A")).toBeInTheDocument();
    // History, bookmarks and extensions are only asked for once something is typed.
    expect(calls.map((call) => call.path).filter((path) => /history|bookmarks|extensions/.test(path))).toEqual([]);
  });

  it("orders tabs by the tab switcher preference: recently used, or the browser's tab order", async () => {
    markTabUsed(11, 1);
    markTabUsed("t1", 2);
    setup();
    await settled();
    expect(titles().slice(0, 4)).toEqual(["News", "Flight search", "Mail", "Settings › Models"]);
    markTabUsed(13, 3);
    saveSidebarPrefs({ tabSwitcherOrder: "tabs" });
    await waitFor(() => expect(titles().slice(0, 4)).toEqual(["Flight search", "Mail", "News", "Settings › Models"]));
    saveSidebarPrefs({ tabSwitcherOrder: "recent" });
    await waitFor(() => expect(titles().slice(0, 4)).toEqual(["News", "Settings › Models", "Flight search", "Mail"]));
  });

  it("with a query, filters tabs and adds History, Bookmarks and Extensions; says when nothing matches", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await settled();
    await user.type(screen.getByRole("combobox"), "busan");
    await waitFor(() =>
      expect(groups()).toEqual([
        ["Recent Chat", 1],
        ["History", 1],
        ["Bookmarks", 1],
        ["Extensions", 1],
      ]),
    );
    expect(calls.some((call) => call.path === "/v1/sessions?q=busan&limit=6")).toBe(true);
    expect(calls.some((call) => call.path.startsWith("/v1/browser/person/history?") && call.path.includes("q=busan"))).toBe(true);
    expect(within(option(3)).getByText("Turned off")).toBeInTheDocument();
    expect(option(0)).toHaveAttribute("aria-selected", "true");
    await user.clear(screen.getByRole("combobox"));
    await user.type(screen.getByRole("combobox"), "zzz");
    expect(await screen.findByText("Nothing matches “zzz”.")).toBeInTheDocument();
  });

  it("moves with the arrow keys, wraps, and switches to the chosen person tab with Enter", async () => {
    const user = userEvent.setup();
    const { person, closed } = setup();
    await settled();
    await user.keyboard("{ArrowUp}");
    expect(option(6)).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(option(2)).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Enter}");
    expect(person.command).toHaveBeenCalledWith({ command: "activate", tab_id: 11 });
    expect(closed()).toBe(1);
  });

  it("opens an agent tab's chat with the browser sheet, a chat by its path, and reopens a closed tab", async () => {
    const user = userEvent.setup();
    const { path, calls } = setup();
    await settled();
    await user.click(screen.getByText("Flight search"));
    expect(path()).toBe("/sessions/s-mina");
    await user.click(screen.getByText("Quarterly report"));
    expect(path()).toBe("/sessions/s2");
    await user.click(screen.getByText("Sign in to GitHub"));
    await waitFor(() => expect(calls.some((call) => call.method === "POST" && call.path === "/v1/browser/person/recently-closed/c1/restore")).toBe(true));
  });

  it("closes on Escape", async () => {
    const user = userEvent.setup();
    const { closed } = setup();
    await settled();
    await user.keyboard("{Escape}");
    expect(closed()).toBe(1);
  });

  it("omits a section with nothing in it", async () => {
    setup({ tabs: [], person: browser({ connected: false, tabs: [] }) });
    await settled();
    expect(screen.queryByText("Open Tabs")).toBeNull();
    expect(option(0)).toHaveTextContent("Trip to Busan");
  });

  it("renders nothing while closed", () => {
    const { calls } = setup({ open: false });
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(calls).toHaveLength(0);
  });

  it("aborts a chat search the person typed past, and the one still out when the palette closes", async () => {
    const user = userEvent.setup();
    const requests: { q: string; signal: AbortSignal }[] = [];
    const client = createClient({
      token: () => "test-token",
      fetch: (input, init) => {
        const url = new URL(String(input), "http://daemon.test");
        if (url.pathname !== "/v1/sessions" || !init?.signal) return Promise.resolve(json(404, { code: "not_found", message: "none" }));
        const signal = init.signal;
        requests.push({ q: url.searchParams.get("q") ?? "", signal });
        // The daemon never answers: only an abort settles the request.
        const { promise, reject } = Promise.withResolvers<Response>();
        signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
        return promise;
      },
    });
    const palette = (open: boolean) => (
      <ClientContext.Provider value={client}>
        <ToastProvider>
          <MemoryRouter>
            <SearchPalette open={open} onClose={() => {}} tabs={[]} pageLabel={() => null} />
          </MemoryRouter>
        </ToastProvider>
      </ClientContext.Provider>
    );
    const view = render(palette(true));
    await waitFor(() => expect(requests.map((request) => request.q)).toEqual([""]));
    expect(screen.getByText("Searching…")).toBeInTheDocument();
    await user.type(screen.getByRole("combobox"), "bu");
    await waitFor(() => expect(requests.map((request) => request.q)).toEqual(["", "bu"]));
    expect(requests[0]?.signal.aborted).toBe(true);
    expect(requests[1]?.signal.aborted).toBe(false);
    view.rerender(palette(false));
    expect(requests[1]?.signal.aborted).toBe(true);
    expect(requests).toHaveLength(2);
  });

  it("aborts history, bookmark, extension and recently-closed reads the person typed past or closed", async () => {
    const user = userEvent.setup();
    const requests: { path: string; signal: AbortSignal | null | undefined }[] = [];
    const client = createClient({
      token: () => "test-token",
      fetch: (input, init) => {
        const url = new URL(String(input), "http://daemon.test");
        if (url.pathname === "/v1/sessions") return Promise.resolve(json(200, { sessions: [] }));
        if (!url.pathname.startsWith("/v1/browser/person/")) return Promise.resolve(json(404, { code: "not_found", message: "none" }));
        requests.push({ path: `${url.pathname}${url.search}`, signal: init?.signal });
        const { promise, reject } = Promise.withResolvers<Response>();
        init?.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
        return promise;
      },
    });
    const palette = (open: boolean) => (
      <ClientContext.Provider value={client}>
        <ToastProvider>
          <MemoryRouter>
            <SearchPalette open={open} onClose={() => {}} tabs={[]} pageLabel={() => null} />
          </MemoryRouter>
        </ToastProvider>
      </ClientContext.Provider>
    );
    const find = (prefix: string) => requests.filter((request) => request.path.startsWith(prefix));
    const view = render(palette(true));
    await waitFor(() => expect(find("/v1/browser/person/recently-closed")).toHaveLength(1));
    await user.type(screen.getByRole("combobox"), "bu");
    await waitFor(() => expect(find("/v1/browser/person/history")).toHaveLength(1));
    await waitFor(() => expect(find("/v1/browser/person/bookmarks")).toHaveLength(1));
    await waitFor(() => expect(find("/v1/browser/person/extensions")).toHaveLength(1));
    await user.type(screen.getByRole("combobox"), "s");
    await waitFor(() => expect(find("/v1/browser/person/history")).toHaveLength(2));
    const [stale, fresh] = find("/v1/browser/person/history");
    expect(stale?.path).toContain("q=bu&");
    expect(stale?.signal?.aborted).toBe(true);
    expect(fresh?.signal?.aborted).toBe(false);
    // Bookmarks and extensions are read once per open query and filtered here, not re-read per key.
    expect(find("/v1/browser/person/bookmarks")).toHaveLength(1);
    await user.clear(screen.getByRole("combobox"));
    await waitFor(() => expect(fresh?.signal?.aborted).toBe(true));
    expect(find("/v1/browser/person/bookmarks")[0]?.signal?.aborted).toBe(true);
    expect(find("/v1/browser/person/extensions")[0]?.signal?.aborted).toBe(true);
    expect(find("/v1/browser/person/recently-closed")[0]?.signal?.aborted).toBe(false);
    view.rerender(palette(false));
    expect(find("/v1/browser/person/recently-closed").every((request) => request.signal?.aborted)).toBe(true);
  });

  it("offers only web pages from history and bookmarks, and shows hostile titles as text", async () => {
    const user = userEvent.setup();
    const hostile = "<img src=x onerror=alert(1)>Busan";
    setup({
      respond: (pathname) => {
        if (pathname === "/v1/browser/person/history")
          return json(200, {
            items: [
              { title: "Busan run", url: "javascript:alert(1)", visit_count: 1, last_visit_at: minutesAgo(1) },
              { title: "Busan file", url: "file:///etc/passwd", visit_count: 1, last_visit_at: minutesAgo(2) },
              { title: hostile, url: "https://evil.example/busan", visit_count: 1, last_visit_at: minutesAgo(3) },
            ],
          });
        if (pathname === "/v1/browser/person/bookmarks")
          return json(200, {
            roots: [
              {
                id: "f",
                title: "Bar",
                children: [
                  { id: "b1", title: "Busan data", url: "data:text/html,<script>alert(1)</script>", children: [] },
                  { id: "b2", title: "Busan creds", url: "https://user:pass@guide.example/busan", children: [] },
                ],
              },
            ],
          });
        return undefined;
      },
    });
    await settled();
    await user.type(screen.getByRole("combobox"), "busan");
    await waitFor(() => expect(groups()).toContainEqual(["History", 1]));
    expect(groups().some(([heading]) => heading === "Bookmarks")).toBe(false);
    expect(screen.getByText(hostile)).toBeInTheDocument();
    expect(document.querySelector(".search-palette img[src='x']")).toBeNull();
    expect(screen.queryByText("Busan run")).toBeNull();
    expect(screen.queryByText("Busan file")).toBeNull();
  });
});

describe("relativeTime", () => {
  const now = new Date(2026, 9, 12, 12, 0);
  it("reads like Aside's chat list and refuses missing times", () => {
    expect(relativeTime(new Date(2026, 9, 12, 11, 59, 40).toISOString(), now)).toBe("Just now");
    expect(relativeTime(new Date(2026, 9, 12, 11, 59).toISOString(), now)).toBe("1 minute ago");
    expect(relativeTime(new Date(2026, 9, 12, 11, 15).toISOString(), now)).toBe("45 minutes ago");
    expect(relativeTime(new Date(2026, 9, 12, 11, 0).toISOString(), now)).toBe("1 hour ago");
    expect(relativeTime(new Date(2026, 9, 12, 2, 0).toISOString(), now)).toBe("10 hours ago");
    expect(relativeTime(new Date(2026, 9, 11, 2, 0).toISOString(), now)).toBe("Yesterday");
    expect(relativeTime(new Date(2026, 9, 8, 2, 0).toISOString(), now)).toBe("4 days ago");
    expect(relativeTime("", now)).toBeNull();
    expect(relativeTime(null, now)).toBeNull();
    expect(relativeTime("not a date", now)).toBeNull();
  });

  it("reads a time ahead of this clock as just now", () => {
    expect(relativeTime(new Date(2026, 9, 12, 12, 5).toISOString(), now)).toBe("Just now");
    expect(relativeTime(new Date(2026, 9, 14, 9, 0).toISOString(), now)).toBe("Just now");
  });

  it("counts calendar days across a daylight-saving change", () => {
    const zone = process.env.TZ;
    process.env.TZ = "America/New_York";
    try {
      // Clocks went forward at 2:00 on Mar 8 2026, so Mar 8 was 23 hours long.
      const morning = new Date(2026, 2, 9, 0, 30);
      expect(relativeTime(new Date(2026, 2, 8, 0, 15).toISOString(), morning)).toBe("23 hours ago");
      expect(relativeTime(new Date(2026, 2, 7, 23, 45).toISOString(), morning)).toBe("23 hours ago");
      // 24 h 15 min of wall time, yet two calendar days back: a fixed 24-hour day would say Yesterday.
      expect(relativeTime(new Date(2026, 2, 7, 23, 15).toISOString(), morning)).toBe("2 days ago");
      const later = new Date(2026, 2, 10, 0, 30);
      expect(relativeTime(new Date(2026, 2, 9, 0, 15).toISOString(), later)).toBe("Yesterday");
      expect(relativeTime(new Date(2026, 2, 8, 0, 45).toISOString(), later)).toBe("2 days ago");
      // Nov 1 2026 was 25 hours long when clocks went back.
      const fall = new Date(2026, 10, 2, 23, 30);
      expect(relativeTime(new Date(2026, 10, 1, 0, 0).toISOString(), fall)).toBe("Yesterday");
      expect(relativeTime(new Date(2026, 9, 31, 23, 59).toISOString(), fall)).toBe("2 days ago");
    } finally {
      if (zone === undefined) delete process.env.TZ;
      else process.env.TZ = zone;
    }
  });
});
