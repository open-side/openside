import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ClientContext } from "../context";
import { fakeClient, json } from "../test/fakeDaemon";
import { QueueList } from "./QueueList";

function setup({ status = 200, running = false } = {}) {
  const daemon = fakeClient(() =>
    status === 200 ? json(200, { id: "m2", text: "x", status: { queued: "queue" } }) : json(status, { code: "invalid_transition", message: "x" }),
  );
  render(
    <ClientContext.Provider value={daemon.client}>
      <QueueList
        sessionId="s1"
        running={running}
        queue={[
          { id: "m1", text: "steer this", mode: "steer" },
          { id: "m2", text: "follow up", mode: "queue" },
        ]}
      />
    </ClientContext.Provider>,
  );
  return daemon;
}

function row(text: string): HTMLElement {
  return screen.getByText(text).closest("li") as HTMLElement;
}

async function choose(user: ReturnType<typeof userEvent.setup>, text: string, item: string) {
  await user.click(within(row(text)).getByRole("button", { name: "Message options" }));
  await user.click(await screen.findByRole("menuitem", { name: item }));
}

describe("QueueList", () => {
  it("labels each lane and keeps delivery order", () => {
    setup();
    const items = within(screen.getByRole("region", { name: "Queued messages" })).getAllByRole("listitem");
    expect(items.map((item) => item.dataset.message)).toEqual(["m1", "m2"]);
    expect(within(row("steer this")).getByText("Steering")).toBeInTheDocument();
    expect(within(row("follow up")).getByText("Queued")).toBeInTheDocument();
  });

  it("edits a queued message in place", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await choose(user, "follow up", "Edit message");
    const box = screen.getByLabelText("Edit queued message");
    await user.clear(box);
    await user.type(box, "follow up, revised");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(calls).toEqual([
      expect.objectContaining({ method: "PATCH", path: "/v1/sessions/s1/queue/m2", body: { text: "follow up, revised" } }),
    ]);
    expect(screen.queryByLabelText("Edit queued message")).toBeNull();
  });

  it("cancels an edit without a request and keeps the editor open on failure", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ status: 409 });
    await choose(user, "follow up", "Edit message");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(calls).toHaveLength(0);
    await choose(user, "follow up", "Edit message");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/no longer possible/);
    expect(screen.getByLabelText("Edit queued message")).toBeInTheDocument();
  });

  it("promotes follow-ups with Send now and cancels messages", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(within(row("steer this")).getByRole("button", { name: "Message options" }));
    expect(await screen.findByRole("menuitem", { name: "Cancel message" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Send now" })).toBeNull();
    await user.keyboard("{Escape}");

    await choose(user, "follow up", "Send now");
    await choose(user, "steer this", "Cancel message");
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual([
      "POST /v1/sessions/s1/queue/m2/promote",
      "DELETE /v1/sessions/s1/queue/m1",
    ]);
  });

  it("offers Steer instead while a run is active", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ running: true });
    await choose(user, "follow up", "Steer instead");
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual(["POST /v1/sessions/s1/queue/m2/promote"]);
  });
});
