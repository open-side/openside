import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { HOME_SUGGESTIONS, SuggestedTasks } from "./SuggestedTasks";

describe("SuggestedTasks", () => {
  it("renders nothing, heading included, without suggestions", () => {
    const { container } = render(<SuggestedTasks suggestions={[]} onPick={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("hands the picked suggestion back", async () => {
    const onPick = vi.fn();
    render(<SuggestedTasks suggestions={HOME_SUGGESTIONS} onPick={onPick} />);
    expect(screen.getByRole("heading", { name: "Suggested tasks" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Check an account/ }));
    expect(onPick).toHaveBeenCalledWith(HOME_SUGGESTIONS[0]);
  });
});
