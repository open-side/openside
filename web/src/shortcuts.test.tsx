import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { keyLabel, PHONE, shortcutFor, shortcutKeys, useShortcuts } from "./shortcuts";

function Where() {
  return <output aria-label="path">{useLocation().pathname}</output>;
}

function Harness({ toggleSidebar, openDrawer, openSearch, composer = true }: { toggleSidebar: () => void; openDrawer: () => void; openSearch: () => void; composer?: boolean }) {
  useShortcuts({ toggleSidebar, openDrawer, openSearch });
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
  const openSearch = vi.fn();
  render(
    <MemoryRouter initialEntries={["/sessions/s1"]}>
      <Harness toggleSidebar={toggleSidebar} openDrawer={openDrawer} openSearch={openSearch} composer={composer} />
    </MemoryRouter>,
  );
  return { toggleSidebar, openDrawer, openSearch, path: () => screen.getByRole("status", { name: "path" }).textContent };
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
    expect(path()).toBe("/sessions/s1");
  });

  it("Mod+Shift+A opens search with Meta or Ctrl, from a text field and the message box too", () => {
    const { openSearch, path } = setup();
    expect(press(document.body, { code: "KeyA", key: "A", metaKey: true, shiftKey: true })).toBe(true);
    expect(press(screen.getByLabelText("Search"), { code: "KeyA", key: "A", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(press(screen.getByRole("textbox", { name: "Message" }), { code: "KeyA", key: "A", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(openSearch).toHaveBeenCalledTimes(3);
    expect(path()).toBe("/sessions/s1");
  });

  it("Ctrl+Shift+K still opens search off Apple devices, for a browser that keeps Ctrl+Shift+A to itself", () => {
    const { openSearch } = setup();
    expect(press(document.body, { code: "KeyK", key: "K", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(press(screen.getByLabelText("Search"), { code: "KeyK", key: "K", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(openSearch).toHaveBeenCalledTimes(2);
    // Ctrl+K is the browser's own search box; Cmd+Shift+K is not ours either.
    expect(press(document.body, { code: "KeyK", key: "k", ctrlKey: true })).toBe(false);
    expect(press(document.body, { code: "KeyK", key: "K", metaKey: true, shiftKey: true })).toBe(false);
    expect(press(document.body, { code: "KeyK", key: "K", ctrlKey: true, shiftKey: true, repeat: true })).toBe(false);
    expect(press(document.body, { code: "KeyK", key: "Process", ctrlKey: true, shiftKey: true, keyCode: 229 })).toBe(false);
    expect(openSearch).toHaveBeenCalledTimes(2);
  });

  it("on Apple devices search is ⇧⌘A only; Ctrl+Shift+K is left to the browser", () => {
    const chord = (init: KeyboardEventInit) => shortcutFor(new KeyboardEvent("keydown", init), true);
    expect(chord({ code: "KeyA", metaKey: true, shiftKey: true })).toBe("search");
    expect(chord({ code: "KeyK", ctrlKey: true, shiftKey: true })).toBeNull();
    expect(shortcutFor(new KeyboardEvent("keydown", { code: "KeyK", ctrlKey: true, shiftKey: true }), false)).toBe("search");
  });

  it("leaves Mod+A, held repeats, Alt and composing IME keys to the browser instead of opening search", () => {
    const { openSearch } = setup();
    expect(press(document.body, { code: "KeyA", key: "a", ctrlKey: true })).toBe(false);
    expect(press(document.body, { code: "KeyA", key: "A", ctrlKey: true, shiftKey: true, repeat: true })).toBe(false);
    expect(press(document.body, { code: "KeyA", key: "A", ctrlKey: true, shiftKey: true, altKey: true })).toBe(false);
    expect(press(document.body, { code: "KeyA", key: "A", ctrlKey: true, shiftKey: true, isComposing: true })).toBe(false);
    expect(press(document.body, { code: "KeyA", key: "Process", ctrlKey: true, shiftKey: true, keyCode: 229 })).toBe(false);
    expect(press(document.body, { code: "KeyA", key: "A", shiftKey: true })).toBe(false);
    expect(openSearch).not.toHaveBeenCalled();
  });

  it("Mod+, opens Settings from anywhere, a text field included, and leaves Mod+Shift+, alone", () => {
    const { path } = setup();
    expect(press(screen.getByLabelText("Search"), { code: "Comma", key: ",", shiftKey: true, ctrlKey: true })).toBe(false);
    expect(path()).toBe("/sessions/s1");
    expect(press(screen.getByLabelText("Search"), { code: "Comma", key: ",", metaKey: true })).toBe(true);
    expect(path()).toBe("/settings");
  });
});

describe("shortcut labels", () => {
  it("writes chords with Apple glyphs on Apple devices and Ctrl elsewhere", () => {
    expect(keyLabel(["Mod", "Shift", "A"], true)).toBe("⇧⌘A");
    expect(keyLabel(["Mod", "Shift", "A"], false)).toBe("Ctrl+Shift+A");
    expect(keyLabel(["Mod", "S"], true)).toBe("⌘S");
    expect(keyLabel(["Shift", "Enter"], false)).toBe("Shift+Enter");
  });

  it("shows Mod+Shift+A for search on every platform, or the person's own chord", () => {
    expect(keyLabel(shortcutKeys("search"), true)).toBe("⇧⌘A");
    expect(keyLabel(shortcutKeys("search"), false)).toBe("Ctrl+Shift+A");
  });
});
