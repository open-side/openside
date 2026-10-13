import { beforeEach, describe, expect, it, vi } from "vitest";
import { attachSuggestions } from "./suggest";

let input: HTMLInputElement;
const list = () => document.querySelector<HTMLElement>("[role=listbox]")!;
const press = (key: string) => input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
const type = (value: string) => {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
};

beforeEach(() => {
  document.body.innerHTML = '<div class="auth-input-wrap"><input id="user" placeholder="Email" /></div>';
  input = document.querySelector("input")!;
  attachSuggestions(input, ["ada@example.com", "ada@work.example", "grace@example.com"]);
});

describe("attachSuggestions", () => {
  it("wires the combobox roles and stays closed until the input is focused", () => {
    expect(input.getAttribute("role")).toBe("combobox");
    expect(input.getAttribute("aria-controls")).toBe(list().id);
    expect(list().hidden).toBe(true);
    input.dispatchEvent(new Event("focus"));
    expect(list().hidden).toBe(false);
    expect(list().querySelectorAll("[role=option]")).toHaveLength(3);
  });

  it("filters while typing and hides when nothing is left or the value is already complete", () => {
    type("ada");
    expect([...list().children].map((option) => option.textContent)).toEqual(["ada@example.com", "ada@work.example"]);
    type("zzz");
    expect(list().hidden).toBe(true);
    type("grace@example.com");
    expect(list().hidden).toBe(true);
  });

  it("takes a suggestion with arrows and Enter, and lets Enter through when none is highlighted", () => {
    const onInput = vi.fn();
    input.addEventListener("input", onInput);
    type("ada");
    onInput.mockClear();
    const plainEnter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    input.dispatchEvent(plainEnter);
    expect(plainEnter.defaultPrevented).toBe(false);
    press("ArrowDown");
    press("ArrowDown");
    expect(input.getAttribute("aria-activedescendant")).toBe(list().children[1]!.id);
    press("Enter");
    expect(input.value).toBe("ada@work.example");
    expect(onInput).toHaveBeenCalledTimes(1);
    expect(list().hidden).toBe(true);
  });

  it("picks on click and closes on Escape", () => {
    type("g");
    (list().children[0] as HTMLElement).click();
    expect(input.value).toBe("grace@example.com");
    type("a");
    press("Escape");
    expect(list().hidden).toBe(true);
  });
});
