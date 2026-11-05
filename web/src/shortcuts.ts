import { useEffect } from "react";
import { useNavigate } from "react-router";

export const PHONE = "(max-width: 820px)";
const COMPOSER = ".composer textarea";

export type Shortcut = "home" | "focus-composer" | "sidebar";

function editable(target: EventTarget | null): target is HTMLElement {
  if (!(target instanceof HTMLElement)) return false;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  return target.isContentEditable || target.closest('[contenteditable]:not([contenteditable="false"])') !== null;
}

/**
 * Aside's keyboard map. `event.code`, so it holds on any keyboard layout; nothing while an IME
 * is composing. Mod+Shift+E and Mod+E are app-wide. Mod+S is left alone inside any other text
 * field, where the browser's own Save belongs, except the message box, the screen's main input.
 */
export function shortcutFor(event: KeyboardEvent): Shortcut | null {
  // 229: the key a composing IME reports where `isComposing` is not yet set.
  if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return null;
  if (event.altKey || !(event.metaKey || event.ctrlKey)) return null;
  if (event.code === "KeyE") return event.shiftKey ? "home" : "focus-composer";
  if (event.code === "KeyS" && !event.shiftKey && !event.repeat) {
    if (editable(event.target) && !event.target.matches(COMPOSER)) return null;
    return "sidebar";
  }
  return null;
}

/** Binds `shortcutFor` while the shell is mounted; the browser keeps every key it does not handle. */
export function useShortcuts(toggleSidebar: () => void, openDrawer: () => void) {
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
      } else return;
      event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, toggleSidebar, openDrawer]);
}
