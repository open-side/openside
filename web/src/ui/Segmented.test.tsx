import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { Segmented } from "./misc";

const OPTIONS = [
  { value: "a", label: "Alpha" },
  { value: "b", label: "Beta" },
  { value: "c", label: "Gamma" },
];

function Harness({ kind }: { kind: "tab" | "radio" }) {
  const [value, setValue] = useState("a");
  return (
    <>
      <button type="button">Before</button>
      <Segmented label="Letters" kind={kind} options={OPTIONS} value={value} onChange={setValue} />
      <button type="button">After</button>
    </>
  );
}

describe("Segmented", () => {
  it("is one Tab stop, at the chosen segment", async () => {
    const user = userEvent.setup();
    render(<Harness kind="radio" />);
    await user.click(screen.getByRole("button", { name: "Before" }));
    await user.tab();
    expect(screen.getByRole("radio", { name: "Alpha" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "After" })).toHaveFocus();
  });

  it("moves the choice and focus with arrows (wrapping), Home and End", async () => {
    const user = userEvent.setup();
    render(<Harness kind="tab" />);
    await user.click(screen.getByRole("tab", { name: "Alpha" }));
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Beta" })).toHaveFocus();
    expect(screen.getByRole("tab", { name: "Beta" })).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowLeft}{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "Gamma" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: "Alpha" })).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{End}");
    expect(screen.getByRole("tab", { name: "Gamma" })).toHaveFocus();
    expect(screen.getByRole("tab", { name: "Alpha" })).toHaveAttribute("tabindex", "-1");
  });
});
