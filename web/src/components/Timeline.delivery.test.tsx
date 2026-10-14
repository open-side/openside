import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { Event } from "../api/types";
import { ClientContext } from "../context";
import { outgoing, useOutgoing } from "../state/outgoing";
import { replaySession } from "../state/session";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { Timeline } from "./Timeline";

afterEach(() => act(() => outgoing.snapshot().forEach((entry) => outgoing.forget(entry.message_id))));

function log(payloads: Record<string, unknown>[]): Event[] {
  return payloads.map((p, i) => ({ session_id: "s1", seq: i + 1, ts: "2026-01-01T00:00:00Z", run_id: null, ...p })) as Event[];
}

const queued = { kind: "message.queued", message_id: "m1", text: "Hi there", mode: "queue", author_kind: "person", from_agent: null };
const injected = { kind: "message.injected", message_id: "m1" };
const started = { kind: "turn.started" };

function Harness({ events, client }: { events: Event[]; client: ReturnType<typeof fakeClient>["client"] }) {
  const sending = useOutgoing("s1");
  return (
    <ClientContext.Provider value={client}>
      <Timeline view={replaySession(events)} sessionId="s1" outgoing={sending} />
    </ClientContext.Provider>
  );
}

function mount(payloads: Record<string, unknown>[], handler: (call: Call) => Response = () => json(202, {})) {
  const daemon = fakeClient(handler);
  const view = render(<Harness events={log(payloads)} client={daemon.client} />);
  return { ...daemon, rerender: (more: Record<string, unknown>[]) => view.rerender(<Harness events={log(more)} client={daemon.client} />) };
}

describe("delivery states in the transcript", () => {
  it("keeps a person-controlled report prefix as a person message and hides internal delivering receipts", () => {
    const person = { ...queued, message_id: "task-report-person" };
    const view = mount([person]);
    expect(screen.getByText("Hi there")).toBeInTheDocument();
    expect(screen.getByText("Sent")).toBeInTheDocument();
    view.rerender([{ ...queued, message_id: "plain-internal", text: "Team delivery", author_kind: "internal", from_agent: "chief" }]);
    expect(screen.queryByText("Sent")).toBeNull();
    expect(screen.queryByText("Team delivery")).toBeNull();
  });

  it("never renders injected internal browser notices as person bubbles", () => {
    mount([
      { ...queued, text: "[browser event] Tab closed", author_kind: "internal" },
      injected,
      started,
    ]);
    expect(screen.queryByText("[browser event] Tab closed")).toBeNull();
    expect(screen.queryByText("Read")).toBeNull();
  });

  it("shows Sent, then Read and typing dots until a completed reply arrives", () => {
    const sent = mount([queued]);
    expect(screen.getByText("Hi there")).toBeInTheDocument();
    expect(screen.getByText("Sent")).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: /working/i })).toBeNull();
    expect(document.querySelector(".typing")).toBeNull();

    sent.rerender([queued, injected]);
    expect(screen.getByText("Read")).toBeInTheDocument();
    expect(screen.queryByText("Sent")).toBeNull();
    expect(document.querySelector(".typing")).toBeNull();

    sent.rerender([queued, injected, started]);
    expect(screen.getByText("Read")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Agent is working");
    expect(document.querySelectorAll(".typing-dot")).toHaveLength(3);

    sent.rerender([queued, injected, started, { kind: "assistant.delta", text: "Hello!" }]);
    expect(document.querySelectorAll(".typing-dot")).toHaveLength(3);
    // As in a messenger, the newest message keeps its receipt after the answer.
    expect(screen.getByText("Read")).toBeInTheDocument();
    expect(screen.queryByText("Hello!")).toBeNull();
    sent.rerender([queued, injected, started, { kind: "assistant.final", text: "Hello!" }, { kind: "session.settled" }]);
    expect(document.querySelector(".typing")).toBeNull();
    expect(screen.getByText("Hello!")).toBeInTheDocument();
  });

  it("keeps one delivery note in the thread, under the person's newest bubble", () => {
    mount([queued, injected, { kind: "assistant.final", text: "Hello!" }, { kind: "session.settled" }, { ...queued, message_id: "m2", text: "And another" }]);
    const receipts = [...document.querySelectorAll(".receipt")];
    expect(receipts.map((node) => node.textContent)).toEqual(["Sent"]);
    expect(receipts[0]!.closest("li")).toHaveAttribute("data-message", "m2");
  });

  it("puts a Task started before the turn's words after them", () => {
    mount([
      queued,
      injected,
      started,
      { kind: "tool.started", tool_call_id: "c1", name: "create_task", args: { goal: "Check the weather" } },
      { kind: "tool.ended", tool_call_id: "c1", result: { task_id: "t1" }, is_error: false },
      { kind: "subagent.started", subagent_id: "t1", background: true, model: "m", parent_tool_call_id: "c1", task: "Check the weather", tools: [] },
      { kind: "assistant.final", text: "On it." },
      { kind: "turn.ended" },
    ]);
    const rows = [...document.querySelectorAll(".timeline > li")].map((row) => row.className.split(" ")[0]);
    expect(rows.slice(rows.indexOf("bubble"))).toEqual(["bubble", "bubble", "task-row"]);
    expect(document.querySelector(".bubble.assistant")!.nextElementSibling).toHaveClass("task-row");
  });

  it("does not defer a running Task to hidden provisional narration", () => {
    mount([
      queued,
      injected,
      started,
      { kind: "tool.started", tool_call_id: "c1", name: "create_task", args: { goal: "Check the weather" } },
      { kind: "tool.ended", tool_call_id: "c1", result: { task_id: "t1" }, is_error: false },
      { kind: "subagent.started", subagent_id: "t1", background: true, model: "m", parent_tool_call_id: "c1", task: "Check the weather", tools: [] },
      { kind: "assistant.delta", text: "Internal routing narration" },
    ]);
    expect(screen.queryByText("Internal routing narration")).toBeNull();
    expect(document.querySelector(".task-row")).not.toBeNull();
  });

  it("leaves a message sent during a run to the Queued list, not a Sent bubble", () => {
    mount([queued, injected, started, { kind: "message.queued", message_id: "m2", text: "and then", mode: "queue", author_kind: "person", from_agent: null }]);
    expect(screen.queryByText("and then")).toBeNull();
    expect(screen.queryByText("Sent")).toBeNull();
  });

  it("shows a message that never reached the daemon as Not sent and retries under the same id", async () => {
    const user = userEvent.setup();
    let fail = true;
    const daemon = mount([], () => (fail ? json(500, { code: "internal", message: "boom" }) : json(202, {})));
    await act(async () => {
      await outgoing.submit(daemon.client, { message_id: "o1", session_id: "s1", text: "draft", attachments: [] });
    });
    expect(await screen.findByText(/Not sent/)).toBeInTheDocument();
    fail = false;
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Sent")).toBeInTheDocument();
    expect(daemon.calls.map((call) => (call.body as { message_id: string }).message_id)).toEqual(["o1", "o1"]);
  });

  it("lets the event log take over a device-side message once it is known", async () => {
    const daemon = mount([], () => json(202, {}));
    await act(async () => {
      await outgoing.submit(daemon.client, { message_id: "m1", session_id: "s1", text: "Hi there", attachments: [] });
    });
    expect(await screen.findByText("Sent")).toBeInTheDocument();
    daemon.rerender([queued]);
    expect(screen.getAllByText("Hi there")).toHaveLength(1);
  });

  it("marks a run that died on the newest message as Not finished, with Retry", async () => {
    const user = userEvent.setup();
    const daemon = mount([queued, injected, { kind: "message.completed", message_id: "m1", outcome: "failed", reason: "interrupted" }, { kind: "session.settled" }]);
    expect(screen.getByText("Not finished")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(daemon.calls[0]).toMatchObject({ method: "POST", path: "/v1/sessions/s1/messages", body: { text: "Hi there", mode: "queue" } }));
    expect((daemon.calls[0]?.body as { message_id: string }).message_id).not.toBe("m1");
    expect(await screen.findByText("Sent")).toBeInTheDocument();
  });
});

describe("failed run next to the agent's own error row", () => {
  it("leaves the retry to the error row", () => {
    mount([
      queued,
      injected,
      { kind: "message.completed", message_id: "m1", outcome: "failed", reason: "interrupted" },
      { kind: "runtime.error", code: "interrupted", message: "Agent process interrupted" },
      { kind: "session.settled" },
    ]);
    expect(screen.getByText("Not finished")).toBeInTheDocument();
    expect(document.querySelector(".bubble.user .receipt-action")).toBeNull();
  });
});

describe("typing dots styling", () => {
  // vitest runs from web/; the stylesheet is the shipped source, not a processed copy.
  const css = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");

  it("bounces vertically by default", () => {
    expect(css).toMatch(/@keyframes typing-bounce[^}]*\{[\s\S]*?translateY\(-5px\)/);
  });

  it("swaps the bounce for a movement-free fade under reduced motion", () => {
    const block = /@media \(prefers-reduced-motion: reduce\) \{\s*\.typing-dot \{[^}]*\}/.exec(css)?.[0] ?? "";
    expect(block).toContain("typing-pulse");
    const pulse = /@keyframes typing-pulse \{[\s\S]*?\n\}/.exec(css)?.[0] ?? "";
    expect(pulse).toContain("opacity");
    expect(pulse).not.toContain("transform");
    // The global reduced-motion rule must not cut the pulse after one iteration.
    expect(css).toMatch(/\*:not\([^)]*\.typing-dot\)/);
  });
});
