import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PHONE, useShortcuts } from "./shortcuts";

function Where() {
  return <output aria-label="path">{useLocation().pathname}</output>;
}

function Harness({ toggleSidebar, openDrawer, composer = true }: { toggleSidebar: () => void; openDrawer: () => void; composer?: boolean }) {
  useShortcuts(toggleSidebar, openDrawer);
  return (
    <>
      <Where />
      <input aria-label="Search" />
      <div contentEditable aria-label="Notes" />
      {composer ? (
        <div className="composer">
          <textarea aria-label="Message" />
        </div>
      ) : null}
    </>
  );
}

function setup({ phone = false, composer = true } = {}) {
  vi.stubGlobal("matchMedia", (query: string) => ({ matches: phone && query === PHONE }) as MediaQueryList);
  const toggleSidebar = vi.fn();
  const openDrawer = vi.fn();
  render(
    <MemoryRouter initialEntries={["/agents/mina"]}>
      <Harness toggleSidebar={toggleSidebar} openDrawer={openDrawer} composer={composer} />
    </MemoryRouter>,
  );
  return { toggleSidebar, openDrawer, path: () => screen.getByRole("status", { name: "path" }).textContent };
}

/** Dispatches on `target` and reports whether the page claimed the key. */
function press(target: Element, init: KeyboardEventInit & { keyCode?: number }): boolean {
  return !fireEvent.keyDown(target, init);
}

afterEach(() => vi.unstubAllGlobals());

describe("keyboard shortcuts", () => {
  it("Mod+Shift+E opens Home from anywhere, a text field included", () => {
    const { path } = setup();
    expect(press(screen.getByLabelText("Search"), { code: "KeyE", key: "E", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(path()).toBe("/");
  });

  it("Mod+E focuses the message box with Meta or Ctrl, and is left to the browser without one", () => {
    setup();
    expect(press(document.body, { code: "KeyE", key: "e", metaKey: true })).toBe(true);
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus();
    screen.getByLabelText("Search").focus();
    expect(press(screen.getByLabelText("Search"), { code: "KeyE", key: "e", ctrlKey: true })).toBe(true);
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus();
  });

  it("leaves Mod+E to the browser on a screen without a message box", () => {
    setup({ composer: false });
    expect(press(document.body, { code: "KeyE", key: "e", ctrlKey: true })).toBe(false);
  });

  it("Mod+S folds the sidebar on desktop", () => {
    const desktop = setup();
    expect(press(document.body, { code: "KeyS", key: "s", metaKey: true })).toBe(true);
    expect(desktop.toggleSidebar).toHaveBeenCalledTimes(1);
    expect(desktop.openDrawer).not.toHaveBeenCalled();
  });

  it("Mod+S opens the drawer on a phone", () => {
    const phone = setup({ phone: true });
    expect(press(document.body, { code: "KeyS", key: "s", ctrlKey: true })).toBe(true);
    expect(phone.openDrawer).toHaveBeenCalledTimes(1);
    expect(phone.toggleSidebar).not.toHaveBeenCalled();
  });

  it("Mod+S works from the message box but stays the browser's in any other text field", () => {
    const { toggleSidebar } = setup();
    expect(press(screen.getByLabelText("Search"), { code: "KeyS", key: "s", ctrlKey: true })).toBe(false);
    expect(press(screen.getByLabelText("Notes"), { code: "KeyS", key: "s", ctrlKey: true })).toBe(false);
    expect(toggleSidebar).not.toHaveBeenCalled();
    expect(press(screen.getByRole("textbox", { name: "Message" }), { code: "KeyS", key: "s", ctrlKey: true })).toBe(true);
    expect(toggleSidebar).toHaveBeenCalledTimes(1);
  });

  it("ignores keys while an IME is composing, with Alt, held repeats and unhandled keys", () => {
    const { toggleSidebar, path } = setup();
    expect(press(document.body, { code: "KeyE", key: "e", ctrlKey: true, shiftKey: true, isComposing: true })).toBe(false);
    expect(press(document.body, { code: "KeyE", key: "Process", ctrlKey: true, shiftKey: true, keyCode: 229 })).toBe(false);
    expect(press(document.body, { code: "KeyS", key: "s", ctrlKey: true, altKey: true })).toBe(false);
    expect(press(document.body, { code: "KeyS", key: "s", ctrlKey: true, repeat: true })).toBe(false);
    expect(press(document.body, { code: "KeyS", key: "s" })).toBe(false);
    expect(press(document.body, { code: "KeyK", key: "k", ctrlKey: true })).toBe(false);
    expect(toggleSidebar).not.toHaveBeenCalled();
    expect(path()).toBe("/agents/mina");
  });
});
