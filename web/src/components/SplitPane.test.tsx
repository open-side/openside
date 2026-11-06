import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { MemoryRouter, Route, Routes, useParams } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Event } from "../api/types";
import { ClientContext } from "../context";
import { replaySession } from "../state/session";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { SplitPane, SplitToggle } from "./SplitPane";
import { SplitPaneContext, useSplitPane, useSplitPaneState } from "./splitPaneState";
import { Timeline } from "./Timeline";

// The side chat is a whole ThreadView with its own stream; the pane only decides which one and how.
vi.mock("../pages/ThreadView", () => ({
  ThreadView: ({ sessionId, side, header }: { sessionId: string; side?: boolean; header: (state: { view: { title: string } }) => ReactNode }) => (
    <div data-testid="side-thread" data-session={sessionId} data-side={String(side)}>
      {header({ view: { title: "" } })}
    </div>
  ),
}));

const PLAN = "## Plan\n\nDay one: museums.";

function Thread({ children }: { children?: ReactNode }) {
  const api = useSplitPaneState();
  return (
    <SplitPaneContext.Provider value={api}>
      <SplitToggle />
      {children}
      {api.open ? <SplitPane sessionId="s1" /> : null}
    </SplitPaneContext.Provider>
  );
}

function FullChat() {
  return <p>full chat {useParams().id}</p>;
}

function mount(handler: (call: Call) => Response | undefined = () => undefined, children?: ReactNode) {
  const daemon = fakeClient((call) => handler(call) ?? json(404, { error: { code: "not_found", message: "nope" } }));
  render(
    <ClientContext.Provider value={daemon.client}>
      <ToastProvider>
        <MemoryRouter>
          <Routes>
            <Route path="/" element={<Thread>{children}</Thread>} />
            <Route path="/sessions/:id" element={<FullChat />} />
          </Routes>
        </MemoryRouter>
      </ToastProvider>
    </ClientContext.Provider>,
  );
  return daemon;
}

const pane = () => screen.getByRole("complementary", { name: "Side pane" });

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("SplitPane", () => {
  it("toggles from the thread header and reports its state", async () => {
    const user = userEvent.setup();
    mount();
    const toggle = screen.getByRole("button", { name: "Show side pane" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByRole("complementary")).toBeNull();
    await user.click(toggle);
    expect(screen.getByRole("button", { name: "Hide side pane", pressed: true })).toBeInTheDocument();
    expect(pane()).toBeInTheDocument();
  });

  it("renders no toggle where the thread cannot split", () => {
    render(<SplitToggle />);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("opens empty with a way to start a side chat, and closes from its own header", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(screen.getByRole("button", { name: "Show side pane" }));
    expect(within(pane()).getByText("Open a file to preview it here.")).toBeInTheDocument();
    expect(within(pane()).getByText("or")).toBeInTheDocument();
    expect(within(pane()).getAllByRole("button", { name: "New side chat" })).toHaveLength(2);
    // Nothing to expand yet.
    expect(within(pane()).queryByRole("button", { name: /full/ })).toBeNull();
    await user.click(within(pane()).getByRole("button", { name: "Hide side pane" }));
    expect(screen.queryByRole("complementary")).toBeNull();
    expect(screen.getByRole("button", { name: "Show side pane" })).toHaveAttribute("aria-pressed", "false");
  });

  it("starts a side chat that belongs to no persona and shows it in the pane", async () => {
    const user = userEvent.setup();
    const daemon = mount((call) => (call.method === "POST" && call.path === "/v1/sessions" ? json(201, { id: "side1", title: "" }) : undefined));
    await user.click(screen.getByRole("button", { name: "Show side pane" }));
    await user.click(within(pane()).getByText("New side chat"));
    const thread = await within(pane()).findByTestId("side-thread");
    expect(daemon.calls.find((call) => call.path === "/v1/sessions")?.body).toEqual({ agent_id: null });
    expect(thread).toHaveAttribute("data-session", "side1");
    expect(thread).toHaveAttribute("data-side", "true");
    expect(within(thread).getByText("New chat")).toBeInTheDocument();
    await user.click(within(pane()).getByRole("button", { name: "Open as a full chat" }));
    expect(await screen.findByText("full chat side1")).toBeInTheDocument();
  });

  it("says so when a side chat cannot start", async () => {
    const user = userEvent.setup();
    mount((call) => (call.path === "/v1/sessions" ? json(503, { code: "unavailable", message: "Daemon is busy" }) : undefined));
    await user.click(screen.getByRole("button", { name: "Show side pane" }));
    await user.click(within(pane()).getByText("New side chat"));
    expect(await screen.findByText("Daemon is busy")).toBeInTheDocument();
    expect(within(pane()).queryByTestId("side-thread")).toBeNull();
    expect(within(pane()).getByText("Open a file to preview it here.")).toBeInTheDocument();
  });
});

describe("panel documents beside the thread", () => {
  const events = [
    { kind: "turn.started" },
    { kind: "assistant.final", text: "Here it is.", attachments: [{ path: "documents/delivered/b.md", title: "Itinerary", display: "panel", bytes: PLAN.length, content_type: "text/markdown" }] },
  ].map((p, i) => ({ session_id: "s1", seq: i + 1, ts: "2026-01-01T00:00:00Z", run_id: null, ...p })) as Event[];
  const markdown = (call: Call) => (call.path.endsWith("b.md") ? new Response(PLAN, { status: 200, headers: { "content-type": "text/markdown" } }) : undefined);
  const viewport = (desktop: boolean) => vi.stubGlobal("matchMedia", (query: string) => ({ matches: desktop, media: query, addEventListener() {}, removeEventListener() {} }));

  it("opens in the split pane on a desktop-wide window", async () => {
    const user = userEvent.setup();
    viewport(true);
    mount(markdown, <Timeline view={replaySession(events)} sessionId="s1" />);
    await user.click(screen.getByRole("button", { name: /Itinerary/ }));
    expect(await within(pane()).findByRole("heading", { name: "Plan" })).toBeInTheDocument();
    expect(within(pane()).getByText("Itinerary")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Hide side pane", pressed: true })).toBeInTheDocument();
    await user.click(within(pane()).getByRole("button", { name: "Open full size" }));
    expect(await within(await screen.findByRole("dialog", { name: "Itinerary" })).findByRole("heading", { name: "Plan" })).toBeInTheDocument();
  });

  it("keeps the sheet on a narrow window", async () => {
    const user = userEvent.setup();
    viewport(false);
    mount(markdown, <Timeline view={replaySession(events)} sessionId="s1" />);
    await user.click(screen.getByRole("button", { name: /Itinerary/ }));
    expect(await screen.findByRole("dialog", { name: "Itinerary" })).toBeInTheDocument();
    expect(screen.queryByRole("complementary")).toBeNull();
  });

  it("previews other files the way the Files sheet does", async () => {
    const user = userEvent.setup();
    viewport(true);
    const artifact = { path: "out/data.bin", mime: "application/octet-stream", size: 4, source: "tool", purpose: "deliverable", created_at: "2026-01-01T00:00:00Z", modified_at: "2026-01-01T00:00:00Z" };
    mount((call) => (call.path === "/v1/sessions/s1/artifacts" ? json(200, { artifacts: [artifact] }) : undefined), <ShowFile path="out/data.bin" />);
    await user.click(screen.getByRole("button", { name: "show file" }));
    expect(await within(pane()).findByText("No preview for this file type.")).toBeInTheDocument();
    expect(within(pane()).getByText("data.bin")).toBeInTheDocument();
  });
});

function ShowFile({ path }: { path: string }) {
  const split = useSplitPane();
  return (
    <button type="button" onClick={() => split?.show({ kind: "file", path })}>
      show file
    </button>
  );
}
