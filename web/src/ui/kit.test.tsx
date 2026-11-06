import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { KbdChip, Switch } from "./misc";
import { MenuItem, PopoverMenu } from "./Menu";
import { Dropdown, Select } from "./Select";
import { SettingsRow } from "./Settings";
import { ThumbnailPicker } from "./ThumbnailPicker";
import { readFileSync } from "node:fs";

// Vitest stubs CSS imports (even `?raw`) and jsdom gives an http import.meta.url, so the
// stylesheet is read from disk relative to the web package root (vitest's cwd).
const uiCss = readFileSync("src/ui/ui.css", "utf8");

describe("Switch", () => {
  it("takes its name from the row and toggles with Space", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SettingsRow
        title="Spell check"
        description="Check spelling as you type"
        control={({ labelId, descriptionId }) => <Switch checked={false} onChange={onChange} aria-labelledby={labelId} aria-describedby={descriptionId} />}
      />,
    );
    const toggle = screen.getByRole("switch", { name: "Spell check" });
    expect(toggle).toHaveAccessibleDescription("Check spelling as you type");
    expect(screen.getByRole("group", { name: "Spell check" })).toBeInTheDocument();
    toggle.focus();
    await user.keyboard(" ");
    expect(onChange).toHaveBeenCalledWith(true);
  });
});

describe("SettingsRow", () => {
  it("renders a navigation row as one link", () => {
    render(<SettingsRow title="Advanced" href="https://example.com/advanced" />);
    const link = screen.getByRole("link", { name: "Advanced" });
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
  });
});

describe("ThumbnailPicker", () => {
  function Harness() {
    const [value, setValue] = useState<"sf" | "ny">("ny");
    return (
      <ThumbnailPicker
        label="Tab style"
        value={value}
        onChange={setValue}
        options={[
          { value: "sf", label: "San Francisco", thumbnail: null },
          { value: "ny", label: "New York", thumbnail: null },
        ]}
      />
    );
  }

  it("is a radio group with one Tab stop that arrows move", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    expect(screen.getByRole("radiogroup", { name: "Tab style" })).toBeInTheDocument();
    const sf = screen.getByRole("radio", { name: "San Francisco" });
    const ny = screen.getByRole("radio", { name: "New York" });
    expect(ny).toHaveAttribute("aria-checked", "true");
    expect(sf).toHaveAttribute("tabindex", "-1");
    ny.focus();
    await user.keyboard("{ArrowLeft}");
    expect(sf).toHaveFocus();
    expect(sf).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{End}");
    expect(ny).toHaveFocus();
  });
});

describe("KbdChip", () => {
  it("speaks its label instead of the glyphs", () => {
    render(<KbdChip keys={["⌘", "S"]} label="Command S" />);
    expect(screen.getByText("Command S")).toBeInTheDocument();
    expect(screen.getByText("⌘").closest("kbd.ui-kbd")).toHaveAttribute("aria-hidden", "true");
  });
});

describe("PopoverMenu", () => {
  it("marks picker rows as menuitemradio and filters with the search row", async () => {
    const user = userEvent.setup();
    const pick = vi.fn();
    render(
      <PopoverMenu trigger={<button type="button">Model</button>} label="Model" search={{ placeholder: "Search models", empty: "No models" }}>
        <MenuItem checked onClick={() => pick("sol")}>
          GPT-6.1 Sol
        </MenuItem>
        <MenuItem checked={false} onClick={() => pick("luna")}>
          GPT-6 Luna
        </MenuItem>
      </PopoverMenu>,
    );
    await user.click(screen.getByRole("button", { name: "Model" }));
    expect(await screen.findByRole("menuitemradio", { name: "GPT-6.1 Sol" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "GPT-6 Luna" })).toHaveAttribute("aria-checked", "false");
    await user.type(screen.getByPlaceholderText("Search models"), "luna");
    expect(screen.queryByRole("menuitemradio", { name: "GPT-6.1 Sol" })).not.toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(pick).toHaveBeenCalledWith("luna");
    await user.click(screen.getByRole("button", { name: "Model" }));
    await user.type(await screen.findByPlaceholderText("Search models"), "zzz");
    // Base UI appends a word joiner to the live-region text.
    expect(screen.getByRole("status")).toHaveTextContent(/^No models/);
  });
});

describe("Dropdown", () => {
  it("reads a multiple choice as a list", () => {
    render(
      <Dropdown
        multiple
        aria-label="Customize sidebar"
        value={["chats", "bookmarks"]}
        onChange={() => undefined}
        options={[
          { value: "chats", label: "Chats" },
          { value: "bookmarks", label: "Bookmarks" },
          { value: "history", label: "History" },
        ]}
      />,
    );
    expect(screen.getByRole("combobox", { name: "Customize sidebar" })).toHaveTextContent("Chats, Bookmarks");
  });
});

describe("Select", () => {
  function Harness() {
    const [value, setValue] = useState("light");
    return (
      <Select
        variant="inline"
        aria-label="Theme"
        value={value}
        onChange={setValue}
        options={[
          { value: "system", label: "System" },
          { value: "light", label: "Light" },
          { value: "dark", label: "Dark" },
        ]}
      />
    );
  }

  it("opens a listbox of options with the current one selected, and Escape returns focus", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const trigger = screen.getByRole("combobox", { name: "Theme" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    await user.click(trigger);
    const listbox = await screen.findByRole("listbox");
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(trigger).toHaveAttribute("aria-controls", listbox.id);
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["System", "Light", "Dark"]);
    expect(screen.getByRole("option", { name: "Light" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("option", { name: "Dark" })).toHaveAttribute("aria-selected", "false");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(trigger).toHaveAttribute("aria-expanded", "false"));
    expect(trigger).toHaveFocus();
  });
});

describe("PopoverMenu focus", () => {
  it("returns focus to its trigger on Escape", async () => {
    const user = userEvent.setup();
    render(
      <PopoverMenu trigger={<button type="button">Options</button>} label="Conversation options">
        <MenuItem onClick={() => undefined}>Rename</MenuItem>
      </PopoverMenu>,
    );
    const trigger = screen.getByRole("button", { name: "Options" });
    await user.click(trigger);
    expect(await screen.findByRole("menuitem", { name: "Rename" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    expect(trigger).toHaveFocus();
  });
});

describe("touch targets", () => {
  it("gives the select trigger and the split “…” button a 44px hit area at phone width", () => {
    const phone = uiCss.slice(uiCss.indexOf("@media (max-width: 820px) {\n  .ui-select,\n  .ui-split-select-more"));
    expect(phone).toMatch(/^@media \(max-width: 820px\) \{\s+\.ui-select,\s+\.ui-split-select-more \{\s+position: relative;/);
    expect(phone).toMatch(/\.ui-select::after,\s+\.ui-split-select-more::after \{[^}]*inset: calc\(\(100% - 44px\) \/ 2\)/);
  });
});

describe("PopoverMenu picker rows", () => {
  it("keeps menuitemradio state and works from the keyboard", async () => {
    const user = userEvent.setup();
    const pick = vi.fn();
    render(
      <PopoverMenu trigger={<button type="button">Effort</button>} label="Effort">
        <MenuItem checked onClick={() => pick("low")}>
          Low
        </MenuItem>
        <MenuItem checked={false} onClick={() => pick("high")}>
          High
        </MenuItem>
        <MenuItem onClick={() => pick("settings")}>Settings</MenuItem>
      </PopoverMenu>,
    );
    const trigger = screen.getByRole("button", { name: "Effort" });
    trigger.focus();
    await user.keyboard("{Enter}");
    const low = await screen.findByRole("menuitemradio", { name: "Low" });
    expect(low).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "High" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("menuitem", { name: "Settings" })).not.toHaveAttribute("aria-checked");
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(screen.getByRole("menuitemradio", { name: "High" })).toHaveAttribute("data-highlighted"));
    await user.keyboard("{Enter}");
    expect(pick).toHaveBeenCalledWith("high");
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("says why a row is off: an accessible description and a tooltip when the keyboard reaches it", async () => {
    const user = userEvent.setup();
    render(
      <PopoverMenu trigger={<button type="button">Folder</button>} label="Folder">
        <MenuItem onClick={() => undefined}>Rename</MenuItem>
        <MenuItem disabled description="Why">
          Attach folder
        </MenuItem>
      </PopoverMenu>,
    );
    const trigger = screen.getByRole("button", { name: "Folder" });
    trigger.focus();
    await user.keyboard("{Enter}");
    const rename = await screen.findByRole("menuitem", { name: "Rename" });
    expect(rename).not.toHaveAttribute("aria-describedby");
    const row = screen.getByRole("menuitem", { name: "Attach folder" });
    expect(row).toHaveAttribute("aria-disabled", "true");
    expect(row).toHaveAccessibleDescription("Why");

    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(row).toHaveAttribute("data-highlighted"));
    await waitFor(() => expect(document.querySelector(".ui-tooltip")).toHaveTextContent("Why"));
  });
});

describe("coarse pointer", () => {
  it("grows buttons and chips an invisible 44px hit area without changing their height", () => {
    const coarse = uiCss.slice(uiCss.indexOf("@media (pointer: coarse) {"));
    expect(coarse).toMatch(/:where\(\.ui-button, button\.ui-chip\) \{\s+position: relative;/);
    expect(coarse).toMatch(/inset: calc\(\(100% - 44px\) \/ 2\) 0;/);
  });
});
