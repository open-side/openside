import "fake-indexeddb/auto";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { PointerEventsCheckLevel, type UserEvent } from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary, McpServer, PersonTab, Project, SkillSummary } from "../api/types";
import { ClientContext } from "../context";
import { AgentTabsContext, type OwnedTab } from "../hooks/useAgentTabs";
import { AgentsContext } from "../hooks/useAgents";
import { PersonBrowserContext, type PersonBrowser } from "../hooks/usePersonBrowser";
import { outbox } from "../pwa/outbox";
import { composerDaemon, json, sessionFixture, type Call, type ComposerDaemonOptions } from "../test/composerDaemon";
import { Composer } from "./Composer";
import { contextItems, tabLink, type ContextSources } from "./ComposerContextMenu";
import { createSessionRequest, DEFAULT_SESSION_OPTIONS, optionsPatch } from "./sessionOptions";

afterEach(async () => {
  cleanup();
  for (const entry of outbox.snapshot()) await outbox.discard(entry.message_id);
});

const SKILLS: SkillSummary[] = [{ name: "research", description: "Deep web research", source: "user", enabled: true }];

const WEBSITE: Project = {
  id: "p1",
  name: "Website",
  root: "/home/me/code/website",
  scratch: false,
  needs_confirmation: false,
  instructions: "",
  created_at: "2026-11-06T00:00:00Z",
  updated_at: "2026-11-06T00:00:00Z",
};

const personTab = (tab_id: number, title: string, url: string, group_id: number | null = null): PersonTab => ({
  tab_id,
  title,
  url,
  group_id,
  active: false,
  audible: false,
  discarded: false,
  index: tab_id,
  muted: false,
  pinned: false,
  window_id: 1,
});

const PERSON_TABS = [
  personTab(11, "Flights to Lisbon", "https://flights.example/lis", 5),
  personTab(12, "Hotel list", "https://hotels.example/list"),
  // Already shown as this chat's agent tabs, so never listed twice.
  personTab(13, "Agent page", "https://agent.example/", 9),
];

const PERSON: PersonBrowser = {
  connected: true,
  focusedWindow: 1,
  tabs: PERSON_TABS,
  groups: [
    { group_id: 5, title: "Trip", color: "blue", collapsed: false, window_id: 1 },
    { group_id: 9, title: "Agent Tabs", color: "grey", collapsed: false, window_id: 1 },
  ],
  bookmarks: [],
  bookmarkBar: null,
  command: () => Promise.resolve(),
  refreshBookmarks: () => undefined,
};

const AGENT_TABS: OwnedTab[] = [{ tab_id: "t1", title: "Fares", url: "https://fares.example/", opened_by: "agent", owner_session_id: "s1" }];

const GITHUB: McpServer = {
  id: "github",
  label: "GitHub",
  status: "connected",
  enabled: true,
  allow_private_network: false,
  secrets: [],
  transport: "http",
  tools: [{ name: "search", exposed_name: "github_search", read_only_hint: true }],
};

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>;
}

function setup({
  variant = "home",
  sessionId = "s1",
  agents = [],
  person = PERSON,
  ...options
}: ComposerDaemonOptions & { variant?: "home" | "thread"; sessionId?: string; agents?: AgentSummary[]; person?: PersonBrowser } = {}) {
  const onSent = vi.fn();
  const daemon = composerDaemon({ skills: SKILLS, servers: [GITHUB], personTabs: person.tabs, ...options });
  const tree = (running: boolean) => (
    <ClientContext.Provider value={daemon.client}>
      <AgentsContext.Provider value={{ agents, error: null, offline: false, refresh: () => undefined }}>
        <AgentTabsContext.Provider value={AGENT_TABS}>
          <PersonBrowserContext.Provider value={person}>
            <MemoryRouter>
              <Where />
              <Routes>
                <Route path="*" element={variant === "home" ? <Composer variant="home" onSent={onSent} /> : <Composer sessionId={sessionId} running={running} />} />
              </Routes>
            </MemoryRouter>
          </PersonBrowserContext.Provider>
        </AgentTabsContext.Provider>
      </AgentsContext.Provider>
    </ClientContext.Provider>
  );
  const { rerender } = render(tree(false));
  const patches = () => daemon.calls.filter((call) => call.method === "PATCH");
  return { ...daemon, onSent, patches, setRunning: (running: boolean) => rerender(tree(running)) };
}

/** Base UI mounts a popup a frame after the click that opens it. */
async function openMenu(user: UserEvent, trigger: string | RegExp, menu: string, role: "menu" | "dialog" = "menu") {
  await user.click(await screen.findByRole("button", { name: trigger }));
  return screen.findByRole(role, { name: menu });
}

async function pick(user: UserEvent, trigger: string | RegExp, menu: string, row: string | RegExp, role: "menu" | "dialog" = "menu") {
  const popup = await openMenu(user, trigger, menu, role);
  await user.click(within(popup).getByRole("menuitemradio", { name: row }));
}

// While a submenu is open Base UI turns pointer events off on the parent popup, and jsdom has no
// layout to tell that the submenu sits outside it; this pointer skips that check for submenu rows.
const submenuPointer = userEvent.setup({ pointerEventsCheck: PointerEventsCheckLevel.Never });

async function openPermission(user: UserEvent) {
  const plus = await openMenu(user, "Add files and options", "Add to this chat");
  await user.click(within(plus).getByRole("menuitem", { name: /^Permission/ }));
  return screen.findByRole("menu", { name: "Permission" });
}

async function pickPermission(user: UserEvent, row: string) {
  const permission = await openPermission(user);
  await submenuPointer.click(within(permission).getByRole("menuitemradio", { name: row }));
}

describe("Home: a new chat", () => {
  it("creates a chat with no persona, sends to it and reports the chat it made", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup();
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Plan a trip");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    const posts = calls.filter((call) => call.method === "POST");
    expect(posts.map((call) => [call.path, call.body])).toEqual([
      ["/v1/sessions", { agent_id: null, confirmation: "ask_first" }],
      ["/v1/sessions/fresh/messages", expect.objectContaining({ text: "Plan a trip" })],
    ]);
  });

  it("locks the words while the chat is being created, then sends only what was there", async () => {
    const user = userEvent.setup();
    const { promise: created, resolve: create } = Promise.withResolvers<Response>();
    const { calls, onSent } = setup({ respond: (call) => (call.method === "POST" && call.path === "/v1/sessions" ? created : undefined) });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "Plan a trip");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(field).toHaveAttribute("aria-busy", "true"));
    expect(field).toHaveAttribute("readonly");
    await user.type(field, " and a hotel");
    expect(field).toHaveValue("Plan a trip");

    await act(async () => create(json(201, sessionFixture({ id: "fresh" }))));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    expect(field).toHaveValue("");
    expect(field).not.toHaveAttribute("readonly");
    expect(calls.filter((call) => call.path.endsWith("/messages")).map((call) => call.body)).toEqual([expect.objectContaining({ text: "Plan a trip" })]);
  });

  it("creates the chat with the permission, confirmation, model, effort and incognito picked in its toolbar", async () => {
    const user = userEvent.setup();
    const { calls, onSent, patches } = setup();
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Research flights");
    await pickPermission(user, "Read only");
    await pick(user, "Confirmation: Ask first", "Confirmation", "Skip confirmation");
    await pick(user, "Model: GPT-6.1", "Model", "GPT-6 Luna", "dialog");
    await pick(user, "Effort: Default", "Effort", "High");
    const plus = await openMenu(user, "Add files and options", "Add to this chat");
    await user.click(within(plus).getByRole("menuitemradio", { name: "Incognito mode" }));
    expect(screen.getByRole("button", { name: "Confirmation: Skip confirmation" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Model: GPT-6 Luna" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    const [create, submit] = calls.filter((call) => call.method === "POST");
    expect(create).toMatchObject({ path: "/v1/sessions" });
    expect(create?.body).toEqual({
      agent_id: null,
      incognito: true,
      browser_space: "shared",
      mode: "read-only",
      model: "openai:gpt-6-luna",
      effort: "high",
      confirmation: "skip",
    });
    expect(submit).toMatchObject({ path: "/v1/sessions/fresh/messages", body: { text: "Research flights" } });
    expect(patches()).toEqual([]);
    // The toolbar starts over for the next chat.
    expect(screen.getByRole("button", { name: "Effort: Low" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Model: GPT-6.1" })).toBeInTheDocument();
  });

  it("asks for the Ultrabrowse profile instead of an effort", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup();
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Compare every fare");
    await pick(user, "Effort: Low", "Effort", "High");
    await pick(user, "Effort: High", "Effort", "Ultrabrowse");
    expect(screen.getByRole("button", { name: "Effort: Ultrabrowse" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalled());
    expect(calls.find((call) => call.path === "/v1/sessions")?.body).toEqual({ agent_id: null, confirmation: "ask_first", profile: "ultrabrowse" });
  });

  it("starts the confirmation chip from the Preferences default and creates the chat with it", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup({ preferences: { confirmation: "skip" } });
    expect(await screen.findByRole("button", { name: "Confirmation: Skip confirmation" })).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Book it");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    expect(calls.find((call) => call.path === "/v1/sessions")?.body).toEqual({ agent_id: null, confirmation: "skip" });
  });

  it("attaches tabs picked before the chat existed once it is created", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup();
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "Compare @hotel");
    await user.click(screen.getByRole("option", { name: /Hotel list/ }));
    expect(screen.getByRole("button", { name: "Remove tab Hotel list" })).toBeInTheDocument();
    expect(calls.some((call) => call.path.includes("/attached-tabs"))).toBe(false);
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    expect(calls.filter((call) => call.method === "POST").map((call) => [call.path, call.body])).toEqual([
      ["/v1/sessions", { agent_id: null, confirmation: "ask_first" }],
      ["/v1/sessions/fresh/attached-tabs", { tab_id: 12 }],
      ["/v1/sessions/fresh/messages", expect.objectContaining({ text: "Compare [Hotel list](https://hotels.example/list)" })],
    ]);
  });

  it("sends nothing when a picked tab cannot be attached and hands the words and that tab to the new chat", async () => {
    const user = userEvent.setup();
    const home = setup({
      respond: (call) =>
        call.method === "POST" && call.path.endsWith("/attached-tabs") && (call.body as { tab_id: number }).tab_id === 11
          ? json(409, { code: "person_browser_not_connected", message: "x" })
          : undefined,
    });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "Compare @hotel");
    await user.click(screen.getByRole("option", { name: /Hotel list/ }));
    await act(() => new Promise<void>((done) => requestAnimationFrame(() => done())));
    await user.type(field, "and @flights", { skipClick: true });
    await user.click(screen.getByRole("option", { name: /Flights to Lisbon/ }));
    const typed = (field as HTMLTextAreaElement).value;

    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(home.onSent).toHaveBeenCalledWith("fresh"));
    expect(home.calls.filter((call) => call.method === "POST").map((call) => [call.path, call.body])).toEqual([
      ["/v1/sessions", { agent_id: null, confirmation: "ask_first" }],
      ["/v1/sessions/fresh/attached-tabs", { tab_id: 12 }],
      ["/v1/sessions/fresh/attached-tabs", { tab_id: 11 }],
    ]);
    expect(field).toHaveValue("");
    cleanup();

    // The chat Home navigated to picks the handoff up: the words, the error and the refused tab.
    const chat = setup({ variant: "thread", sessionId: "fresh" });
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue(typed);
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn’t attach the tab. Try again.");
    expect(screen.queryByRole("button", { name: "Remove tab Hotel list" })).toBeNull();
    const retry = screen.getByRole("button", { name: "Try attaching Flights to Lisbon again" });
    expect(retry.closest("li")).toHaveClass("tab-chip", "is-failed");

    await user.click(retry);
    await waitFor(() => expect(screen.queryByRole("button", { name: "Try attaching Flights to Lisbon again" })).toBeNull());
    expect(screen.getByRole("button", { name: "Remove tab Flights to Lisbon" }).closest("li")).not.toHaveClass("is-failed");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(chat.calls.filter((call) => call.method === "POST" && call.path.endsWith("/attached-tabs")).map((call) => [call.path, call.body])).toEqual([
      ["/v1/sessions/fresh/attached-tabs", { tab_id: 11 }],
    ]);
    expect(chat.calls.some((call) => call.path.endsWith("/messages"))).toBe(false);
  });

  it("drops a refused tab from the new chat with its X", async () => {
    const user = userEvent.setup();
    const home = setup({ respond: (call) => (call.method === "POST" && call.path.endsWith("/attached-tabs") ? json(409, { code: "person_browser_not_connected", message: "x" }) : undefined) });
    await user.type(screen.getByRole("textbox", { name: "Message" }), "See @hotel");
    await user.click(screen.getByRole("option", { name: /Hotel list/ }));
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(home.onSent).toHaveBeenCalledWith("fresh"));
    cleanup();

    const chat = setup({ variant: "thread", sessionId: "fresh" });
    await user.click(screen.getByRole("button", { name: "Remove tab Hotel list" }));
    expect(screen.queryByRole("button", { name: "Try attaching Hotel list again" })).toBeNull();
    expect(chat.calls.some((call) => call.path.includes("/attached-tabs/"))).toBe(false);
  });

  it("keeps a paired device's permission at or below the daemon default and links to the global setting", async () => {
    const user = userEvent.setup();
    setup({ policyMode: "guard" });
    const permission = await openPermission(user);
    await waitFor(() => expect(within(permission).getByRole("menuitemradio", { name: "Full access" })).toHaveAttribute("aria-disabled", "true"));
    expect(within(permission).getByRole("menuitemradio", { name: "Guard" })).toHaveAttribute("aria-checked", "true");
    expect(within(permission).getByRole("menuitemradio", { name: "Read only" })).not.toHaveAttribute("aria-disabled");
    await submenuPointer.click(within(permission).getByRole("menuitem", { name: "Global setting" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/settings/agents");
  });

  it("starts the chat in the project picked from the Project chip", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup({ projects: [WEBSITE] });
    const projects = await openMenu(user, /^Project$/, "Projects");
    await user.click(await within(projects).findByRole("menuitemradio", { name: "Website" }));
    expect(await screen.findByRole("button", { name: "Website" })).toBeInTheDocument();

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Fix the header");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    const create = calls.find((call) => call.method === "POST" && call.path === "/v1/sessions");
    expect(create?.body).toEqual({ agent_id: null, confirmation: "ask_first", project_id: "p1" });
    // The next chat starts outside any project again.
    expect(screen.getByRole("button", { name: "Project" })).toBeInTheDocument();
  });

  it("makes a project from a folder picked on this computer and starts the chat in it", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup({ remote: false, pickedFolder: "/home/me/code/site" });
    const projects = await openMenu(user, /^Project$/, "Projects");
    const existing = within(projects).getByRole("menuitem", { name: "Use an existing folder" });
    await waitFor(() => expect(existing).not.toHaveAttribute("aria-disabled"));
    await user.click(existing);
    expect(await screen.findByRole("button", { name: "site" })).toBeInTheDocument();
    const posts = calls.filter((call) => call.method === "POST");
    expect(posts.map((call) => [call.path, call.body])).toEqual([
      ["/v1/folders/pick", undefined],
      ["/v1/projects", { name: "site", root: "/home/me/code/site" }],
    ]);

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Read the README");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    expect(calls.find((call) => call.method === "POST" && call.path === "/v1/sessions")?.body).toMatchObject({ project_id: "p1" });
  });

  it("attaches a folder picked on this computer and starts the chat in it", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup({ remote: false, pickedFolder: "/home/me/code/site" });
    const plus = await openMenu(user, "Add files and options", "Add to this chat");
    const row = within(plus).getByRole("menuitem", { name: "Attach folder" });
    await waitFor(() => expect(row).not.toHaveAttribute("aria-disabled"));
    await user.click(row);
    expect(await screen.findByRole("button", { name: "site" })).toBeInTheDocument();
    expect(calls.filter((call) => call.method === "POST").map((call) => call.path)).toEqual(["/v1/folders/pick"]);

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Read the README");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    expect(calls.find((call) => call.method === "POST" && call.path === "/v1/sessions")?.body).toEqual({ agent_id: null, confirmation: "ask_first", workspace: "/home/me/code/site" });
  });

  it("keeps making projects and attaching folders to the computer running openside, and says so", async () => {
    const user = userEvent.setup();
    const { writes } = setup({ projects: [WEBSITE] });
    const note = "Only on the computer running openside";
    const projects = await openMenu(user, /^Project$/, "Projects");
    await waitFor(() => expect(within(projects).getByRole("menuitem", { name: "Use an existing folder" })).toHaveAccessibleDescription(note));
    expect(within(projects).getByRole("menuitem", { name: "Use an existing folder" })).toHaveAttribute("aria-disabled", "true");
    expect(within(projects).getByRole("menuitem", { name: "Start from scratch" })).toHaveAttribute("aria-disabled", "true");
    // Existing projects stay open to every device.
    expect(within(projects).getByRole("menuitemradio", { name: "Website" })).not.toHaveAttribute("aria-disabled");
    await user.keyboard("{Escape}");

    const plus = await openMenu(user, "Add files and options", "Add to this chat");
    const row = within(plus).getByRole("menuitem", { name: "Attach folder" });
    expect(row).toHaveAttribute("aria-disabled", "true");
    expect(row).toHaveAccessibleName("Attach folder");
    expect(row).toHaveAccessibleDescription(note);

    await user.keyboard("{ArrowUp}");
    await waitFor(() => expect(row).toHaveFocus());
    const tooltip = await waitFor(() => {
      const popup = document.querySelector(".ui-tooltip");
      expect(popup).not.toBeNull();
      return popup;
    });
    expect(tooltip).toHaveTextContent(note);
    expect(writes()).toEqual([]);
  });

  it("disables a project whose folder needs confirming, with the reason", async () => {
    const user = userEvent.setup();
    const { writes } = setup({ projects: [{ ...WEBSITE, needs_confirmation: true }] });
    const projects = await openMenu(user, /^Project$/, "Projects");
    const row = await within(projects).findByRole("menuitemradio", { name: "Website" });
    expect(row).toHaveAttribute("aria-disabled", "true");
    expect(row).toHaveAccessibleDescription("Confirm its folder in Settings › Projects first");
    await user.click(row);
    expect(row).toHaveAttribute("aria-checked", "false");
    expect(writes()).toEqual([]);
  });

  it("moves an existing chat into a project, and out again, with PUT …/folder", async () => {
    const user = userEvent.setup();
    const { writes } = setup({ variant: "thread", projects: [WEBSITE] });
    // The chat's options arrive first; until then the composer has nothing to change.
    await screen.findByRole("button", { name: /^Confirmation/ });
    const plus = await openMenu(user, "Add files and options", "Add to this chat");
    await user.click(within(plus).getByRole("menuitem", { name: /^Projects/ }));
    const projects = await screen.findByRole("menu", { name: "Projects" });
    await submenuPointer.click(await within(projects).findByRole("menuitemradio", { name: "Website" }));
    await waitFor(() => expect(writes().map((call) => [call.method, call.path, call.body])).toEqual([["PUT", "/v1/sessions/s1/folder", { project_id: "p1", workspace: null }]]));

    const again = await openMenu(user, "Add files and options", "Add to this chat");
    await user.click(within(again).getByRole("menuitem", { name: /^Projects/ }));
    const rows = await screen.findByRole("menu", { name: "Projects" });
    expect(within(rows).getByRole("menuitemradio", { name: "Website" })).toHaveAttribute("aria-checked", "true");
    await submenuPointer.click(within(rows).getByRole("menuitemradio", { name: "No project" }));
    await waitFor(() => expect(writes()).toHaveLength(2));
    expect(writes()[1]?.body).toEqual({ project_id: null, workspace: null });
  });

  it("leaves No project unchecked for a chat working in a folder of its own, and checks it once the chat leaves it", async () => {
    const user = userEvent.setup();
    const { writes } = setup({ variant: "thread", projects: [WEBSITE], session: { workspace: "/home/me/elsewhere" } });
    await screen.findByRole("button", { name: /^Confirmation/ });
    const plus = await openMenu(user, "Add files and options", "Add to this chat");
    await user.click(within(plus).getByRole("menuitem", { name: /^Projects/ }));
    const rows = await screen.findByRole("menu", { name: "Projects" });
    expect(within(rows).getByRole("menuitemradio", { name: "No project" })).toHaveAttribute("aria-checked", "false");
    expect(within(rows).getByRole("menuitemradio", { name: "Website" })).toHaveAttribute("aria-checked", "false");
    await submenuPointer.click(within(rows).getByRole("menuitemradio", { name: "No project" }));
    await waitFor(() => expect(writes().map((call) => call.body)).toEqual([{ project_id: null, workspace: null }]));

    const again = await openMenu(user, "Add files and options", "Add to this chat");
    await user.click(within(again).getByRole("menuitem", { name: /^Projects/ }));
    const after = await screen.findByRole("menu", { name: "Projects" });
    expect(within(after).getByRole("menuitemradio", { name: "No project" })).toHaveAttribute("aria-checked", "true");
  });

  it("names the chat's folder in full on the + menu's Projects row, which cuts a long name short", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread", projects: [WEBSITE], session: { workspace: "/home/me/a-folder-name-longer-than-the-menu" } });
    await screen.findByRole("button", { name: /^Confirmation/ });
    const plus = await openMenu(user, "Add files and options", "Add to this chat");
    const row = within(plus).getByRole("menuitem", { name: /^Projects/ });
    expect(within(row).getByTitle("a-folder-name-longer-than-the-menu")).toHaveTextContent(/^a-folder-name-longer-than-the-menu$/);
  });

  it("offers Low to Max and Ultrabrowse, and opens on Mod+Shift+.", async () => {
    setup();
    await screen.findByRole("button", { name: "Effort: Low" });
    fireEvent.keyDown(window, { key: ">", code: "Period", ctrlKey: true, shiftKey: true });
    const menu = await screen.findByRole("menu", { name: "Effort" });
    expect(within(menu).getAllByRole("menuitemradio").map((row) => row.textContent)).toEqual(["Low", "Medium", "High", "Extra High", "Max", "Ultrabrowse"]);
  });

  it("withholds the costly efforts and Ultrabrowse from an agent client", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ deviceKind: "agent_client" });
    await waitFor(() => expect(calls.some((call) => call.path === "/v1/devices/me")).toBe(true));
    const menu = await openMenu(user, "Effort: Low", "Effort");
    for (const name of ["Extra High", "Max", "Ultrabrowse"]) expect(within(menu).getByRole("menuitemradio", { name })).toHaveAttribute("aria-disabled", "true");
    for (const name of ["Low", "Medium", "High"]) expect(within(menu).getByRole("menuitemradio", { name })).not.toHaveAttribute("aria-disabled");
  });

  it("shows the daemon's refusal to create the chat and keeps the words", async () => {
    const user = userEvent.setup();
    const { onSent } = setup({ respond: (call) => (call.method === "POST" && call.path === "/v1/sessions" ? json(403, { code: "local_socket_required", message: "x" }) : undefined) });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "Go");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Only the computer running openside can make this change");
    expect(field).toHaveValue("Go");
    expect(onSent).not.toHaveBeenCalled();
  });

  it("does not start a chat offline: the words and settings stay and nothing is queued", async () => {
    const user = userEvent.setup();
    const { onSent } = setup({
      respond: (call) => {
        if (call.method === "POST" && call.path === "/v1/sessions") throw new TypeError("Failed to fetch");
        return undefined;
      },
    });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "Later please");
    await pick(user, "Effort: Low", "Effort", "Low");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Can’t start a new chat while offline");
    expect(field).toHaveValue("Later please");
    expect(screen.getByRole("button", { name: "Effort: Low" })).toBeInTheDocument();
    expect(outbox.snapshot()).toHaveLength(0);
    expect(onSent).not.toHaveBeenCalled();
  });
});

describe("Thread: options change the chat in place", () => {
  it("sends only the changed field with each PATCH and shows the chat's answer", async () => {
    const user = userEvent.setup();
    const { patches } = setup({ variant: "thread" });
    await pickPermission(user, "Read only");
    await pick(user, "Confirmation: Ask first", "Confirmation", "Skip confirmation");
    await pick(user, "Effort: Low", "Effort", "High");
    await pick(user, "Model: GPT-6.1", "Model", "GPT-6 Luna", "dialog");
    await waitFor(() => expect(patches()).toHaveLength(4));
    expect(patches().map((call) => [call.path, call.body])).toEqual([
      ["/v1/sessions/s1", { mode: "read-only" }],
      ["/v1/sessions/s1", { confirmation: "skip" }],
      ["/v1/sessions/s1", { effort: "high" }],
      ["/v1/sessions/s1", { model: "openai:gpt-6-luna" }],
    ]);
    expect(screen.getByRole("button", { name: "Confirmation: Skip confirmation" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Effort: High" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Model: GPT-6 Luna" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("puts the option back and says so when the PATCH fails", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread", respond: (call) => (call.method === "PATCH" ? json(500, { code: "internal", message: "boom" }) : undefined) });
    await pick(user, "Effort: Low", "Effort", "High");
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn’t change this chat’s settings. Try again.");
    expect(screen.getByRole("button", { name: "Effort: Low" })).toBeInTheDocument();
  });

  it("names a refusal it can explain instead of the generic failure", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread", respond: (call) => (call.method === "PATCH" ? json(403, { code: "local_socket_required", message: "x" }) : undefined) });
    await pick(user, "Confirmation: Ask first", "Confirmation", "Skip confirmation");
    expect(await screen.findByRole("alert")).toHaveTextContent("Only the computer running openside can make this change");
    expect(screen.getByRole("button", { name: "Confirmation: Ask first" })).toBeInTheDocument();
  });

  it("keeps what a chat was created with: incognito and Ultrabrowse cannot be switched on later", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread" });
    const effort = await openMenu(user, "Effort: Low", "Effort");
    expect(within(effort).getByRole("menuitemradio", { name: "Ultrabrowse" })).toHaveAttribute("aria-disabled", "true");
    await user.keyboard("{Escape}");
    const plus = await openMenu(user, "Add files and options", "Add to this chat");
    expect(within(plus).getByRole("menuitemradio", { name: "Incognito mode" })).toHaveAttribute("aria-disabled", "true");
  });

  it("keeps the tabs of a picked group that attached and lists a refused one to retry", async () => {
    const user = userEvent.setup();
    const tabs = [...PERSON_TABS, personTab(14, "Return flight", "https://flights.example/ret", 5)];
    let refuse = true;
    const { calls } = setup({
      variant: "thread",
      person: { ...PERSON, tabs },
      respond: (call) =>
        refuse && call.method === "POST" && call.path.endsWith("/attached-tabs") && (call.body as { tab_id: number }).tab_id === 14
          ? json(409, { code: "person_browser_not_connected", message: "x" })
          : undefined,
    });
    const attachPosts = () => calls.filter((call) => call.method === "POST" && call.path.endsWith("/attached-tabs")).map((call) => call.body);
    await user.type(screen.getByRole("textbox", { name: "Message" }), "@trip");
    await user.click(await screen.findByRole("option", { name: /^Trip/ }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn’t attach the tab. Try again.");
    expect(attachPosts()).toEqual([{ tab_id: 11 }, { tab_id: 14 }]);
    expect(screen.getByRole("button", { name: "Remove tab Flights to Lisbon" }).closest("li")).not.toHaveClass("is-failed");
    const retry = screen.getByRole("button", { name: "Try attaching Return flight again" });
    expect(retry.closest("li")).toHaveClass("tab-chip", "is-failed");

    refuse = false;
    await user.click(retry);
    await waitFor(() => expect(screen.queryByRole("button", { name: "Try attaching Return flight again" })).toBeNull());
    expect(attachPosts()).toEqual([{ tab_id: 11 }, { tab_id: 14 }, { tab_id: 14 }]);
    expect(screen.getByRole("button", { name: "Remove tab Return flight" }).closest("li")).not.toHaveClass("is-failed");
    expect(screen.getByRole("button", { name: "Remove tab Flights to Lisbon" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps an agent's own chat on the agent's model", async () => {
    setup({ variant: "thread", agents: [{ id: "a1", home_session_id: "s1" } as AgentSummary] });
    const chip = await screen.findByRole("button", { name: "Model: GPT-6.1" });
    expect(chip).toBeDisabled();
    expect(chip).toHaveAttribute("title", "An agent’s own chat keeps the agent’s model.");
  });

  /**
   * Every PATCH is held until the test answers it. An accepted change is applied then, and its
   * answer reports the chat as of that change only; a refused one changes nothing.
   */
  function heldPatches() {
    let chat = sessionFixture();
    const held: { body: unknown; answer: (ok: boolean) => void }[] = [];
    const respond = (call: Call) => {
      // Reads answer with the chat as it stands, so one sent after a change reports that change.
      if (call.method === "GET" && call.path === "/v1/sessions/s1") return json(200, chat);
      if (call.method !== "PATCH") return undefined;
      const { promise, resolve } = Promise.withResolvers<Response>();
      const answer = (ok: boolean) => {
        if (!ok) return resolve(json(500, { code: "internal", message: "boom" }));
        chat = { ...chat, ...(call.body as object) };
        resolve(json(200, chat));
      };
      held.push({ body: call.body, answer });
      return promise;
    };
    const answer = async (index: number, ok = true) => {
      await act(async () => held[index]?.answer(ok));
    };
    return { held, respond, answer };
  }

  it("sends option changes one at a time and keeps a newer pending choice over an older answer", async () => {
    const user = userEvent.setup();
    const { held, respond, answer } = heldPatches();
    setup({ variant: "thread", respond });
    await pick(user, "Effort: Low", "Effort", "High");
    await pick(user, "Model: GPT-6.1", "Model", "GPT-6 Luna", "dialog");
    // The model change waits for the effort change to be answered.
    expect(held.map((patch) => patch.body)).toEqual([{ effort: "high" }]);
    expect(screen.getByRole("button", { name: "Model: GPT-6 Luna" })).toBeInTheDocument();

    // That answer still names the old model; the chip keeps the newer pick.
    await answer(0);
    await waitFor(() => expect(held).toHaveLength(2));
    expect(held[1]?.body).toEqual({ model: "openai:gpt-6-luna" });
    expect(screen.getByRole("button", { name: "Effort: High" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Model: GPT-6 Luna" })).toBeInTheDocument();

    await answer(1);
    expect(screen.getByRole("button", { name: "Effort: High" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Model: GPT-6 Luna" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("reverts only the field whose PATCH failed, keeping a later change on its way", async () => {
    const user = userEvent.setup();
    const { held, respond, answer } = heldPatches();
    setup({ variant: "thread", respond });
    await pick(user, "Effort: Low", "Effort", "High");
    await pick(user, "Model: GPT-6.1", "Model", "GPT-6 Luna", "dialog");

    await answer(0, false);
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn’t change this chat’s settings. Try again.");
    // The effort reverts to unset, so the chip names the newly picked model's own level.
    expect(screen.getByRole("button", { name: "Effort: Default" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Model: GPT-6 Luna" })).toBeInTheDocument();

    await waitFor(() => expect(held).toHaveLength(2));
    await answer(1);
    expect(screen.getByRole("button", { name: "Model: GPT-6 Luna" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Effort: Default" })).toBeInTheDocument();
  });

  it("reads the chat again only after a change in flight is answered, so the read cannot undo it", async () => {
    const user = userEvent.setup();
    const { held, respond, answer } = heldPatches();
    const { calls, setRunning } = setup({ variant: "thread", respond });
    const reads = () => calls.filter((call) => call.method === "GET" && call.path === "/v1/sessions/s1").length;
    await waitFor(() => expect(reads()).toBe(1));
    await pick(user, "Confirmation: Ask first", "Confirmation", "Skip confirmation");
    expect(held).toHaveLength(1);

    // A run starting asks for a fresh read; it waits behind the open change.
    setRunning(true);
    await act(() => new Promise((done) => setTimeout(done, 20)));
    expect(reads()).toBe(1);

    await answer(0);
    await waitFor(() => expect(reads()).toBe(2));
    await act(() => new Promise((done) => setTimeout(done, 20)));
    expect(screen.getByRole("button", { name: "Confirmation: Skip confirmation" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("Effort chip: the model's own level", () => {
  it("names the level the chat's model runs at while no effort is picked, follows the model, and sends no effort", async () => {
    const user = userEvent.setup();
    const { calls, onSent } = setup();
    const menu = await openMenu(user, "Effort: Low", "Effort");
    // The model's level is not a pick: no row is checked.
    for (const row of within(menu).getAllByRole("menuitemradio")) expect(row).toHaveAttribute("aria-checked", "false");
    await user.keyboard("{Escape}");
    await pick(user, "Model: GPT-6.1", "Model", "Claude Sonnet 5.5", "dialog");
    expect(await screen.findByRole("button", { name: "Effort: High" })).toBeInTheDocument();
    // A model whose level the provider picks reads as the default.
    await pick(user, "Model: Claude Sonnet 5.5", "Model", "GPT-6 Luna", "dialog");
    expect(await screen.findByRole("button", { name: "Effort: Default" })).toBeInTheDocument();

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Plan a trip");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSent).toHaveBeenCalledWith("fresh"));
    expect(calls.find((call) => call.path === "/v1/sessions")?.body).toEqual({ agent_id: null, model: "openai:gpt-6-luna", confirmation: "ask_first" });
  });

  it("shows an existing chat's model level without changing the chat", async () => {
    const { patches } = setup({ variant: "thread" });
    expect(await screen.findByRole("button", { name: "Effort: Low" })).toBeInTheDocument();
    expect(patches()).toEqual([]);
  });
});

describe("context ring", () => {
  it("shows how full the chat's context is", async () => {
    setup({ variant: "thread", session: { context: { used_tokens: 50_000, window_tokens: 200_000 } } });
    expect(await screen.findByRole("img", { name: "Context 25% full · 50K of 200K tokens" })).toBeInTheDocument();
  });

  it("says when the size is not known yet", async () => {
    setup({ variant: "thread", session: { context: { used_tokens: 1200, window_tokens: null } } });
    expect(await screen.findByRole("img", { name: "Context use shows after the first reply" })).toBeInTheDocument();
  });
});

describe("@ and / menus", () => {
  it("lists Tab Groups, Commands, Open Tabs, Skills and Apps after @", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread" });
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Compare @");
    const menu = screen.getByRole("listbox", { name: "Add context" });
    await within(menu).findByRole("group", { name: "Apps" });
    await within(menu).findByRole("group", { name: "Skills" });
    expect(within(menu).getAllByRole("group").map((group) => group.getAttribute("aria-labelledby"))).toEqual([
      "context-groups",
      "context-commands",
      "context-tabs",
      "context-skills",
      "context-apps",
    ]);
    const names = (section: string) => within(within(menu).getByRole("group", { name: section })).getAllByRole("option").map((row) => row.textContent);
    expect(names("Tab Groups")).toEqual(["Agent Tabs1 tab", "Trip1 tab"]);
    expect(names("Commands")).toEqual(["ClearStart over in an empty chat with the same settings.", "CompactLong chats are condensed automatically.", "FeedbackReport a problem with openside.", "NewOpen a new chat."]);
    expect(names("Open Tabs")).toEqual(["Flights to Lisbonflights.example", "Hotel listhotels.example"]);
    expect(names("Skills")).toEqual(["/researchDeep web research"]);
    expect(names("Apps")).toEqual(["GitHub1 tool"]);
  });

  it("attaches a picked open tab to the chat and detaches it with its chip", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ variant: "thread" });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "See @");
    await user.click(screen.getByRole("option", { name: /Hotel list/ }));
    expect(field).toHaveValue("See [Hotel list](https://hotels.example/list) ");
    await waitFor(() => expect(calls.find((call) => call.method === "POST" && call.path.endsWith("/attached-tabs"))).toMatchObject({ path: "/v1/sessions/s1/attached-tabs", body: { tab_id: 12 } }));
    await user.click(await screen.findByRole("button", { name: "Remove tab Hotel list" }));
    await waitFor(() => expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/sessions/s1/attached-tabs/12")).toBe(true));
    expect(screen.queryByRole("button", { name: "Remove tab Hotel list" })).toBeNull();
  });

  it("shows tabs the chat already has as chips", async () => {
    setup({ variant: "thread", attached: [{ tab_id: 11, title: "Flights to Lisbon", url: "https://flights.example/lis", attached_at: "2026-11-06T00:00:00Z", attached_by: "agent" }] });
    expect(await screen.findByRole("button", { name: "Remove tab Flights to Lisbon" })).toBeInTheDocument();
  });

  it("offers only Commands and Skills after /", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread" });
    await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
    const menu = screen.getByRole("listbox", { name: "Add context" });
    await within(menu).findByRole("group", { name: "Skills" });
    expect(within(menu).getAllByRole("group").map((group) => group.getAttribute("aria-labelledby"))).toEqual(["context-commands", "context-skills"]);
  });

  it("filters by what follows @, wraps with the arrows, runs commands and closes with Escape", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread" });
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "@new");
    const menu = screen.getByRole("listbox", { name: "Add context" });
    expect(within(menu).getAllByRole("option").map((row) => row.textContent)).toEqual(["NewOpen a new chat."]);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox", { name: "Add context" })).toBeNull();

    await user.clear(field);
    await user.type(field, "@");
    await user.keyboard("{ArrowUp}");
    const rows = within(screen.getByRole("listbox", { name: "Add context" })).getAllByRole("option");
    // Up from the first row lands on the last one present when the key was pressed.
    expect(rows.find((row) => row.getAttribute("aria-selected") === "true")).toBe(rows.at(-1));

    await user.clear(field);
    await user.type(field, "@new{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/$/);
    expect(field).toHaveValue("");
  });

  it("starts over in an empty chat with the same options from Clear", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ variant: "thread", session: { mode: "read-only", effort: "high" } });
    await screen.findByRole("button", { name: "Effort: High" });
    await user.type(screen.getByRole("textbox", { name: "Message" }), "@clear{Enter}");
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent("/sessions/fresh"));
    expect(calls.find((call) => call.path === "/v1/sessions")?.body).toEqual({ agent_id: null, mode: "read-only", model: "openai:gpt-6.1", effort: "high", confirmation: "ask_first" });
  });

  it("never opens inside a word", async () => {
    const user = userEvent.setup();
    setup({ variant: "thread" });
    await user.type(screen.getByRole("textbox", { name: "Message" }), "mail me@example");
    expect(screen.queryByRole("listbox", { name: "Add context" })).toBeNull();
    await act(async () => undefined);
  });
});

describe("contextItems", () => {
  const base: ContextSources = {
    trigger: "@",
    query: "",
    chatTabs: AGENT_TABS,
    agentTabs: AGENT_TABS,
    personConnected: false,
    personTabs: [],
    personGroups: [],
    commands: [],
    skills: [],
    apps: [],
  };

  it("lists every agent tab as an open tab while the person's browser is away, without attaching it", () => {
    const rows = contextItems(base);
    expect(rows.map((row) => [row.section, row.label])).toEqual([
      ["groups", "Agent Tabs"],
      ["tabs", "Fares"],
    ]);
    expect(rows[1]?.attach).toBeUndefined();
  });

  it("attaches every member of a picked person group", () => {
    const rows = contextItems({ ...base, chatTabs: [], personConnected: true, personTabs: [PERSON_TABS[0]!, personTab(14, "Return", "https://flights.example/ret", 5)], personGroups: PERSON.groups });
    expect(rows[0]).toMatchObject({ section: "groups", label: "Trip", detail: "2 tabs", attach: [11, 14] });
  });

  it("leaves apps that are not connected out", () => {
    const rows = contextItems({ ...base, chatTabs: [], agentTabs: [], apps: [GITHUB, { ...GITHUB, id: "down", label: "Down", status: "failed" }] });
    expect(rows.map((row) => row.label)).toEqual(["GitHub"]);
  });
});

describe("tabLink", () => {
  it("links only http and https pages", () => {
    expect(tabLink({ title: "Run me", url: "javascript:alert(1)" })).toBe("Run me");
    expect(tabLink({ title: "Inline", url: "data:text/html,<b>x</b>" })).toBe("Inline");
    expect(tabLink({ title: "", url: "file:///etc/passwd" })).toBe("Untitled tab");
    expect(tabLink({ title: "Docs", url: "https://docs.example/a" })).toBe("[Docs](https://docs.example/a)");
  });

  it("keeps parentheses, spaces and brackets from breaking out of the link", () => {
    expect(tabLink({ title: "Foo (bar)", url: "https://en.example/wiki/Foo_(bar)" })).toBe("[Foo (bar)](https://en.example/wiki/Foo_%28bar%29)");
    expect(tabLink({ title: "a](javascript:x) [b", url: "https://x.example/p?q=a b" })).toBe("[a\\](javascript:x) \\[b](https://x.example/p?q=a%20b)");
    expect(tabLink({ title: "back\\slash\nnext", url: "https://x.example/" })).toBe("[back\\\\slash next](https://x.example/)");
  });
});

describe("createSessionRequest", () => {
  it("leaves untouched options out and never names a persona", () => {
    expect(createSessionRequest(DEFAULT_SESSION_OPTIONS)).toEqual({ agent_id: null });
  });

  it("puts an incognito chat in the shared browser space", () => {
    expect(createSessionRequest({ ...DEFAULT_SESSION_OPTIONS, incognito: true, confirmation: "skip" })).toEqual({ agent_id: null, incognito: true, browser_space: "shared", confirmation: "skip" });
  });
});

describe("optionsPatch", () => {
  it("carries only the fields an existing chat can change", () => {
    expect(optionsPatch({ effort: "low" })).toEqual({ effort: "low" });
    expect(optionsPatch({ incognito: true, profile: "ultrabrowse" })).toEqual({});
  });
});
