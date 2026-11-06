import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Link, MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { addressTarget, openAddress } from "../components/AppHeader";
import type * as AppHeaderModule from "../components/AppHeader";
import { saveNewTabMode } from "../appearance";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { ShellContext, type Shell } from "../shell";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { HomePage } from "./HomePage";

// jsdom cannot navigate away, so the hand-off to the browser is observed instead.
vi.mock("../components/AppHeader", async (importOriginal) => ({ ...(await importOriginal<typeof AppHeaderModule>()), openAddress: vi.fn() }));

const NO_BROWSER = { status: "not_running", prefs: null };
const SHELL_DEFAULTS: Shell = { openDrawer: () => undefined, openSearch: () => undefined, refreshSessions: () => undefined, needsYou: 0, sidebarCollapsed: false, toggleSidebar: () => undefined };
// A Korean browser profile: the engine's display name is localised, its prepopulated id (67) is not.
const NAVER_KO = { status: "available", prefs: { search_engine: { current: 67, options: [{ id: 1, name: "Google" }, { id: 67, name: "네이버" }, { id: 68, name: "Daum" }] } } };

interface Setup {
  /** How many chat creations fail before one succeeds. */
  failedCreates?: number;
  prefs?: object;
  /** Holds the browser-prefs answer back until the test resolves it. */
  prefsAnswer?: Promise<Response>;
  /** Chats waiting for the person, as the shell counts them. */
  needsYou?: number;
  /** How many message submissions fail before one succeeds. */
  failedSubmits?: number;
  /** Where this tab's history starts; `/` is a freshly opened new tab. */
  path?: string;
}

function setup({ failedCreates = 0, failedSubmits = 0, prefs = NO_BROWSER, prefsAnswer, needsYou = 0, path = "/" }: Setup = {}) {
  let failures = failedCreates;
  let submitFailures = failedSubmits;
  const daemon = fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (call.method === "POST" && url.pathname === "/v1/sessions") return failures-- > 0 ? json(500, { code: "internal", message: "The daemon could not start the chat" }) : json(201, { id: "s-new" });
    if (call.method === "POST" && url.pathname === "/v1/sessions/s-new/messages") return submitFailures-- > 0 ? json(500, { code: "internal", message: "The daemon could not take the message" }) : json(202, { id: "m", status: "queued" });
    if (call.method === "GET" && url.pathname === "/v1/browser/prefs") return prefsAnswer ?? json(200, prefs);
    return json(404, { code: "not_found", message: "not found" });
  });
  render(
    <ClientContext.Provider value={daemon.client}>
      <ShellContext.Provider value={{ ...SHELL_DEFAULTS, needsYou }}>
      {/* No assistant persona: Home works without any agent (aside-gap-audit D1). */}
      <AgentsContext.Provider value={{ agents: [], error: null, offline: false, refresh: () => {} }}>
        <ToastProvider>
          <MemoryRouter initialEntries={[path]}>
            <Routes>
              <Route index element={<HomePage />} />
              <Route path="/sessions/:id" element={<p>session page</p>} />
              {/* Stand-ins for the sidebar's New Chat and Settings › General's New tab mode control. */}
              <Route path="/chats" element={<Link to="/">New Chat</Link>} />
              <Route
                path="/settings"
                element={
                  <>
                    <p>settings page</p>
                    <button type="button" onClick={() => saveNewTabMode("ask")}>New tabs open in Ask</button>
                    <button type="button" onClick={() => saveNewTabMode("search")}>New tabs open in Search</button>
                    <Link to="/">New Chat</Link>
                  </>
                }
              />
            </Routes>
          </MemoryRouter>
        </ToastProvider>
      </AgentsContext.Provider>
      </ShellContext.Provider>
    </ClientContext.Provider>,
  );
  return daemon;
}

const message = () => screen.getByRole("textbox", { name: "Message" });
const webSearch = () => screen.getByRole("searchbox", { name: "Search the web or enter an address" });

// These cases exercise Ask on arrival; a new tab's default (Search) has its own case below.
beforeEach(() => localStorage.setItem("openside.newTabMode", "ask"));
afterEach(() => localStorage.clear());

describe("HomePage", () => {
  it("opens a new tab in Search unless Ask was chosen in Settings", async () => {
    localStorage.clear();
    const { calls } = setup();
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-selected", "true");
    expect(webSearch()).toHaveAttribute("placeholder", "Search or type a URL");
    expect(webSearch()).toHaveFocus();
    // The engine comes from the browser's own default.
    await waitFor(() => expect(calls.some((call) => call.path === "/v1/browser/prefs")).toBe(true));
  });

  it("opens in Ask when New Chat leads to Home from another page, whatever new tabs open in", async () => {
    localStorage.clear();
    const user = userEvent.setup();
    setup({ path: "/chats" });
    await user.click(screen.getByRole("link", { name: "New Chat" }));
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    expect(message()).toBeInTheDocument();
  });

  it("applies a New tab mode changed in Settings to the next fresh tab, not to an in-app visit", async () => {
    localStorage.clear();
    const user = userEvent.setup();
    setup();
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-selected", "true");
    cleanup();
    setup({ path: "/settings" });
    await user.click(screen.getByRole("button", { name: "New tabs open in Ask" }));
    cleanup();
    setup();
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    expect(message()).toHaveFocus();
    cleanup();
    setup({ path: "/settings" });
    await user.click(screen.getByRole("button", { name: "New tabs open in Search" }));
    await user.click(screen.getByRole("link", { name: "New Chat" }));
    // Arriving from inside the app is a new chat, so it stays Ask.
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    cleanup();
    setup();
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-selected", "true");
  });

  it("starts a new chat without a persona, then opens that chat", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    expect(screen.getByRole("img", { name: "openside" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "What should we do?" })).toBeInTheDocument();
    await user.type(message(), "Find a cheaper flight");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("session page")).toBeInTheDocument();
    const posts = calls.filter((call) => call.method === "POST");
    expect(posts.map((call) => call.path)).toEqual(["/v1/sessions", "/v1/sessions/s-new/messages"]);
    expect(posts[0]?.body).toHaveProperty("agent_id", null);
    expect(posts[1]?.body).toMatchObject({ text: "Find a cheaper flight" });
    expect(calls.some((call) => call.path.startsWith("/v1/agents"))).toBe(false);
  });

  it("keeps the words on Home when the new chat refuses the message, then sends to that same chat on retry", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ failedSubmits: 1 });
    await user.type(message(), "Find a cheaper flight");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("The daemon could not take the message")).toBeInTheDocument();
    expect(screen.queryByText("session page")).toBeNull();
    expect(message()).toHaveValue("Find a cheaper flight");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("session page")).toBeInTheDocument();
    const posts = calls.filter((call) => call.method === "POST").map((call) => call.path);
    expect(posts).toEqual(["/v1/sessions", "/v1/sessions/s-new/messages", "/v1/sessions/s-new/messages"]);
  });

  it("keeps the words and stays on Home when the chat cannot be created, then sends on retry", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ failedCreates: 1 });
    await user.type(message(), "Find a cheaper flight");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("The daemon could not start the chat")).toBeInTheDocument();
    expect(screen.queryByText("session page")).toBeNull();
    expect(message()).toHaveValue("Find a cheaper flight");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("session page")).toBeInTheDocument();
    const posts = calls.filter((call) => call.method === "POST").map((call) => call.path);
    expect(posts).toEqual(["/v1/sessions", "/v1/sessions", "/v1/sessions/s-new/messages"]);
  });

  it("hands Search to the browser's default engine, found by id in a Korean profile, and never creates a chat", async () => {
    localStorage.clear();
    vi.mocked(openAddress).mockClear();
    const user = userEvent.setup();
    const { calls } = setup({ prefs: NAVER_KO });
    await user.type(webSearch(), "날씨 & 서울{Enter}");
    const naver = { name: "네이버", url: "https://search.naver.com/search.naver?query={q}" };
    await waitFor(() => expect(openAddress).toHaveBeenLastCalledWith("날씨 & 서울", naver));
    expect(addressTarget("날씨 & 서울", naver)).toBe("https://search.naver.com/search.naver?query=%EB%82%A0%EC%94%A8%20%26%20%EC%84%9C%EC%9A%B8");
    expect(calls.some((call) => call.method === "POST")).toBe(false);
    expect(screen.queryByText("session page")).toBeNull();
  });

  it("waits for the browser's engine when Search is submitted before it is known", async () => {
    localStorage.clear();
    vi.mocked(openAddress).mockClear();
    const answer = Promise.withResolvers<Response>();
    const user = userEvent.setup();
    setup({ prefsAnswer: answer.promise });
    await user.type(webSearch(), "cats{Enter}");
    expect(openAddress).not.toHaveBeenCalled();
    answer.resolve(json(200, NAVER_KO));
    await waitFor(() => expect(openAddress).toHaveBeenCalledWith("cats", { name: "네이버", url: "https://search.naver.com/search.naver?query={q}" }));
  });

  it("has the account menu on the top-right avatar", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: /^Account:/ }));
    expect(await screen.findByRole("menu", { name: "Account" })).toBeInTheDocument();
  });

  it("marks the avatar while chats wait for the person", () => {
    setup({ needsYou: 2 });
    const avatar = screen.getByRole("button", { name: /^Account:.*, Needs you$/ });
    expect(avatar.querySelector(".home-avatar-dot")).not.toBeNull();
  });

  it("has no dot on the avatar when nothing waits", () => {
    setup();
    const avatar = screen.getByRole("button", { name: /^Account:/ });
    expect(avatar).not.toHaveAccessibleName(/Needs you/);
    expect(avatar.querySelector(".home-avatar-dot")).toBeNull();
  });

  it("ties the chosen mode to the panel it shows", async () => {
    const user = userEvent.setup();
    setup();
    const ask = screen.getByRole("tab", { name: "Ask" });
    const panel = screen.getByRole("tabpanel", { name: "Ask" });
    expect(ask).toHaveAttribute("aria-controls", panel.id);
    expect(within(panel).getByRole("textbox", { name: "Message" })).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Search" }));
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-controls", panel.id);
    expect(screen.getByRole("tab", { name: "Ask" })).not.toHaveAttribute("aria-controls");
    expect(within(screen.getByRole("tabpanel", { name: "Search" })).getByRole("searchbox")).toBeInTheDocument();
  });

  it("shows no Recent tasks section", () => {
    setup();
    expect(screen.queryByRole("region", { name: "Recent tasks" })).toBeNull();
  });
});

describe("HomePage suggested tasks", () => {
  it("fills the composer from a suggested task without sending it", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const section = screen.getByRole("region", { name: "Suggested tasks" });
    expect(within(section).getAllByRole("listitem")).toHaveLength(3);
    await user.click(within(section).getByRole("button", { name: /Automate my routines/ }));
    expect(message()).toHaveValue("Look through my recent chats, find tasks I keep doing by hand, and suggest which ones you could run for me on a schedule.");
    expect(calls.some((call) => call.method === "POST")).toBe(false);
  });

  it("switches a suggestion pick from Search back to Ask", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("tab", { name: "Search" }));
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull();
    await user.click(screen.getByRole("button", { name: /Set up openside’s memory/ }));
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    expect(message()).toHaveValue("Get to know how I work. Look at what I’ve shared with you and save what will help with future tasks to your memory.");
  });

  it("opens Settings from the Customize card", async () => {
    const user = userEvent.setup();
    setup();
    const card = screen.getByRole("link", { name: /Customize openside/ });
    expect(card).toHaveAttribute("href", "/settings");
    await user.click(card);
    expect(await screen.findByText("settings page")).toBeInTheDocument();
  });
});

describe("HomePage keyboard", () => {
  it("flips the mode with Tab from an empty field, and leaves Tab to the field once typed", async () => {
    const user = userEvent.setup();
    setup();
    expect(screen.getByText("to switch")).toBeInTheDocument();
    await user.click(message());
    await user.keyboard("{Tab}");
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-selected", "true");
    expect(webSearch()).toHaveFocus();
    await user.keyboard("{Tab}");
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    expect(message()).toHaveFocus();
    await user.click(screen.getByRole("tab", { name: "Search" }));
    await user.type(webSearch(), "cats");
    await user.keyboard("{Tab}");
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-selected", "true");
    expect(webSearch()).not.toHaveFocus();
  });

  it("moves focus back with Shift+Tab instead of switching", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(message());
    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveFocus();
  });

  it("leaves Tab alone while an input method is composing", () => {
    setup();
    fireEvent.keyDown(message(), { key: "Tab", isComposing: true });
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(message(), { key: "Tab" });
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-selected", "true");
  });

  it("keeps focus on the mode toggle while arrowing between modes", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("tab", { name: "Ask" }));
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "Search" })).toHaveAttribute("aria-selected", "true");
    expect(webSearch()).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Search" })).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Ask" })).toHaveFocus();
    expect(message()).toBeInTheDocument();
  });

  it("focuses the search field when Search is clicked", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("tab", { name: "Search" }));
    expect(webSearch()).toHaveFocus();
  });
});
