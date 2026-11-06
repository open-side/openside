import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createClient } from "../api/client";
import type { ComputerUseStreamEvent } from "../api/types";
import { ClientContext } from "../context";
import { emptySessionView, type SessionView, type TimelineItem } from "../state/session";
import { ComputerUseCard } from "./ComputerUseCard";

const streams: { onEvent: (event: ComputerUseStreamEvent) => void; close: () => void }[] = [];
vi.mock("../api/sse", () => ({
  openComputerUseStream: (options: { onEvent: (event: ComputerUseStreamEvent) => void }) => {
    const stream = { onEvent: options.onEvent, close: vi.fn() };
    streams.push(stream);
    return stream;
  },
}));

const CONNECTED: TimelineItem = { type: "tool", id: "t1", name: "computer_use_connect", args: { window_id: "w1" }, updates: [], result: { connected: true }, done: true };
const CONNECTION = { id: "cu1", app: "calc", title: "Calculator", phase: "connected" } as const;

function renderCard(view: SessionView) {
  const client = createClient({ token: () => "t", fetch: (async () => new Response("{}")) as typeof fetch });
  return render(
    <ClientContext.Provider value={client}>
      <ComputerUseCard sessionId="s1" view={view} />
    </ClientContext.Provider>,
  );
}

afterEach(() => {
  streams.length = 0;
});

describe("ComputerUseCard", () => {
  it("never opens the desktop stream in a private chat", () => {
    const { container } = renderCard({ ...emptySessionView, incognito: true, items: [CONNECTED] });
    expect(streams).toHaveLength(0);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows a valid frame and ignores a malformed one", () => {
    renderCard({ ...emptySessionView, items: [CONNECTED] });
    expect(streams).toHaveLength(1);
    const [stream] = streams;
    if (!stream) throw new Error("the card opened no desktop stream");
    act(() => stream.onEvent({ type: "state", connection: CONNECTION } as ComputerUseStreamEvent));
    act(() =>
      stream.onEvent({ type: "frame", connection: "cu1", seq: 1, width: 10, height: 10, mime: "text/html", data: "PHNjcmlwdD4=" } as ComputerUseStreamEvent),
    );
    expect(screen.queryByRole("img")).toBeNull();
    act(() =>
      stream.onEvent({ type: "frame", connection: "cu1", seq: 2, width: 10, height: 10, mime: "image/png", data: "iVBORw0KGgo=" } as ComputerUseStreamEvent),
    );
    expect(screen.getByRole("img").getAttribute("src")).toBe("data:image/png;base64,iVBORw0KGgo=");
  });
});
