import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ClientContext } from "../context";
import type { SubagentView } from "../state/session";
import { fakeClient, json } from "../test/fakeDaemon";
import { SubagentPanel } from "./SubagentPanel";

const running: SubagentView = {
  id: "sa1",
  task: "Collect prices",
  model: "provider:model",
  tools: ["browser_snapshot"],
  parentToolCallId: "d1",
  status: "running",
  progress: [{ kind: "model_request" }, { kind: "model_response", text: "two shops" }],
};
const failed: SubagentView = { ...running, id: "sa2", task: "Book", status: "failed", reason: "budget exceeded", progress: [] };

function setup(subagents: SubagentView[]) {
  const daemon = fakeClient((call) => json(200, { id: call.path.split("/")[5], status: "running" }));
  const view = render(
    <ClientContext.Provider value={daemon.client}>
      <SubagentPanel sessionId="s1" subagents={subagents} />
    </ClientContext.Provider>,
  );
  return { ...daemon, ...view };
}

describe("SubagentPanel", () => {
  it("lists children with status and their live event stream", async () => {
    const user = userEvent.setup();
    setup([running, failed]);
    const panel = screen.getByRole("region", { name: "Subagents" });
    expect(panel).toHaveTextContent("1 active");
    const [first, second] = within(panel).getAllByRole("listitem").filter((li) => li.dataset.subagent);
    expect(first).toHaveTextContent("Collect prices");
    expect(first).toHaveTextContent("Running");
    expect(within(first!).getByLabelText("Progress events")).toHaveTextContent("2");
    await user.click(within(first!).getByText("Collect prices"));
    const events = within(first!).getByRole("list", { name: "Subagent events" });
    expect(within(events).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "model_request",
      'model_response {"text":"two shops"}',
    ]);
    expect(second).toHaveTextContent("Failed");
    await user.click(within(second!).getByText("Book"));
    expect(second).toHaveTextContent("budget exceeded");
    // Finished children cannot be steered or cancelled.
    expect(within(second!).queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("steers and cancels a running child through the API", async () => {
    const user = userEvent.setup();
    const { calls } = setup([running]);
    await user.click(screen.getByText("Collect prices"));
    await user.type(screen.getByLabelText("Steer sa1"), "only EU shops");
    await user.click(screen.getByRole("button", { name: "Steer" }));
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(calls.map((c) => [c.method, c.path, c.body])).toEqual([
      ["POST", "/v1/sessions/s1/subagents/sa1/steer", { text: "only EU shops" }],
      ["POST", "/v1/sessions/s1/subagents/sa1/cancel", undefined],
    ]);
    expect(screen.getByLabelText("Steer sa1")).toHaveValue("");
  });

  it("renders nothing for a session without subagents", () => {
    const { container } = setup([]);
    expect(container).toBeEmptyDOMElement();
  });
});
