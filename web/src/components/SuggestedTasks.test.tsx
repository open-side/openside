import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { HOME_SUGGESTIONS, SuggestedTasks } from "./SuggestedTasks";

describe("SuggestedTasks", () => {
  it("renders nothing, heading included, without suggestions", () => {
    const { container } = render(<SuggestedTasks suggestions={[]} onPrompt={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("hands a prompt card's prompt back and links the Settings card", async () => {
    const onPrompt = vi.fn();
    render(
      <MemoryRouter>
        <SuggestedTasks suggestions={HOME_SUGGESTIONS} onPrompt={onPrompt} />
      </MemoryRouter>,
    );
    expect(screen.getByRole("heading", { name: "Suggested tasks" })).toBeInTheDocument();
    const memory = screen.getByRole("button", { name: /Set up openside’s memory/ });
    expect(memory).toHaveTextContent("Use this prompt");
    await userEvent.click(memory);
    expect(onPrompt).toHaveBeenCalledWith("memory", "shell.suggested.memory.prompt");
    const settings = screen.getByRole("link", { name: /Customize openside/ });
    expect(settings).toHaveAttribute("href", "/settings");
    expect(settings).toHaveTextContent("Open Settings");
  });
});
