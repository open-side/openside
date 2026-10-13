import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Card, Ticket } from "./api/types";
import { App } from "./App";
import { fixture } from "./cards/testing";
import { agentSummary } from "./test/agents";
import { json } from "./test/fakeDaemon";

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
  window.history.pushState({}, "", "/");
});

/** The daemon's own card fixture, re-keyed to one ticket so several can coexist in a test. */
function cardFor(fixtureId: string, ticketId: string): Card {
  return { ...fixture(fixtureId), id: `ticket:${ticketId}`, ticket_id: ticketId };
}

function otpTicket(id: string, session: string): Ticket {
  return {
    id,
    session_id: session,
    tool_call_id: `call-${id}`,
    deferral: "external",
    kind: "secret_input",
    summary: "Code from the sign-in text",
    instructions: null,
    options: [],
    origin: { subagent_id: null },
    tool: null,
    target: { origin: "https://shop.example", tab_id: "t1", field_ref: "e4" },
    resume_when: null,
    created_at: "2026-10-12T00:00:00Z",
    expires_at: "2026-10-12T01:00:00Z",
    status: "pending",
    resolution: null,
    otp: { channel: "sms", can_register: false },
    card: cardFor("ticket:otp1", id),
  };
}

function loginTicket(id: string, session: string): Ticket {
  return {
    ...otpTicket(id, session),
    kind: "login",
    otp: null,
    target: { origin: "https://accounts.example", tab_id: "t1", field_ref: null },
    card: cardFor("ticket:login1", id),
  };
}

function choiceTicket(id: string, session: string): Ticket {
  return {
    ...otpTicket(id, session),
    kind: "choice",
    otp: null,
    card: cardFor("ticket:choice1", id),
  };
}

/** The collapsed bubble's label, which is also its accessible name. */
const summary = (fixtureId: string) => new RegExp(fixture(fixtureId).collapsed_summary);

/** An event stream that replays `events` and stays open, as the daemon's does. */
function stream(session: string, events: Record<string, unknown>[]): Response {
  const encoder = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      events.forEach((event, index) => {
        const full = { session_id: session, seq: index + 1, ts: "2026-10-12T00:00:00Z", run_id: null, ...event };
        controller.enqueue(encoder.encode(`id: ${index + 1}\ndata: ${JSON.stringify(full)}\n\n`));
      });
    },
  });
  return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
}

function stubDaemon(options: { events?: Record<string, unknown>[]; pending?: Ticket[] }) {
  const resolves: { path: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://daemon.test");
      const method = init?.method ?? "GET";
      if (method === "POST" && url.pathname.endsWith("/resolve")) {
        resolves.push({ path: url.pathname, body: JSON.parse(String(init?.body)) });
        return json(200, { ...otpTicket("k1", "s1"), status: "resolved", resolution: { status: "filled" } });
      }
      if (url.pathname === "/v1/sessions/s1/events") return stream("s1", options.events ?? []);
      if (url.pathname.endsWith("/events")) return stream(url.pathname.split("/")[3] ?? "", []);
      if (url.pathname === "/v1/tickets") return json(200, { tickets: options.pending ?? [] });
      if (url.pathname === "/v1/sessions") return json(200, { sessions: [] });
      if (url.pathname === "/v1/agents") return json(200, { agents: [agentSummary("chief", "openside", { home_session_id: "s-chief" })] });
      return json(404, { code: "not_found", message: "not found" });
    }),
  );
  return resolves;
}

describe("code requests never move the person", () => {
  it("an OTP request in the open conversation expands into a focused form in place; the route does not change", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1");
    const before = window.history.length;
    const resolves = stubDaemon({ events: [{ kind: "ticket.opened", ticket: otpTicket("k1", "s1") }], pending: [otpTicket("k1", "s1")] });
    render(<App />);
    await user.click(await screen.findByRole("button", { name: summary("ticket:otp1") }));
    const code = await screen.findByLabelText("Code");
    // The dock has polled and still leaves the visible conversation's request to its inline card.
    await vi.waitFor(() => expect(vi.mocked(fetch).mock.calls.some(([input]) => String(input).includes("/v1/tickets?status=pending"))).toBe(true));
    expect(code).toHaveFocus();
    expect(screen.getAllByLabelText("Code")).toHaveLength(1);
    expect(screen.queryByRole("complementary", { name: /needed for/ })).toBeNull();
    await user.type(code, "482913{Enter}");
    await vi.waitFor(() => expect(resolves).toEqual([{ path: "/v1/tickets/k1/resolve", body: { value: "482913" } }]));
    expect(window.location.pathname + window.location.search).toBe("/sessions/s1");
    expect(window.history.length).toBe(before);
  });

  it("over Settings, another conversation's code request floats in a dock and the route stays", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/settings");
    const resolves = stubDaemon({ pending: [otpTicket("k2", "s2")] });
    render(<App />);
    const dock = await screen.findByRole("complementary", { name: "Code needed for accounts.example" });
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();
    expect(within(dock).getByRole("link", { name: /^Go to / })).toHaveAttribute("href", "/sessions/s2");
    // The dock rests as the card's collapsed bubble; opening it puts the cursor in the code field.
    expect(within(dock).queryByLabelText("Code")).toBeNull();
    await user.click(await within(dock).findByRole("button", { name: summary("ticket:otp1") }));
    expect(within(dock).getByLabelText("Code")).toHaveFocus();
    await user.type(within(dock).getByLabelText("Code"), "123456{Enter}");
    await vi.waitFor(() => expect(resolves).toEqual([{ path: "/v1/tickets/k2/resolve", body: { value: "123456" } }]));
    await vi.waitFor(() => expect(screen.queryByRole("complementary", { name: "Code needed for accounts.example" })).toBeNull());
    expect(window.location.pathname).toBe("/settings");
  });

  it("a tapped code notification (?ticket=) opens the code sheet, and submitting returns to the conversation", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1?ticket=k1");
    const resolves = stubDaemon({ events: [{ kind: "ticket.opened", ticket: otpTicket("k1", "s1") }] });
    render(<App />);
    const sheet = await screen.findByRole("dialog", { name: "Verification code" });
    const code = within(sheet).getByLabelText("Code");
    expect(code).toHaveAttribute("autocomplete", "one-time-code");
    await vi.waitFor(() => expect(code).toHaveFocus());
    await user.type(code, "777111{Enter}");
    await vi.waitFor(() => expect(resolves).toEqual([{ path: "/v1/tickets/k1/resolve", body: { value: "777111" } }]));
    await vi.waitFor(() => expect(screen.queryByRole("dialog", { name: "Verification code" })).toBeNull());
    expect(window.location.pathname + window.location.search).toBe("/sessions/s1");
  });

  it("?ticket= for a request that is no code request is consumed and leaves the conversation as is", async () => {
    // jsdom has no layout; the conversation scrolls the linked card into view.
    const scrolled = vi.fn();
    Element.prototype.scrollIntoView = scrolled;
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1?ticket=k9");
    const info: Ticket = { ...otpTicket("k9", "s1"), kind: "info", otp: null, target: null, summary: "Which colour?", card: cardFor("ticket:form1", "k9") };
    stubDaemon({ events: [{ kind: "ticket.opened", ticket: info }] });
    render(<App />);
    expect(await screen.findByRole("button", { name: summary("ticket:form1") })).toBeInTheDocument();
    await vi.waitFor(() => expect(window.location.search).toBe(""));
    expect(window.location.pathname).toBe("/sessions/s1");
    expect(scrolled).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a login request in the open conversation expands into the login card alone; the route does not change", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1");
    const before = window.history.length;
    const login = loginTicket("k3", "s1");
    const resolves = stubDaemon({ events: [{ kind: "ticket.opened", ticket: login }], pending: [login] });
    render(<App />);
    await user.click(await screen.findByRole("button", { name: summary("ticket:login1") }));
    const card = await screen.findByRole("group", { name: "Log in" });
    expect(screen.queryByText("Action needed")).toBeNull();
    await user.type(within(card).getByLabelText("Email"), "jane@example.test");
    await user.type(within(card).getByLabelText("Password"), "secret-pw{Enter}");
    await vi.waitFor(() => expect(resolves).toEqual([{ path: "/v1/tickets/k3/resolve", body: { value: { values: { user: "jane@example.test", pass: "secret-pw" }, save: false } } }]));
    expect(document.body.innerHTML).not.toContain("secret-pw");
    expect(window.location.pathname + window.location.search).toBe("/sessions/s1");
    expect(window.history.length).toBe(before);
  });

  it("another conversation's request rests as one collapsed bubble over the assistant's thread", async () => {
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/agents/chief");
    stubDaemon({ pending: [otpTicket("k5", "s2")] });
    render(<App />);
    const dock = await screen.findByRole("complementary", { name: "Code needed for accounts.example" });
    expect(await screen.findByRole("textbox", { name: "Message" })).toBeInTheDocument();
    // Folded: the form is not on screen until the person asks for it.
    expect(await within(dock).findByRole("button", { name: summary("ticket:otp1") })).toHaveAttribute("aria-expanded", "false");
    expect(within(dock).queryByLabelText("Code")).toBeNull();
    // Portaled out of the page, so it can never be laid over the conversation's own markup.
    expect(document.querySelector("main")?.contains(dock)).toBe(false);
  });

  it("in a conversation, only the other conversations' requests dock; its own stays inline", async () => {
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1");
    stubDaemon({ events: [{ kind: "ticket.opened", ticket: otpTicket("k1", "s1") }], pending: [otpTicket("k1", "s1"), otpTicket("k6", "s2")] });
    render(<App />);
    const dock = await screen.findByRole("complementary", { name: "Code needed for accounts.example" });
    expect(within(dock).getByRole("link", { name: /^Go to / })).toHaveAttribute("href", "/sessions/s2");
    await vi.waitFor(() => expect(document.querySelectorAll('main .oc-card[data-ticket="k1"]')).toHaveLength(1));
    expect(document.querySelectorAll('.oc-card[data-ticket="k1"]')).toHaveLength(1);
    expect(document.querySelectorAll('.oc-card[data-ticket="k6"]')).toHaveLength(1);
    expect(screen.queryByText(/more waiting/)).toBeNull();
  });

  it("over Settings, another conversation's choice floats in the dock and the route stays", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/settings");
    const resolves = stubDaemon({ pending: [choiceTicket("k4", "s2")] });
    render(<App />);
    const dock = await screen.findByRole("complementary", { name: "Choice needed for accounts.example" });
    await user.click(await within(dock).findByRole("button", { name: summary("ticket:choice1") }));
    expect(within(dock).getAllByRole("radio")).toHaveLength(2);
    await user.click(within(dock).getByLabelText(/Work account/));
    await user.click(within(dock).getByRole("button", { name: "Continue" }));
    await vi.waitFor(() => expect(resolves).toEqual([{ path: "/v1/tickets/k4/resolve", body: { value: { option: "page-account-2" } } }]));
    expect(window.location.pathname).toBe("/settings");
  });

  it("a tapped login notification (?ticket=) opens the login sheet", async () => {
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1?ticket=k3");
    stubDaemon({ events: [{ kind: "ticket.opened", ticket: loginTicket("k3", "s1") }] });
    render(<App />);
    const sheet = await screen.findByRole("dialog", { name: "Log in" });
    await vi.waitFor(() => expect(within(sheet).getByLabelText("Email")).toHaveFocus());
  });
});
