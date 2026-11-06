import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { COMPOSER_CATALOG } from "../test/composerDaemon";
import { ModelPicker, type ModelPickerProps } from "./ModelPicker";

afterEach(cleanup);

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>;
}

function open(props: Partial<ModelPickerProps> = {}) {
  const onChange = vi.fn();
  render(
    <MemoryRouter>
      <Where />
      <Routes>
        <Route path="/" element={<ModelPicker catalog={COMPOSER_CATALOG} value="openai:gpt-6.1" onChange={onChange} {...props} />} />
        <Route path="/settings/models" element={<p>models screen</p>} />
      </Routes>
    </MemoryRouter>,
  );
  return onChange;
}

/** Base UI mounts the popup a frame after the click; with a search row it is a dialog around the list. */
async function openPicker(user: UserEvent, chip = "Model: GPT-6.1") {
  await user.click(screen.getByRole("button", { name: chip }));
  return screen.findByRole("dialog", { name: "Model" });
}

const rows = () => screen.getAllByRole("menuitemradio").map((row) => row.textContent);

describe("ModelPicker", () => {
  it("lists only the chat models that can be used now, by display name, with a ✓ on the current one", async () => {
    const user = userEvent.setup();
    const onChange = open();
    await openPicker(user);
    // The image model and the one without a usable account stay in Settings.
    expect(rows()).toEqual(["GPT-6.1", "GPT-6 Luna", "Claude Sonnet 5.5"]);
    expect(screen.getByRole("menuitemradio", { name: "GPT-6.1" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "GPT-6 Luna" })).toHaveAttribute("aria-checked", "false");
    await user.click(screen.getByRole("menuitemradio", { name: "Claude Sonnet 5.5" }));
    expect(onChange).toHaveBeenCalledWith("anthropic:claude-sonnet-5.5");
  });

  it("filters the rows from its search field", async () => {
    const user = userEvent.setup();
    open();
    const menu = await openPicker(user);
    const search = within(menu).getByRole("searchbox", { name: "Search models" });
    await user.click(search);
    await user.type(search, "luna");
    expect(rows()).toEqual(["GPT-6 Luna"]);
    await user.clear(search);
    await user.type(search, "nothing like this");
    expect(screen.queryAllByRole("menuitemradio")).toHaveLength(0);
    // Base UI appends a word joiner to the live-region text.
    expect(within(menu).getByRole("status")).toHaveTextContent(/^No model matches\./);
  });

  it("opens Settings › Models from its last row", async () => {
    const user = userEvent.setup();
    open();
    await openPicker(user);
    await user.click(screen.getByRole("menuitem", { name: "Settings" }));
    expect(await screen.findByText("models screen")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent("/settings/models");
  });

  it("names the daemon default on the chip while nothing is chosen, and a configured id by its model part", () => {
    open({ value: null });
    expect(screen.getByRole("button", { name: "Model: GPT-6.1" })).toBeInTheDocument();
    cleanup();
    open({ value: "acme:big-model" });
    expect(screen.getByRole("button", { name: "Model: big-model" })).toBeInTheDocument();
  });

  it("says so when no model is ready yet", async () => {
    const user = userEvent.setup();
    open({ catalog: { ...COMPOSER_CATALOG, providers: [] }, value: null });
    await openPicker(user, "Model: gpt-6.1");
    expect(screen.getByRole("menuitem", { name: "No model is ready yet." })).toHaveAttribute("aria-disabled", "true");
  });

  it("keeps a disabled chip closed with its reason as the tooltip", () => {
    open({ disabled: true, title: "An agent’s own chat keeps the agent’s model." });
    const chip = screen.getByRole("button", { name: "Model: GPT-6.1" });
    expect(chip).toBeDisabled();
    expect(chip).toHaveAttribute("title", "An agent’s own chat keeps the agent’s model.");
  });
});
