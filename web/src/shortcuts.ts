import { useEffect } from "react";
import { useNavigate } from "react-router";

export const PHONE = "(max-width: 820px)";
const COMPOSER = ".composer textarea";

export type Shortcut = "home" | "focus-composer" | "sidebar" | "search";

function editable(target: EventTarget | null): target is HTMLElement {
  if (!(target instanceof HTMLElement)) return false;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  return target.isContentEditable || target.closest('[contenteditable]:not([contenteditable="false"])') !== null;
}

/**
 * openside's keyboard map. `event.code`, so it holds on any keyboard layout; nothing while an IME
 * is composing. Mod+Shift+E, Mod+E and search are app-wide. Mod+S is left alone inside any other
 * text field, where the browser's own Save belongs, except the message box, the screen's main
 * input. Search is Mod+Shift+A everywhere, plus Ctrl+Shift+K off Apple devices: there Chromium
 * binds Ctrl+Shift+A to its tab search (not a reserved key in Chromium 155, so the page still sees
 * it first, but other versions, browsers or extensions may claim it), and no Chromium accelerator
 * uses Ctrl+Shift+K.
 */
export function shortcutFor(event: KeyboardEvent, apple = applePlatform()): Shortcut | null {
  // 229: the key a composing IME reports where `isComposing` is not yet set.
  if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return null;
  if (event.altKey || !(event.metaKey || event.ctrlKey)) return null;
  if (event.code === "KeyE") return event.shiftKey ? "home" : "focus-composer";
  if (event.shiftKey && !event.repeat && (event.code === "KeyA" || (event.code === "KeyK" && !apple && event.ctrlKey && !event.metaKey))) return "search";
  if (event.code === "KeyS" && !event.shiftKey && !event.repeat) {
    if (editable(event.target) && !event.target.matches(COMPOSER)) return null;
    return "sidebar";
  }
  return null;
}

/** The search chord to show: ⇧⌘A on Apple devices, Ctrl+Shift+K elsewhere (see `shortcutFor`). */
export function searchKeys(apple = applePlatform()): readonly string[] {
  return apple ? ["Mod", "Shift", "A"] : ["Ctrl", "Shift", "K"];
}

/** What the Settings table lists: only keys this file (or the composer) really handles. */
export function shortcutTable(apple = applePlatform()): readonly { keys: readonly string[]; action: string }[] {
  return [
    { keys: ["Mod", "Shift", "E"], action: "New task (Home)" },
    { keys: ["Mod", "E"], action: "Focus the message box" },
    { keys: ["Mod", "S"], action: "Show or hide the sidebar" },
    { keys: searchKeys(apple), action: "Search tabs, chats and conversations" },
    { keys: ["Enter"], action: "Send a message" },
    { keys: ["Shift", "Enter"], action: "New line in a message" },
  ];
}

function applePlatform(): boolean {
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  return /mac|iphone|ipad|ipod/i.test(nav.userAgentData?.platform || nav.platform || nav.userAgent);
}

const APPLE_GLYPHS: Record<string, string> = { Mod: "⌘", Shift: "⇧", Enter: "↩" };

/** Keys as people read them here: "⇧⌘E" on Apple devices, "Ctrl+Shift+E" elsewhere. */
export function keyLabel(keys: readonly string[], apple = applePlatform()): string {
  if (apple) {
    // Apple's order puts Shift before Command.
    const ordered = [...keys].sort((a, b) => Number(b === "Shift") - Number(a === "Shift"));
    return ordered.map((key) => APPLE_GLYPHS[key] ?? key).join("");
  }
  return keys.map((key) => (key === "Mod" ? "Ctrl" : key)).join("+");
}

export interface ShortcutActions {
  toggleSidebar: () => void;
  openDrawer: () => void;
  openSearch: () => void;
}

/** Binds `shortcutFor` while the shell is mounted; the browser keeps every key it does not handle. */
export function useShortcuts({ toggleSidebar, openDrawer, openSearch }: ShortcutActions) {
  const navigate = useNavigate();
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const shortcut = shortcutFor(event);
      if (shortcut === "home") navigate("/");
      else if (shortcut === "focus-composer") {
        const box = document.querySelector<HTMLTextAreaElement>(COMPOSER);
        if (!box) return;
        box.focus();
      } else if (shortcut === "sidebar") {
        if (window.matchMedia(PHONE).matches) openDrawer();
        else toggleSidebar();
      } else if (shortcut === "search") openSearch();
      else return;
      event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, toggleSidebar, openDrawer, openSearch]);
}
