import "fake-indexeddb/auto";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { outbox } from "../pwa/outbox";
import { outgoing } from "../state/outgoing";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { Composer } from "./Composer";

afterEach(async () => {
  for (const entry of outbox.snapshot()) await outbox.discard(entry.message_id);
});

const isSend = (call: Call) => call.method === "POST" && !call.path.includes("/prewarm");

function unreachable(call: Call): Response {
  if (call.path.includes("/prewarm")) return json(204, undefined);
  throw new TypeError("Failed to fetch");
}

function mount(sessionId: string, handler: (call: Call) => Response, extra: Partial<Parameters<typeof Composer>[0]> = {}) {
  const daemon = fakeClient(handler);
  render(
    <ClientContext.Provider value={daemon.client}>
      <Composer sessionId={sessionId} {...extra} />
    </ClientContext.Provider>,
  );
  return daemon;
}

describe("Composer offline outbox", () => {
  it("queues a message typed while the daemon is unreachable and re-sends it under the same id", async () => {
    const user = userEvent.setup();
    const down = mount("s1", unreachable);
    await user.type(screen.getByRole("textbox", { name: "Message" }), "hello later");
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(await screen.findByText("1 message waiting to send")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("");
    expect(screen.queryByRole("alert")).toBeNull();
    const attempted = down.calls.find(isSend)!.body as { message_id: string };

    const [entry] = outbox.snapshot();
    expect(entry).toMatchObject({ session_id: "s1", text: "hello later", message_id: attempted.message_id });

    const up = fakeClient(() => json(202, { id: "m", status: "queued" }));
    let sent = 0;
    await act(async () => {
      sent = (await outbox.flush(up.client)).sent;
    });
    expect(sent).toBe(1);
    expect(up.calls.find(isSend)).toMatchObject({ path: "/v1/sessions/s1/messages", body: { message_id: attempted.message_id, text: "hello later", mode: "queue" } });
    await waitFor(() => expect(screen.queryByText("1 message waiting to send")).toBeNull());
  });

  it("keeps a refusal from the daemon in the transcript as a Not sent message instead of queueing it", async () => {
    const user = userEvent.setup();
    mount("s1", (call) => (call.path.includes("/prewarm") ? json(204, undefined) : json(400, { code: "empty_message", message: "Nothing to send" })));
    await user.type(screen.getByRole("textbox", { name: "Message" }), "x");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(outgoing.snapshot()).toMatchObject([{ session_id: "s1", text: "x", state: "failed" }]));
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("");
    expect(outbox.snapshot()).toEqual([]);
    outgoing.forget(outgoing.snapshot()[0]!.message_id);
  });
});

describe("Composer share draft", () => {
  it("starts from shared text and files, once", async () => {
    const applied = vi.fn();
    mount("s1", () => json(204, undefined), {
      draft: { id: "d1", text: "Title\nhttps://example.com", files: [new File(["x"], "shot.png", { type: "image/png" })] },
      onDraftApplied: applied,
    });
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("Title\nhttps://example.com");
    expect(screen.getByRole("button", { name: "Remove attachment shot.png" })).toBeInTheDocument();
    expect(applied).toHaveBeenCalledTimes(1);
    await act(async () => undefined);
    expect(applied).toHaveBeenCalledTimes(1);
  });
});
