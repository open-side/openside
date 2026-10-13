import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Event, Visual } from "../api/types";
import { ClientContext } from "../context";
import { replaySession, type SessionView } from "../state/session";
import { fakeClient } from "../test/fakeDaemon";
import { Timeline } from "./Timeline";

const SOURCE = '<svg xmlns="http://www.w3.org/2000/svg"><rect width="10" height="10"/></svg>';
const TEXT = `Here is the chart:\n\`\`\`svg visual\n${SOURCE}\n\`\`\`\nDone.`;
const visual: Visual = {
  id: "3-0",
  seq: 3,
  kind: "svg",
  path: "visuals/3-0.svg",
  source: SOURCE,
  preview_url: "/v1/sessions/s1/artifacts/visuals%2F3-0.svg/preview?exp=1&sig=abc",
};

function events(payloads: Record<string, unknown>[]): Event[] {
  return payloads.map((p, i) => ({ session_id: "s1", seq: i + 1, ts: "2026-01-01T00:00:00Z", run_id: null, ...p })) as Event[];
}

function setup(view: SessionView, visuals: Record<number, Visual[]> = {}) {
  const daemon = fakeClient(() => new Response(new Blob([new Uint8Array([1, 2, 3])], { type: "image/png" }), { status: 200 }));
  render(
    <ClientContext.Provider value={daemon.client}>
      <Timeline view={view} sessionId="s1" visuals={visuals} />
    </ClientContext.Provider>,
  );
  return daemon;
}

describe("Timeline visuals (A-15)", () => {
  const view = replaySession(
    events([
      { kind: "turn.started" },
      { kind: "assistant.delta", text: "Here" },
      { kind: "assistant.final", text: TEXT },
    ]),
  );

  it("renders a visual block in a script-free sandboxed iframe and toggles to its source", async () => {
    const user = userEvent.setup();
    setup(view, { 3: [visual] });
    const bubble = document.querySelector(".bubble.assistant")!;
    expect(bubble.textContent).toBe("Here is the chart:\nDone.");
    const frame = screen.getByTitle("SVG visual");
    expect(frame.tagName).toBe("IFRAME");
    // Empty sandbox: no scripts, opaque origin, no top navigation.
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(frame).toHaveAttribute("src", `http://localhost:3000${visual.preview_url}`);
    await user.click(screen.getByRole("button", { name: "Source" }));
    expect(screen.queryByTitle("SVG visual")).toBeNull();
    expect(document.querySelector(".visual-source")?.textContent).toBe(SOURCE);
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(screen.getByTitle("SVG visual")).toBeInTheDocument();
  });

  it("keeps the visual block as a code block when the daemon lists no visual for that message", async () => {
    setup(view);
    const bubble = document.querySelector(".bubble.assistant")!;
    await waitFor(() => expect(bubble.querySelector(".code-block pre")?.textContent).toBe(`${SOURCE}\n`));
    expect(bubble.querySelector("p")?.textContent).toBe("Here is the chart:");
    expect(bubble.querySelector(".code-block-head")).toHaveTextContent("svg");
    expect(bubble.textContent).toContain("Done.");
    expect(document.querySelector("iframe")).toBeNull();
  });
});

describe("Timeline attachments (PP-14)", () => {
  it("shows an attached image from the authenticated artifact route and zooms it", async () => {
    const user = userEvent.setup();
    const view = replaySession(
      events([
        {
          kind: "message.queued",
          message_id: "m1",
          text: "what is this?",
          mode: "steer",
          author_kind: "person",
          from_agent: null,
          attachments: [
            { path: "attachments/m1/cat.png", kind: "image", mime: "image/png", name: "cat.png", size: 3, sha256: "0" },
            { path: "attachments/m1/doc.pdf", kind: "file", mime: "application/pdf", name: "doc.pdf", size: 3, sha256: "1" },
          ],
        },
        { kind: "message.injected", message_id: "m1" },
      ]),
    );
    const { calls } = setup(view);
    const open = await screen.findByRole("button", { name: "Open image cat.png" });
    expect(calls[0]).toMatchObject({ method: "GET", path: "/v1/sessions/s1/artifacts/attachments%2Fm1%2Fcat.png" });
    expect(calls[0]?.headers.Authorization).toBe("Bearer test-token");
    expect(screen.getByRole("list", { name: "Attachments" })).toHaveTextContent("doc.pdf");

    await user.click(open);
    const dialog = screen.getByRole("dialog", { name: "Image: cat.png" });
    const level = within(dialog).getByLabelText("Zoom level");
    expect(level).toHaveTextContent("100%");
    await user.click(within(dialog).getByRole("button", { name: "Zoom in" }));
    await user.click(within(dialog).getByRole("button", { name: "Zoom in" }));
    expect(level).toHaveTextContent("200%");
    expect(within(dialog).getByRole("img")).toHaveStyle({ transform: "scale(2)" });
    await user.click(within(dialog).getByRole("button", { name: "Zoom out" }));
    expect(level).toHaveTextContent("150%");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});

describe("Timeline tool activity", () => {
  it("groups consecutive tool calls into one disclosure with per-call status and details", async () => {
    const user = userEvent.setup();
    const view = replaySession(
      events([
        { kind: "turn.started" },
        { kind: "tool.started", tool_call_id: "t1", name: "browser_navigate", args: { url: "https://example.com" } },
        { kind: "tool.ended", tool_call_id: "t1", result: "ok", is_error: false },
        { kind: "tool.started", tool_call_id: "t2", name: "browser_act", args: { click: "#buy" } },
        { kind: "tool.ended", tool_call_id: "t2", result: "missing", is_error: true },
        { kind: "assistant.final", text: "Done." },
      ]),
    );
    setup(view);
    const groups = document.querySelectorAll("details.activity");
    expect(groups).toHaveLength(1);
    const summary = groups[0]!.querySelector("summary")!;
    expect(summary).toHaveTextContent("Browsed example.com · 2 steps");
    expect(summary).toHaveTextContent("1 failed");
    await user.click(summary);
    // Each call is a sentence about what the agent did; the page's steps hang under the page.
    const lines = [...groups[0]!.querySelectorAll(".tool-line")];
    expect(lines.map((line) => line.textContent)).toEqual(["Opened example.com", "Interacted with the pageFailed"]);
    expect(groups[0]).not.toHaveTextContent(/browser_|Browser navigate/);
    expect([...groups[0]!.querySelectorAll("h4")].every((heading) => heading.closest("details.tool-tech"))).toBe(true);
    const failed = groups[0]!.querySelector<HTMLElement>("li.tool-failed")!;
    await user.click(failed.querySelector("summary")!);
    // JSON stays behind a closed disclosure.
    const tech = failed.querySelector("details.tool-tech")!;
    expect(tech).not.toHaveAttribute("open");
    expect(tech.querySelector("summary")).toHaveTextContent("Technical details");
    expect(failed).toHaveTextContent('"click": "#buy"');
    expect(failed).toHaveTextContent("Error");
  });

  it("turns the trailing calls of a live run into a status line that opens onto them", async () => {
    const user = userEvent.setup();
    const view = replaySession(
      events([
        { kind: "turn.started" },
        { kind: "tool.started", tool_call_id: "t1", name: "browser_navigate", args: { url: "https://nid.naver.com/login" } },
        { kind: "tool.ended", tool_call_id: "t1", result: { url: "https://nid.naver.com/login" }, is_error: false },
        { kind: "tool.started", tool_call_id: "t2", name: "browser_act", args: { tab_id: "a", target: "e3", action: "click" } },
      ]),
    );
    setup(view);
    expect(document.querySelector("details.activity:not(.status-line-group)")).toBeNull();
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("Clicking on nid.naver.com");
    await user.click(status.querySelector("summary")!);
    expect(status.querySelectorAll("li.tool")).toHaveLength(2);
    expect(status.querySelector("li.tool-running")).toHaveTextContent("Clicking on the page");
  });

  it("reads one reply as one bubble, with the calls between its words as an inline chip", () => {
    const view = replaySession(
      events([
        { kind: "message.queued", message_id: "m1", text: "Check the repo", mode: "queue", author_kind: "person", from_agent: null },
        { kind: "message.injected", message_id: "m1" },
        { kind: "turn.started" },
        { kind: "tool.started", tool_call_id: "t0", name: "read", args: { path: "a.md" } },
        { kind: "tool.ended", tool_call_id: "t0", result: "x", is_error: false },
        { kind: "assistant.delta", text: "Seven crates have updates" },
        { kind: "tool.started", tool_call_id: "t1", name: "read", args: { path: "Cargo.lock" } },
        { kind: "tool.ended", tool_call_id: "t1", result: "x", is_error: false },
        { kind: "assistant.final", text: "this week. I checked the lockfile." },
        { kind: "session.settled" },
      ]),
    );
    setup(view);
    const bubbles = document.querySelectorAll(".bubble.assistant");
    expect(bubbles).toHaveLength(1);
    expect(bubbles[0]).toHaveTextContent("Seven crates have updates");
    expect(bubbles[0]).toHaveTextContent("this week. I checked the lockfile.");
    // The chip between the segments is inside the bubble; the call before the first words is not.
    expect(bubbles[0]!.querySelectorAll(".reply-activity details.activity")).toHaveLength(1);
    expect(bubbles[0]!.querySelector(".activity-label")).toHaveTextContent("Read Cargo.lock");
    expect(document.querySelectorAll("li.activity-row")).toHaveLength(1);
  });

  it("puts the browser card right under the message that started the browsing", () => {
    const view = replaySession(
      events([
        { kind: "message.queued", message_id: "m1", text: "Earlier question", mode: "queue", author_kind: "person", from_agent: null },
        { kind: "message.injected", message_id: "m1" },
        { kind: "assistant.final", text: "Earlier answer." },
        { kind: "message.queued", message_id: "m2", text: "Sign in to Acme", mode: "queue", author_kind: "person", from_agent: null },
        { kind: "message.injected", message_id: "m2" },
        { kind: "tool.started", tool_call_id: "t1", name: "browser_open_tab", args: { url: "https://acme.test" } },
      ]),
    );
    const daemon = fakeClient(() => new Response("{}", { status: 200 }));
    render(
      <ClientContext.Provider value={daemon.client}>
        <Timeline view={view} sessionId="s1" browserCard={{ anchorItemId: "m2", node: <section aria-label="Live browser" /> }} />
      </ClientContext.Provider>,
    );
    const rows = [...document.querySelectorAll("ol.timeline > li")];
    const user = rows.findIndex((row) => row.getAttribute("data-message") === "m2");
    expect(rows[user + 1]).toHaveClass("browser-row");
    expect(rows.filter((row) => row.classList.contains("browser-row"))).toHaveLength(1);
  });

  it("shows the typing dots, not a text status, once the agent has started on a message", () => {
    const view = replaySession(
      events([
        { kind: "message.queued", message_id: "m1", text: "Hi", mode: "queue", author_kind: "person", from_agent: null },
        { kind: "message.injected", message_id: "m1" },
        { kind: "turn.started" },
      ]),
    );
    setup(view);
    expect(screen.getByRole("status")).toHaveTextContent("Agent is working");
    expect(document.querySelectorAll(".typing-dot")).toHaveLength(3);
  });
});
