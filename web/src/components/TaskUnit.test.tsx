import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createClient } from "../api/client";
import { ClientContext } from "../context";
import type { BrowserStateView, SubagentView } from "../state/session";
import { TaskUnit } from "./TaskUnit";

class FakeSocket {
  static last: FakeSocket | null = null;
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(readonly url: string) {
    FakeSocket.last = this;
  }
  send() {}
  close() {
    this.closed = true;
    this.onclose?.();
  }
  receive(data: unknown) {
    this.onmessage?.({ data: JSON.stringify(data) });
  }
}

const task = (status: SubagentView["status"] = "running", output?: string): SubagentView => ({
  id: "a", task: "Sign in to mail", model: "m", tools: [], parentToolCallId: "call-a", status, background: true, progress: [], ...(output ? { output } : {}),
});
const state = (patch: Partial<BrowserStateView> = {}): BrowserStateView => ({
  task_id: "a", tab_id: "tab2", seq: 5, url: "https://nid.example.com/login", host: "nid.example.com", title: "Sign in · Mail",
  phase: "acting", page_kind: "login", facts: { heading: "Navigation Menu" }, trail: ["Opening nid.example.com", "Reading nid.example.com"],
  action: { tool: "browser_act", tool_call_id: "x", phrase: "Typing" }, ...patch,
});

function setup(initialTask = task(), initialState: BrowserStateView | null = state(), newest = false) {
  FakeSocket.last = null;
  const requests: Array<{ path: string; auth: string | null }> = [];
  const onShowPending = vi.fn();
  const client = createClient({
    token: () => "device-token",
    fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), "http://x").pathname;
      requests.push({ path, auth: new Headers(init?.headers).get("Authorization") });
      return new Response(new Blob(["jpeg"], { type: "image/jpeg" }));
    }) as typeof fetch,
    webSocket: (url) => new FakeSocket(url) as unknown as WebSocket,
  });
  const ui = (current: SubagentView, tab: BrowserStateView | null) => (
    <ClientContext.Provider value={client}>
      <TaskUnit sessionId="s1" task={current} state={tab ?? undefined} newest={newest} onShowPending={onShowPending} />
    </ClientContext.Provider>
  );
  const result = render(ui(initialTask, initialState));
  return { ...result, requests, onShowPending, update: (current: SubagentView, tab: BrowserStateView | null) => result.rerender(ui(current, tab)) };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("TaskUnit", () => {
  it("rests as a step bubble with the current line and fainter earlier steps, without opening the stream", () => {
    setup();
    expect(screen.getByRole("button", { name: "Show the browser: Sign in · Mail. Typing on nid.example.com" })).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Earlier steps" })).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "Reading nid.example.com",
      "Opening nid.example.com",
    ]);
    expect(FakeSocket.last).toBeNull();
  });

  it("a Task without a browser is only its steps: no title, no controls", () => {
    const progress = [
      { kind: "tool.started", tool_call_id: "c1", name: "web_fetch", args: {} },
      { kind: "tool.ended", tool_call_id: "c1", name: "web_fetch", result: {}, is_error: false, status_text: "Read the forecast" },
      { kind: "tool.started", tool_call_id: "c2", name: "web_fetch", args: {}, status_text: "Checking the radar" },
    ];
    setup({ ...task(), progress }, null);
    expect(screen.getByRole("status")).toHaveTextContent("Checking the radar");
    expect(within(screen.getByRole("list", { name: "Earlier steps" })).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["Read the forecast"]);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByText("Sign in to mail")).toBeNull();
  });

  it("the newest Task at work opens its browser by itself and folds into its result when done", () => {
    const { update } = setup(task(), state(), true);
    expect(screen.getByRole("region", { name: "Browser: Sign in · Mail" })).toBeInTheDocument();
    update(task("completed", "Signed in to mail, 28 messages. Nothing changed."), state({ seq: 6, phase: "done", action: null }));
    expect(screen.queryByRole("region", { name: /Browser:/ })).toBeNull();
    expect(screen.getByRole("button", { name: /Show the browser/ })).toHaveTextContent("Signed in to mail, 28 messages.");
  });

  it("expands to the tab's live picture with no title chip, and never shows a blank page", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: /Show the browser/ }));
    const card = screen.getByRole("region", { name: "Browser: Sign in · Mail" });
    // A page landmark is never the title, and before a frame there is a skeleton that says so.
    expect(within(card).queryByText("Navigation Menu")).toBeNull();
    expect(within(card).getByText("Loading the browser")).toBeInTheDocument();
    const socket = FakeSocket.last!;
    expect(socket.url).toContain("/v1/sessions/s1/browser/preview");
    act(() => {
      socket.onopen?.();
      socket.receive({ type: "frame", data: "/9j/live", width: 720, height: 450, tab_id: "tab2" });
    });
    expect(within(card).getByAltText("Remote browser tab")).toBeInTheDocument();
    expect(card).toHaveAttribute("data-live", "true");
    expect(within(card).queryByText("Sign in · Mail")).toBeNull();
    expect(within(card).queryByText("Live")).toBeNull();
    expect(within(card).getByRole("status")).toHaveTextContent("Typing on nid.example.com");

    await user.click(within(card).getByRole("button", { name: "Collapse" }));
    expect(socket.closed).toBe(true);
    expect(screen.getByRole("button", { name: /Show the browser/ })).toBeInTheDocument();
  });

  it("shows another tab's stream as this Task's last picture, never as live", async () => {
    const create = vi.fn().mockReturnValue("blob:last");
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: create, revokeObjectURL: vi.fn() }));
    const user = userEvent.setup();
    const { requests } = setup(task(), state({ thumbnail: "thumb1" }));
    await user.click(screen.getByRole("button", { name: /Show the browser/ }));
    act(() => {
      FakeSocket.last!.onopen?.();
      FakeSocket.last!.receive({ type: "frame", data: "/9j/other", width: 720, height: 450, tab_id: "tab1" });
    });
    expect(await screen.findByAltText("Last picture of Sign in · Mail")).toHaveAttribute("src", "blob:last");
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
    expect(screen.getByRole("region", { name: /Browser:/ })).not.toHaveAttribute("data-live");
    expect(requests).toContainEqual({ path: "/v1/sessions/s1/browser/thumbnails/thumb1", auth: "Bearer device-token" });
  });

  it("asks for the person while waiting, and ✕ stops a running Task only after a confirmation", async () => {
    const user = userEvent.setup();
    const { update, onShowPending, requests } = setup(task("waiting_human"), state({ phase: "waiting_human", action: null }), true);
    await user.click(screen.getByRole("button", { name: "Action needed" }));
    expect(onShowPending).toHaveBeenCalledOnce();
    update(task("running"), state({ seq: 6 }));
    await user.click(screen.getByRole("button", { name: "Stop this task" }));
    const confirm = await screen.findByRole("dialog", { name: "Stop this task?" });
    await user.click(within(confirm).getByRole("button", { name: "Keep running" }));
    expect(requests.some((request) => request.path.endsWith("/cancel"))).toBe(false);
    await user.click(screen.getByRole("button", { name: "Stop this task" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Stop this task?" })).getByRole("button", { name: "Stop task" }));
    await waitFor(() => expect(requests.map((request) => request.path)).toContain("/v1/sessions/s1/subagents/a/cancel"));
  });

  it("hides ✕ once the Task has left the page to write its answer", () => {
    setup(task("running"), state({ phase: "done", action: null }), true);
    expect(screen.getByRole("region", { name: /Browser:/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop this task" })).toBeNull();
  });
});
