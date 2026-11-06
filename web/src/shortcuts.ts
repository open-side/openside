import { useEffect } from "react";
import { useNavigate } from "react-router";

export const PHONE = "(max-width: 820px)";
const COMPOSER = ".composer textarea";

export type Shortcut = "home" | "focus-composer" | "sidebar" | "search" | "settings";
/** The shortcuts Settings › Appearance lets a person change; Settings itself stays on Mod+,. */
export type Rebindable = Exclude<Shortcut, "settings">;
export const REBINDABLE: readonly Rebindable[] = ["sidebar", "search", "focus-composer", "home"];

/** A changed shortcut: Mod (⌘ on Apple devices, Ctrl elsewhere) plus optional Alt/Shift and one key. */
export interface Chord {
  code: string;
  shift: boolean;
  alt: boolean;
}

const BINDINGS_KEY = "openside.shortcuts";
export const BINDINGS_EVENT = "openside:shortcuts";
const DEFAULT_CHORDS: Record<Rebindable | "settings", Chord> = {
  sidebar: { code: "KeyS", shift: false, alt: false },
  search: { code: "KeyA", shift: true, alt: false },
  "focus-composer": { code: "KeyE", shift: false, alt: false },
  home: { code: "KeyE", shift: true, alt: false },
  settings: { code: "Comma", shift: false, alt: false },
};
// Chromium keeps the first five for itself before a page sees them, with or without Shift (Mod+Shift+T
// reopens a tab, Mod+Shift+N/W/Q/Tab open a private window, close or quit, go back a tab), so a binding
// there would never fire. The rest are browser basics (address bar, reload, find, print, bookmark,
// history, downloads, tab by number) a binding must not take away. Codes match whatever the modifiers.
const RESERVED_CODES = new Set(["KeyT", "KeyW", "KeyN", "KeyQ", "Tab", "KeyL", "KeyR", "KeyF", "KeyP", "KeyD", "KeyH", "KeyJ", ...Array.from({ length: 9 }, (_, i) => `Digit${i + 1}`)]);
const MODIFIER_CODES = /^(Shift|Control|Alt|Meta|OS)(Left|Right)?$|^(CapsLock|Fn)$/;

type Bindings = Partial<Record<Rebindable, Chord>>;
let cached: Bindings | null = null;

function validChord(value: unknown): value is Chord {
  if (!value || typeof value !== "object") return false;
  const chord = value as Record<string, unknown>;
  return typeof chord.code === "string" && chord.code !== "" && typeof chord.shift === "boolean" && typeof chord.alt === "boolean";
}

/** Changed shortcuts in this browser; a shortcut missing here keeps its default keys. */
export function loadBindings(): Bindings {
  if (cached) return cached;
  const next: Bindings = {};
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(BINDINGS_KEY) ?? "null");
    if (stored && typeof stored === "object") {
      for (const name of REBINDABLE) {
        const chord = (stored as Record<string, unknown>)[name];
        if (validChord(chord)) next[name] = { code: chord.code, shift: chord.shift, alt: chord.alt };
      }
    }
  } catch {
    // A damaged entry falls back to the defaults.
  }
  cached = next;
  return next;
}

/** `null` restores the default keys. Every open screen showing the keys follows at once. */
export function saveBinding(shortcut: Rebindable, chord: Chord | null): void {
  const next = { ...loadBindings() };
  if (chord) next[shortcut] = chord;
  else delete next[shortcut];
  if (Object.keys(next).length === 0) localStorage.removeItem(BINDINGS_KEY);
  else localStorage.setItem(BINDINGS_KEY, JSON.stringify(next));
  cached = next;
  window.dispatchEvent(new Event(BINDINGS_EVENT));
}

if (typeof window !== "undefined") {
  window.addEventListener("storage", (event) => {
    if (event.key !== BINDINGS_KEY && event.key !== null) return;
    cached = null;
    window.dispatchEvent(new Event(BINDINGS_EVENT));
  });
}

const sameChord = (a: Chord, b: Chord) => a.code === b.code && a.shift === b.shift && a.alt === b.alt;

/** The chord a key press spells, or null without Mod or while only modifiers are down. */
export function chordFromEvent(event: KeyboardEvent): Chord | null {
  if (event.isComposing || event.keyCode === 229) return null;
  if (!(event.metaKey || event.ctrlKey) || MODIFIER_CODES.test(event.code) || event.code === "") return null;
  return { code: event.code, shift: event.shiftKey, alt: event.altKey };
}

export type ChordProblem = { kind: "reserved" } | { kind: "taken"; by: Rebindable | "settings" };

/** Why `chord` cannot become `shortcut`'s keys, or null when it can. */
export function chordProblem(shortcut: Rebindable, chord: Chord): ChordProblem | null {
  if (RESERVED_CODES.has(chord.code)) return { kind: "reserved" };
  const bindings = loadBindings();
  for (const other of [...REBINDABLE, "settings" as const]) {
    if (other === shortcut) continue;
    const current = other === "settings" ? DEFAULT_CHORDS.settings : (bindings[other] ?? DEFAULT_CHORDS[other]);
    if (sameChord(current, chord)) return { kind: "taken", by: other };
  }
  return null;
}

const CODE_LABELS: Record<string, string> = {
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backslash: "\\",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Semicolon: ";",
  Quote: "'",
  Backquote: "`",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
};

function codeLabel(code: string): string {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^Numpad\d$/.test(code)) return code.slice(6);
  return CODE_LABELS[code] ?? code;
}

export function chordKeys(chord: Chord): string[] {
  return ["Mod", ...(chord.alt ? ["Alt"] : []), ...(chord.shift ? ["Shift"] : []), codeLabel(chord.code)];
}

/** The keys `shortcut` answers to right now, in `keyLabel` form. */
export function shortcutKeys(shortcut: Rebindable): readonly string[] {
  return chordKeys(loadBindings()[shortcut] ?? DEFAULT_CHORDS[shortcut]);
}

function editable(target: EventTarget | null): target is HTMLElement {
  if (!(target instanceof HTMLElement)) return false;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  return target.isContentEditable || target.closest('[contenteditable]:not([contenteditable="false"])') !== null;
}

/**
 * openside's keyboard map. `event.code`, so it holds on any keyboard layout; nothing while an IME
 * is composing. Mod+Shift+E, Mod+E, Mod+, (Settings) and search are app-wide. Mod+S is left alone
 * inside any other text field, where the browser's own Save belongs, except the message box, the
 * screen's main input. Search is Mod+Shift+A. Chromium binds that key to its tab search, but not as
 * a reserved key, so a page sees it first; on any other tab the extension's palette command, which
 * tools/openside-browser binds to the same key, brings this app forward with its palette open.
 * Ctrl+Shift+K still opens search off Apple devices for a browser that keeps Ctrl+Shift+A to itself.
 */
export function shortcutFor(event: KeyboardEvent, apple = applePlatform()): Shortcut | null {
  // 229: the key a composing IME reports where `isComposing` is not yet set.
  if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return null;
  if (!(event.metaKey || event.ctrlKey)) return null;
  const bindings = loadBindings();
  const pressed = chordFromEvent(event);
  const bound = pressed && !event.repeat ? REBINDABLE.find((name) => bindings[name] && sameChord(bindings[name], pressed)) : undefined;
  if (bound) return bound === "sidebar" && editable(event.target) && !event.target.matches(COMPOSER) ? null : bound;
  if (event.altKey) return null;
  // A changed shortcut no longer answers to its default keys.
  const free = (name: Rebindable): Rebindable | null => (bindings[name] ? null : name);
  if (event.code === "KeyE") return free(event.shiftKey ? "home" : "focus-composer");
  if (event.shiftKey && !event.repeat && (event.code === "KeyA" || (event.code === "KeyK" && !apple && event.ctrlKey && !event.metaKey))) return free("search");
  if (event.code === "Comma" && !event.shiftKey && !event.repeat) return "settings";
  if (event.code === "KeyS" && !event.shiftKey && !event.repeat) {
    if (editable(event.target) && !event.target.matches(COMPOSER)) return null;
    return free("sidebar");
  }
  return null;
}

function applePlatform(): boolean {
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  return /mac|iphone|ipad|ipod/i.test(nav.userAgentData?.platform || nav.platform || nav.userAgent);
}

const APPLE_GLYPHS: Record<string, string> = { Mod: "⌘", Shift: "⇧", Alt: "⌥", Enter: "↩" };
// Apple writes modifiers as ⌥⇧⌘ before the key.
const APPLE_ORDER: Record<string, number> = { Alt: 0, Shift: 1, Mod: 2 };

/** Keys as people read them here: "⇧⌘E" on Apple devices, "Ctrl+Shift+E" elsewhere. */
export function keyLabel(keys: readonly string[], apple = applePlatform()): string {
  if (apple) {
    const ordered = [...keys].sort((a, b) => (APPLE_ORDER[a] ?? 3) - (APPLE_ORDER[b] ?? 3));
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
      else if (shortcut === "settings") navigate("/settings");
      else return;
      event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, toggleSidebar, openDrawer, openSearch]);
}
