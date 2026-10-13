import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { Card, Event, Ticket } from "../api/types";
import { fixture } from "../cards/testing";
import { ClientContext } from "../context";
import { outgoing } from "../state/outgoing";
import { replaySession } from "../state/session";
import { fakeClient, json } from "../test/fakeDaemon";
import { Timeline } from "./Timeline";

afterEach(() => outgoing.snapshot().forEach((entry) => outgoing.forget(entry.message_id)));

function log(payloads: Record<string, unknown>[]): Event[] {
  return payloads.map((p, i) => ({ session_id: "s1", seq: i + 1, ts: "2026-01-01T00:00:00Z", run_id: null, ...p })) as Event[];
}

function ticket(overrides: Partial<Ticket>): Ticket {
  return {
    id: "k1",
    session_id: "s1",
    tool_call_id: "call-1",
    deferral: "approval",
    kind: "confirm",
    summary: "Can I save release-report.md to ~/reports? That folder is outside this workspace.",
    instructions: null,
    options: [],
    origin: { subagent_id: null },
    tool: { name: "write", args: { path: "/home/me/reports/release-report.md" } },
    target: null,
    resume_when: null,
    created_at: "2026-01-01T00:00:00Z",
    expires_at: "2026-01-02T00:00:00Z",
    status: "pending",
    resolution: null,
    ...overrides,
  };
}

const asked = [
  { kind: "message.queued", message_id: "m1", text: "Save the report", mode: "queue", author_kind: "person", from_agent: null },
  { kind: "message.injected", message_id: "m1" },
  { kind: "turn.started" },
  { kind: "tool.started", tool_call_id: "call-1", name: "write", args: { path: "/home/me/reports/release-report.md" } },
];

function mount(payloads: Record<string, unknown>[], handler = () => json(202, {})) {
  const daemon = fakeClient(handler);
  render(
    <ClientContext.Provider value={daemon.client}>
      <Timeline view={replaySession(log(payloads))} sessionId="s1" />
    </ClientContext.Provider>,
  );
  return daemon;
}

describe("agent requests in the thread", () => {
  const approval = () => ticket({ card: { ...fixture("ticket:confirm1"), id: "ticket:k1", ticket_id: "k1" } });

  it("shows a pending approval once as a collapsed bubble, with no chip, status row or tool row for it", () => {
    mount([...asked, { kind: "ticket.opened", ticket: approval() }]);
    expect(document.querySelectorAll('.oc-card[data-ask="pending"]')).toHaveLength(1);
    expect(screen.getByRole("button", { name: /Approve/ })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Action needed")).toBeNull();
    expect(document.querySelector(".activity-row")).toBeNull();
    expect(document.querySelectorAll(".oc-btn")).toHaveLength(0);
  });

  it("opens to the question with one primary action, and a click answers through the ticket", async () => {
    const user = userEvent.setup();
    const daemon = mount([...asked, { kind: "ticket.opened", ticket: approval() }], () => json(200, {}));
    await user.click(document.querySelector<HTMLButtonElement>("button.oc-pill")!);
    expect(document.querySelectorAll(".oc-btn--primary")).toHaveLength(1);
    await user.click(document.querySelector<HTMLButtonElement>(".oc-btn--primary")!);
    expect(daemon.calls.find((call) => call.method === "POST")).toMatchObject({ path: "/v1/tickets/k1/resolve", body: { value: true } });
  });

  it("folds a settled request into a receipt that keeps its place and holds no control", () => {
    const settled = { ...approval().card!, status: "done" as const, collapsed_summary: "You approved", actions: [] };
    mount([
      ...asked,
      { kind: "ticket.opened", ticket: approval() },
      { kind: "ticket.resolved", ticket_id: "k1", resolution: { approved: true }, card: settled },
      { kind: "tool.ended", tool_call_id: "call-1", result: { ok: true }, is_error: false },
    ]);
    const receipt = document.querySelector<HTMLElement>('.oc-card[data-status="done"]')!;
    expect(within(receipt).getByText("You approved")).toBeInTheDocument();
    expect(receipt.querySelector("input, .oc-btn")).toBeNull();
    expect(document.querySelector('.oc-card[data-ask="pending"]')).toBeNull();
    // The card is the record of the call it let run; no second tool row for it.
    expect(document.querySelector(".activity-row")).toBeNull();
  });

  it("records a declined request and an answered question as receipts", () => {
    const declined = { ...approval().card!, status: "failed" as const, collapsed_summary: "You declined", actions: [] };
    const answered = { ...approval().card!, id: "ticket:k2", ticket_id: "k2", status: "done" as const, collapsed_summary: "You answered: Markdown", actions: [] };
    mount([
      ...asked,
      { kind: "ticket.opened", ticket: approval() },
      { kind: "ticket.rejected", ticket_id: "k1", reason: "user", card: declined },
      { kind: "ticket.opened", ticket: ticket({ id: "k2", tool_call_id: "call-2", card: { ...answered, status: "pending", actions: approval().card!.actions } }) },
      { kind: "ticket.resolved", ticket_id: "k2", resolution: { answer: "md" }, card: answered },
    ]);
    const receipts = [...document.querySelectorAll('.oc-card:not([data-ask="pending"]) .oc-pill-text')].map((node) => node.textContent);
    expect(receipts).toEqual(["You declined", "You answered: Markdown"]);
  });

  it("asks a choice with radio options and sends the picked one", async () => {
    const user = userEvent.setup();
    const choice: Card = {
      ...approval().card!,
      kind: "form",
      title: "Which format?",
      collapsed_summary: "Which format?",
      blocks: [{ type: "choice_options", id: "answers", label: "Which format?", layout: "stack", required: true, options: [{ id: "pdf", label: "PDF", description: null, provider: null, action: null }, { id: "md", label: "Markdown", description: null, provider: null, action: null }] }],
      actions: [{ id: "send", label: "Send", style: "primary", target: { type: "ticket_resolve", answer: { type: "selected_choice", block_id: "answers", encoding: "info_answer" } } }],
    };
    const daemon = mount([...asked, { kind: "ticket.opened", ticket: ticket({ card: choice }) }], () => json(200, {}));
    await user.click(document.querySelector<HTMLButtonElement>("button.oc-pill")!);
    expect(screen.getAllByRole("radio")).toHaveLength(2);
    await user.click(screen.getByLabelText("Markdown"));
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(daemon.calls.find((call) => call.method === "POST")).toMatchObject({ path: "/v1/tickets/k1/resolve", body: { value: "md" } });
  });

  it("says which helper asked: its task title in the header, a short id when the task is unknown", async () => {
    const user = userEvent.setup();
    const helper = ticket({ origin: { subagent_id: "sub-12345678" }, card: { ...fixture("ticket:confirm1"), id: "ticket:k1", ticket_id: "k1" } });
    mount([
      ...asked,
      { kind: "subagent.started", subagent_id: "sub-12345678", task: "Compare prices", model: "m", tools: [], parent_tool_call_id: "call-9" },
      { kind: "ticket.opened", ticket: helper },
    ]);
    expect(document.querySelector(".oc-pill .oc-asker")).toHaveTextContent("Compare prices asked");
    await user.click(document.querySelector<HTMLButtonElement>("button.oc-pill")!);
    expect(document.querySelector(".oc-head .oc-asker")).toHaveAttribute("data-subagent-id", "sub-12345678");
  });

  it("falls back to a short id for a helper this thread has not seen", () => {
    mount([...asked, { kind: "ticket.opened", ticket: ticket({ origin: { subagent_id: "sub-12345678" }, card: { ...fixture("ticket:confirm1"), id: "ticket:k1", ticket_id: "k1" } }) }]);
    expect(document.querySelector(".oc-pill .oc-asker")).toHaveTextContent("Subagent sub-1234 asked");
  });
});

describe("a failed turn", () => {
  const failed = [
    { kind: "message.queued", message_id: "m1", text: "Check my invoices", mode: "queue", author_kind: "person", from_agent: null },
    { kind: "message.injected", message_id: "m1" },
    { kind: "turn.started" },
    { kind: "runtime.error", code: "interrupted", message: "Agent process interrupted; dispatched messages will not be resent.", user_message: "I was interrupted before I finished.", retryable: true },
    { kind: "message.completed", message_id: "m1", outcome: "interrupted", reason: "interrupted" },
    { kind: "session.settled" },
  ];

  it("speaks calmly, keeps the daemon's words behind Technical details, and retries with the same message", async () => {
    const user = userEvent.setup();
    const daemon = mount(failed);
    expect(screen.getByText("I was interrupted before I finished.")).toBeInTheDocument();
    expect(screen.getByText(/dispatched messages will not be resent/)).not.toBeVisible();
    await user.click(screen.getByText("Technical details"));
    expect(screen.getByText(/dispatched messages will not be resent/)).toBeVisible();

    await user.click(screen.getAllByRole("button", { name: /Retry/ }).find((button) => button.closest(".runtime-error"))!);
    const post = daemon.calls.find((call) => call.method === "POST");
    expect(post?.path).toBe("/v1/sessions/s1/messages");
    expect(post?.body).toMatchObject({ text: "Check my invoices", mode: "queue" });
    expect((post?.body as { message_id: string }).message_id).not.toBe("m1");
  });

  it("offers no retry once the person has written again, or when the daemon says it cannot help", () => {
    mount([...failed, { kind: "message.queued", message_id: "m2", text: "Never mind", mode: "queue", author_kind: "person", from_agent: null }, { kind: "message.injected", message_id: "m2" }]);
    expect(document.querySelector(".runtime-error button")).toBeNull();
  });

  it("falls back to plain copy for events without a message of their own", () => {
    mount([
      { kind: "message.queued", message_id: "m1", text: "Hi", mode: "queue", author_kind: "person", from_agent: null },
      { kind: "runtime.error", code: "some_new_code", message: "upstream said no" },
    ]);
    expect(screen.getByText("Something went wrong on my side.")).toBeInTheDocument();
    expect(document.querySelector(".runtime-error button")).toBeNull();
  });
});
