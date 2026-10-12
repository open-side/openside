import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { StopTaskDialog, type StopScope } from "./StopTaskDialog";

function open(scope: StopScope) {
  const onConfirm = vi.fn();
  const onClose = vi.fn();
  render(<StopTaskDialog open scope={scope} onClose={onClose} onConfirm={onConfirm} />);
  return { onConfirm, onClose };
}

describe("StopTaskDialog", () => {
  it("the composer Stop says it ends the agent and every running task", async () => {
    const user = userEvent.setup();
    const { onConfirm } = open({ kind: "all", tasks: 3 });
    const dialog = screen.getByRole("dialog", { name: "Stop everything?" });
    expect(dialog).toHaveTextContent("Stops the agent and its 3 running tasks where they are.");
    await user.click(screen.getByRole("button", { name: "Stop everything" }));
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("stays truthful with no tasks and with one", () => {
    const { onClose } = open({ kind: "all", tasks: 0 });
    expect(screen.getByRole("dialog", { name: "Stop everything?" })).toHaveTextContent("Stops the agent where it is.");
    expect(onClose).not.toHaveBeenCalled();
  });

  it("a Task cancel names the task and says the rest keeps running", () => {
    open({ kind: "task", name: "Compare hotels" });
    expect(screen.getByRole("dialog", { name: "Cancel this task?" })).toHaveTextContent("“Compare hotels” stops where it is. The agent and any other tasks keep running");
  });
});
