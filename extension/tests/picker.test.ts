// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPicker, type Picker } from "../src/picker";

const items = [
  { value: "", label: "New chat" },
  { value: "a", label: "Alpha" },
  { value: "b", label: "Beta" },
];
let root: HTMLElement;
let picker: Picker;
const onChange = vi.fn();
const button = () => root.querySelector<HTMLButtonElement>(".picker-button")!;
const press = (key: string) => button().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
const list = () => root.querySelector<HTMLElement>("[role=listbox]")!;

beforeEach(() => {
  document.body.innerHTML = '<div id="sessions"></div>';
  root = document.querySelector<HTMLElement>("#sessions")!;
  onChange.mockClear();
  picker = createPicker(root, "Conversation", onChange);
  picker.setItems(items, "");
});

describe("session picker", () => {
  it("exposes the combobox/listbox contract and the selection", () => {
    expect(button().getAttribute("role")).toBe("combobox");
    expect(button().getAttribute("aria-expanded")).toBe("false");
    expect(list().hidden).toBe(true);
    expect(root.dataset.value).toBe("");
    expect(picker.label).toBe("New chat");
    expect(root.querySelectorAll("[role=option]")).toHaveLength(3);
  });

  it("opens with ArrowDown, moves with arrows/Home/End and selects with Enter", () => {
    press("ArrowDown");
    expect(list().hidden).toBe(false);
    expect(button().getAttribute("aria-expanded")).toBe("true");
    press("ArrowDown");
    press("ArrowDown");
    press("ArrowDown");
    expect(button().getAttribute("aria-activedescendant")).toBe(root.querySelectorAll("[role=option]")[2].id);
    press("Home");
    press("End");
    press("Enter");
    expect(onChange).toHaveBeenCalledWith("b");
    expect(picker.value).toBe("b");
    expect(list().hidden).toBe(true);
  });

  it("jumps by type-ahead and closes on Escape without changing the value", () => {
    press("Enter");
    press("b");
    press("Escape");
    expect(list().hidden).toBe(true);
    expect(onChange).not.toHaveBeenCalled();
    press("Enter");
    press("a");
    press(" ");
    expect(onChange).toHaveBeenCalledWith("a");
  });

  it("selects on click and does not call onChange for programmatic updates", () => {
    picker.value = "a";
    expect(onChange).not.toHaveBeenCalled();
    button().click();
    root.querySelectorAll<HTMLElement>("[role=option]")[2].click();
    expect(onChange).toHaveBeenCalledExactlyOnceWith("b");
    expect(root.querySelector("[aria-selected=true]")?.textContent).toBe("Beta");
  });
});
