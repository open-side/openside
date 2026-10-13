import { afterEach, describe, expect, it } from "vitest";
import { fakeClient, json } from "../test/fakeDaemon";
import { outgoing } from "./outgoing";

afterEach(() => outgoing.snapshot().forEach((entry) => outgoing.forget(entry.message_id)));

const message = { message_id: "m1", session_id: "s1", text: "hello", attachments: [] };

describe("outgoing messages", () => {
  it("is sent once the daemon accepts it, and the event log retires it", async () => {
    const daemon = fakeClient(() => json(202, { id: "m1", status: "queued" }));
    const pending = outgoing.submit(daemon.client, message);
    expect(outgoing.snapshot()[0]?.state).toBe("sending");
    expect(await pending).toBeNull();
    expect(outgoing.snapshot()[0]?.state).toBe("sent");
    outgoing.prune("s1", new Set(["m1"]));
    expect(outgoing.snapshot()).toEqual([]);
  });

  it("is Not sent when the POST fails, and a retry reuses the message id", async () => {
    let fail = true;
    const daemon = fakeClient(() => (fail ? json(500, { code: "internal", message: "boom" }) : json(202, { id: "m1", status: "queued" })));
    expect(await outgoing.submit(daemon.client, message)).not.toBeNull();
    expect(outgoing.snapshot()).toMatchObject([{ message_id: "m1", state: "failed" }]);
    fail = false;
    expect(await outgoing.submit(daemon.client, message)).toBeNull();
    expect(outgoing.snapshot()).toMatchObject([{ message_id: "m1", state: "sent" }]);
    expect(daemon.calls.map((call) => (call.body as { message_id: string }).message_id)).toEqual(["m1", "m1"]);
  });

  it("reports an unreachable daemon in plain words", async () => {
    const daemon = fakeClient(() => {
      throw new TypeError("Failed to fetch");
    });
    const failure = await outgoing.submit(daemon.client, message);
    expect(failure?.status).toBe(0);
    expect(outgoing.snapshot()[0]).toMatchObject({ state: "failed", error: "Can’t reach openside" });
  });
});
