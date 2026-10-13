import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ClientContext } from "../context";
import type { SubagentView } from "../state/session";
import { fakeClient, json } from "../test/fakeDaemon";
import { AgentsTray } from "./SubagentPanel";

const running: SubagentView = {
  id: "sa1",
  task: "Collect prices",
  model: "provider:model",
  tools: ["browser_snapshot"],
  parentToolCallId: "d1",
  status: "running",
  background: false,
  progress: [
    { kind: "model_request" },
    { kind: "tool.started", tool_call_id: "c1", name: "browser_navigate", args: { url: "https://shop.test" } },
    { kind: "tool.ended", tool_call_id: "c1", name: "browser_navigate", result: {}, is_error: false, status_text: "Opened shop.test" },
    { kind: "model_response", text: "two shops" },
  ],
};
const failed: SubagentView = { ...running, id: "sa2", task: "Book", status: "failed", reason: "budget exceeded", progress: [] };

function setup(subagents: SubagentView[]) {
  const daemon = fakeClient((call) => json(200, { id: call.path.split("/")[5], status: "running" }));
  const view = render(
    <ClientContext.Provider value={daemon.client}>
      <AgentsTray subagents={subagents} />
    </ClientContext.Provider>,
  );
  return { ...daemon, ...view };
}

describe("AgentsTray", () => {
  it("lists children with status and their live event stream", async () => {
    const user = userEvent.setup();
    setup([running, failed]);
    await user.click(screen.getByRole("button", { name: "2 agents" }));
    const panel = screen.getByRole("dialog", { name: "Agents" });
    expect(panel).toHaveTextContent("1 working");
    const [first, second] = within(panel).getAllByRole("listitem").filter((li) => li.dataset.subagent);
    expect(first).toHaveTextContent("Collect prices");
    expect(first).toHaveTextContent("Running");
    // No internal event count or raw events: a running child says what it last did.
    expect(first).toHaveTextContent("Running · Opened shop.test");
    expect(first).not.toHaveTextContent("events");
    await user.click(within(first!).getByText("Collect prices"));
    const steps = within(first!).getByRole("list", { name: "Steps" });
    expect(within(steps).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["Opened shop.test"]);
    expect(first).not.toHaveTextContent("model_response");
    expect(second).toHaveTextContent("Failed");
    await user.click(within(second!).getByText("Book"));
    expect(second).toHaveTextContent("budget exceeded");
    // People steer work by talking to the agent: no row offers steer or cancel.
    expect(within(second!).queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("is read-only: a running child offers no steer or cancel", async () => {
    const user = userEvent.setup();
    const { calls } = setup([running]);
    await user.click(screen.getByRole("button", { name: "1 agent" }));
    await user.click(screen.getByText("Collect prices"));
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("button", { name: "Steer" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    expect(calls).toEqual([]);
  });

  it("renders nothing for a session without subagents", () => {
    const { container } = setup([]);
    expect(container).toBeEmptyDOMElement();
  });
});
