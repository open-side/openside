import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Ticket } from "./api/types";
import { App } from "./App";
import { json } from "./test/fakeDaemon";

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
  window.history.pushState({}, "", "/");
});

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
  };
}

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
      if (url.pathname === "/v1/tickets") return json(200, { tickets: options.pending ?? [] });
      if (url.pathname === "/v1/sessions") return json(200, { sessions: [] });
      return json(404, { code: "not_found", message: "not found" });
    }),
  );
  return resolves;
}

describe("code requests never move the person", () => {
  it("an OTP ticket in the open conversation becomes a focused form in place; the route does not change", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1");
    const before = window.history.length;
    const resolves = stubDaemon({ events: [{ kind: "ticket.opened", ticket: otpTicket("k1", "s1") }], pending: [otpTicket("k1", "s1")] });
    render(<App />);
    const code = await screen.findByLabelText("Code");
    // The dock has polled and still leaves the visible conversation's ticket to its inline card.
    await vi.waitFor(() => expect(vi.mocked(fetch).mock.calls.some(([input]) => String(input).includes("/v1/tickets?status=pending"))).toBe(true));
    expect(code).toHaveFocus();
    expect(screen.getAllByLabelText("Code")).toHaveLength(1);
    await user.type(code, "482913{Enter}");
    expect(resolves).toEqual([{ path: "/v1/tickets/k1/resolve", body: { value: "482913" } }]);
    expect(window.location.pathname + window.location.search).toBe("/sessions/s1");
    expect(window.history.length).toBe(before);
  });

  it("over Settings, another conversation's code request floats in a dock and the route stays", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/settings");
    const resolves = stubDaemon({ pending: [otpTicket("k2", "s2")] });
    render(<App />);
    const dock = await screen.findByRole("complementary", { name: "Code needed" });
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();
    await user.type(within(dock).getByLabelText("Code"), "123456{Enter}");
    expect(resolves).toEqual([{ path: "/v1/tickets/k2/resolve", body: { value: "123456" } }]);
    await vi.waitFor(() => expect(screen.queryByRole("complementary", { name: "Code needed" })).toBeNull());
    expect(window.location.pathname).toBe("/settings");
  });

  it("a tapped code notification (?ticket=) opens the code sheet, and submitting returns to the conversation", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1?ticket=k1");
    const resolves = stubDaemon({ events: [{ kind: "ticket.opened", ticket: otpTicket("k1", "s1") }] });
    render(<App />);
    const sheet = await screen.findByRole("dialog", { name: "Enter the code" });
    const code = within(sheet).getByLabelText("Code");
    expect(code).toHaveAttribute("autocomplete", "one-time-code");
    await vi.waitFor(() => expect(code).toHaveFocus());
    await user.type(code, "777111{Enter}");
    expect(resolves).toEqual([{ path: "/v1/tickets/k1/resolve", body: { value: "777111" } }]);
    await vi.waitFor(() => expect(screen.queryByRole("dialog", { name: "Enter the code" })).toBeNull());
    expect(window.location.pathname + window.location.search).toBe("/sessions/s1");
  });

  it("?ticket= for a ticket that is no code request is consumed and leaves the conversation as is", async () => {
    // jsdom has no layout; the conversation scrolls the linked card into view.
    const scrolled = vi.fn();
    Element.prototype.scrollIntoView = scrolled;
    localStorage.setItem("openside.token", "tok");
    window.history.pushState({}, "", "/sessions/s1?ticket=k9");
    const info = { ...otpTicket("k9", "s1"), kind: "info", otp: null, target: null, summary: "Which colour?" } satisfies Ticket;
    stubDaemon({ events: [{ kind: "ticket.opened", ticket: info }] });
    render(<App />);
    expect(await screen.findByText("Which colour?")).toBeInTheDocument();
    await vi.waitFor(() => expect(window.location.search).toBe(""));
    expect(window.location.pathname).toBe("/sessions/s1");
    expect(scrolled).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
