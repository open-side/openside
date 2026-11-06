import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
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

  it("draws a visual block as an inert image and toggles to its source", async () => {
    const user = userEvent.setup();
    setup(view, { 3: [visual] });
    const bubble = document.querySelector(".bubble.assistant .markdown")!;
    expect(bubble.textContent).toBe("Here is the chart:\nDone.");
    // An image never runs script or loads anything, and it has no backdrop of its own.
    const image = await screen.findByRole("img", { name: "SVG visual" });
    expect(image.tagName).toBe("IMG");
    expect(image.getAttribute("src")).toMatch(/^data:image\/svg\+xml/);
    await user.click(screen.getByRole("button", { name: "Source" }));
    expect(screen.queryByRole("img", { name: "SVG visual" })).toBeNull();
    expect(document.querySelector(".visual-source")?.textContent).toBe(SOURCE);
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByRole("img", { name: "SVG visual" })).toBeInTheDocument();
  });

  it("says plainly when a visual cannot be drawn, and strips script from one that can", async () => {
    const broken = { ...visual, id: "3-1", source: "<svg><rect></svg" };
    const scripted = { ...visual, id: "3-2", source: '<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><text>hi</text></svg>' };
    setup(view, { 3: [broken, scripted] });
    expect(await screen.findByText("Couldn’t show this visual. Its source is under Source.")).toBeInTheDocument();
    const image = await screen.findByRole("img", { name: "SVG visual" });
    const svg = decodeURIComponent(image.getAttribute("src")!.split(",").slice(1).join(","));
    expect(svg).not.toContain("<script");
    expect(svg).toContain("<text>hi</text>");
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

describe("generated SVG stays an image", () => {
  it("draws a model-written SVG visual only through <img>, never as inline markup", async () => {
    const hostile = '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><image href="https://tracker.example/x.png"/><rect id="model-rect" width="10" height="10"/></svg>';
    const view = replaySession(events([{ kind: "turn.started" }, { kind: "assistant.final", text: `\`\`\`svg visual\n${hostile}\n\`\`\`` }]));
    const daemon = setup(view, { 2: [{ ...visual, id: "2-0", seq: 2, source: hostile }] });
    const image = await screen.findByRole("img", { name: "SVG visual" });
    expect(image.tagName).toBe("IMG");
    expect(image.getAttribute("src")).toMatch(/^data:image\/svg\+xml/);
    expect(document.querySelector("#model-rect, image, [onload]")).toBeNull();
    expect(daemon.calls.some((call) => call.path.includes("tracker.example"))).toBe(false);
  });

  it("shows an SVG file the agent linked as an <img> from the artifact route, not inline", async () => {
    const daemon = fakeClient(() => new Response('<svg xmlns="http://www.w3.org/2000/svg"><rect id="file-rect"/></svg>', { status: 200, headers: { "content-type": "image/svg+xml" } }));
    render(
      <ClientContext.Provider value={daemon.client}>
        <Timeline view={replaySession(events([{ kind: "turn.started" }, { kind: "assistant.final", text: "![Chart](chart.svg)" }]))} sessionId="s1" />
      </ClientContext.Provider>,
    );
    await waitFor(() => expect(document.querySelector(".md-artifact-image img")).not.toBeNull());
    expect(daemon.calls[0]).toMatchObject({ method: "GET", path: "/v1/sessions/s1/artifacts/chart.svg" });
    expect(document.querySelector("#file-rect")).toBeNull();
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
  it("folds a finished turn into one collapsed Worked line that opens onto its steps and their cards", async () => {
    const user = userEvent.setup();
    const at = (second: number) => `2026-01-01T00:00:${String(second).padStart(2, "0")}Z`;
    const view = replaySession(
      events([
        { kind: "turn.started" },
        { kind: "tool.started", tool_call_id: "t1", name: "browser_navigate", args: { url: "https://example.com" }, ts: at(1) },
        { kind: "tool.ended", tool_call_id: "t1", result: "ok", is_error: false, ts: at(3) },
        { kind: "tool.started", tool_call_id: "t2", name: "browser_act", args: { click: "#buy" }, ts: at(3) },
        { kind: "tool.ended", tool_call_id: "t2", result: "missing", is_error: true, ts: at(5) },
        { kind: "tool.started", tool_call_id: "t3", name: "bash", args: { command: "ls" }, ts: at(9) },
        { kind: "tool.ended", tool_call_id: "t3", result: "z", is_error: false, ts: at(13) },
        { kind: "assistant.final", text: "Done.", ts: at(13) },
      ]),
    );
    setup(view);
    const groups = document.querySelectorAll("details.work");
    expect(groups).toHaveLength(1);
    expect(groups[0]).not.toHaveAttribute("open");
    const summary = groups[0]!.querySelector("summary")!;
    expect(summary).toHaveTextContent("Worked for 12s");
    await user.click(summary);
    expect(groups[0]!.querySelectorAll("li.step")).toHaveLength(3);
    expect(groups[0]).not.toHaveTextContent(/browser_|Browser navigate/);
    const failed = groups[0]!.querySelector<HTMLElement>('li.step[data-state="failed"]')!;
    expect(failed.querySelector(".step-failed")).toHaveTextContent("Failed");
    const card = failed.querySelector(".step-card")!;
    expect(card).not.toBeVisible();
    await user.click(failed.querySelector("summary")!);
    expect(card).toBeVisible();
    expect(card).toHaveTextContent('"click": "#buy"');
  });

  it("keeps a live turn open as Working, with a Thinking shimmer once no step is running", () => {
    const running = replaySession(
      events([
        { kind: "turn.started" },
        { kind: "tool.started", tool_call_id: "t1", name: "browser_navigate", args: { url: "https://nid.naver.com/login" } },
      ]),
    );
    const { unmount } = render(
      <ClientContext.Provider value={fakeClient(() => new Response("{}")).client}>
        <Timeline view={running} sessionId="s1" />
      </ClientContext.Provider>,
    );
    const live = document.querySelector("details.work[data-live]")!;
    expect(live).toHaveAttribute("open");
    expect(live.querySelector("summary")).toHaveTextContent(/^Working/);
    expect(live.querySelector('li.step[data-state="running"]')).not.toBeNull();
    expect(screen.queryByText("Thinking")).toBeNull();
    unmount();
    setup(replaySession(events([
      { kind: "turn.started" },
      { kind: "tool.started", tool_call_id: "t1", name: "read", args: { path: "a.md" } },
      { kind: "tool.ended", tool_call_id: "t1", result: "x", is_error: false },
    ])));
    expect(screen.getByRole("status")).toHaveTextContent("Thinking");
  });

  it("never renders provisional narration even before its tool call arrives", () => {
    setup(replaySession(events([
      { kind: "turn.started" },
      { kind: "assistant.delta", text: "Internal page narration" },
      { kind: "turn.ended" },
    ])));
    expect(screen.queryByText("Internal page narration")).toBeNull();
    expect(document.querySelector(".bubble.assistant")).toBeNull();
  });

  it("hides tool-adjacent narration and shows only the completed reply", () => {
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
    expect(bubbles[0]).not.toHaveTextContent("Seven crates have updates");
    expect(bubbles[0]).toHaveTextContent("this week. I checked the lockfile.");
    expect(document.querySelectorAll("li.work-row")).toHaveLength(1);
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

  it("shows Working and Thinking, not typing dots, once the agent has started on a message", () => {
    const view = replaySession(
      events([
        { kind: "message.queued", message_id: "m1", text: "Hi", mode: "queue", author_kind: "person", from_agent: null },
        { kind: "message.injected", message_id: "m1" },
        { kind: "turn.started" },
      ]),
    );
    setup(view);
    expect(screen.getByRole("status")).toHaveTextContent("Thinking");
    expect(document.querySelector("details.work[data-live] summary")).toHaveTextContent(/^Working/);
    expect(document.querySelector(".typing-dot")).toBeNull();
  });

  it("branches from a finished reply with the turn's person message", async () => {
    const user = userEvent.setup();
    const onFork = vi.fn(async () => undefined);
    const view = replaySession(
      events([
        { kind: "message.queued", message_id: "m1", text: "Hi", mode: "queue", author_kind: "person", from_agent: null },
        { kind: "message.injected", message_id: "m1" },
        { kind: "turn.started" },
        { kind: "assistant.final", text: "Hello." },
        { kind: "message.completed", message_id: "m1", outcome: "completed" },
        { kind: "turn.ended" },
        { kind: "session.settled" },
      ]),
    );
    render(
      <ClientContext.Provider value={fakeClient(() => new Response("{}")).client}>
        <Timeline view={view} sessionId="s1" onFork={onFork} />
      </ClientContext.Provider>,
    );
    const reply = document.querySelector(".bubble.assistant[data-latest]")!;
    await user.click(within(reply as HTMLElement).getByRole("button", { name: "Branch from here" }));
    expect(onFork).toHaveBeenCalledWith("m1");
  });

  it("cites a searched source with its site icon", () => {
    const view = replaySession(
      events([
        { kind: "turn.started" },
        { kind: "tool.started", tool_call_id: "t1", name: "websearch", args: { query: "rust" } },
        {
          kind: "tool.ended",
          tool_call_id: "t1",
          is_error: false,
          result: { sources: [{ url: "https://www.rust-lang.org/learn", title: "Learn Rust", origin: "https://www.rust-lang.org", favicon_ref: { type: "data", data_url: "data:image/png;base64,AAAA" } }] },
        },
        { kind: "assistant.final", text: "See [Learn Rust](https://www.rust-lang.org/learn)." },
      ]),
    );
    setup(view);
    const link = screen.getByRole("link", { name: /Learn Rust/ });
    expect(link).toHaveClass("citation");
    expect(link.querySelector("img.citation-icon")).toHaveAttribute("src", "data:image/png;base64,AAAA");
  });
});
