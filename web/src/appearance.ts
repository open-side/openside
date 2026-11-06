// public/theme-boot.js mirrors applyLook so the first paint already has the stored look.
import "./appearance.css";

export type TabStyle = "sans" | "serif";
export type Transparency = "tinted" | "transparent";

/** Settings › Appearance choices that only change how this browser paints openside. */
export interface Look {
  tabStyle: TabStyle;
  transparency: Transparency;
}

const STORAGE_KEY = "openside.look";
export const DEFAULT_LOOK: Look = { tabStyle: "sans", transparency: "transparent" };

export function loadLook(storage: Storage = localStorage): Look {
  try {
    const stored: unknown = JSON.parse(storage.getItem(STORAGE_KEY) ?? "null");
    if (!stored || typeof stored !== "object") return DEFAULT_LOOK;
    const value = stored as Partial<Record<keyof Look, unknown>>;
    return {
      tabStyle: value.tabStyle === "serif" ? "serif" : DEFAULT_LOOK.tabStyle,
      transparency: value.transparency === "tinted" ? "tinted" : DEFAULT_LOOK.transparency,
    };
  } catch {
    return DEFAULT_LOOK;
  }
}

/** Writes the look to `data-tab-style` / `data-transparency` on the root and remembers it. */
export function applyLook(look: Look, root: HTMLElement = document.documentElement, storage: Storage = localStorage): void {
  root.dataset.tabStyle = look.tabStyle;
  root.dataset.transparency = look.transparency;
  if (look.tabStyle === DEFAULT_LOOK.tabStyle && look.transparency === DEFAULT_LOOK.transparency) storage.removeItem(STORAGE_KEY);
  else storage.setItem(STORAGE_KEY, JSON.stringify(look));
}

export type NewTabMode = "ask" | "search";
const NEW_TAB_MODE_KEY = "openside.newTabMode";

/** Settings › General "New tab mode": how `/` opens when it is the page a new tab loads. Search unless Ask was chosen. */
export function loadNewTabMode(storage: Storage = localStorage): NewTabMode {
  return storage.getItem(NEW_TAB_MODE_KEY) === "ask" ? "ask" : "search";
}

export function saveNewTabMode(mode: NewTabMode, storage: Storage = localStorage): void {
  if (mode === "ask") storage.setItem(NEW_TAB_MODE_KEY, mode);
  else storage.removeItem(NEW_TAB_MODE_KEY);
}
