// public/theme-boot.js mirrors applyTheme so the first paint already has the stored theme
// (CSP allows only same-origin script files, no inline boot script).

export type ThemePreference = "system" | "light" | "dark";
export type Scheme = "light" | "dark";

const STORAGE_KEY = "openside.theme";

/** --canvas per scheme, as in tokens.css; index.html and theme-boot.js carry the same pair. */
const CANVAS = { light: "#f9f9f9", dark: "#1f1f1f" } as const;

export function loadTheme(storage: Storage = localStorage): ThemePreference {
  const stored = storage.getItem(STORAGE_KEY);
  return stored === "light" || stored === "dark" ? stored : "system";
}

/** Keys the removed palette presets and accent picker stored; nothing reads them any more. */
const RETIRED_KEYS = ["openside.palette", "openside.accent"];

export function forgetRetiredThemeKeys(storage: Storage = localStorage): void {
  for (const key of RETIRED_KEYS) storage.removeItem(key);
}

/** `light` or `dark` as painted right now. */
export function resolvedTheme(preference: ThemePreference): Scheme {
  if (preference !== "system") return preference;
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/**
 * index.html carries one theme-color meta per scheme (media query) so the browser chrome is right
 * before any script runs. A System preference keeps each meta on its own scheme; an explicit
 * Light/Dark pins both to that scheme's canvas, because the media query would follow the OS instead.
 */
function syncThemeColor(scheme: Scheme, preference: ThemePreference) {
  document.querySelectorAll('meta[name="theme-color"]').forEach((meta) => {
    const own: Scheme = /dark/.test(meta.getAttribute("media") ?? "") ? "dark" : "light";
    meta.setAttribute("content", CANVAS[preference === "system" ? own : scheme]);
  });
}

/**
 * `system` follows `prefers-color-scheme` through CSS media queries; an explicit
 * choice is written to `data-theme` so CSS can override the media query.
 */
export function applyTheme(
  preference: ThemePreference,
  root: HTMLElement = document.documentElement,
  storage: Storage = localStorage,
): void {
  root.dataset.theme = preference;
  if (preference === "system") storage.removeItem(STORAGE_KEY);
  else storage.setItem(STORAGE_KEY, preference);
  syncThemeColor(resolvedTheme(preference), preference);
}

/** WCAG relative luminance of a `#rrggbb` colour. */
function luminance(hex: string): number {
  const channel = (offset: number) => {
    const value = parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

/** WCAG contrast ratio between two `#rrggbb` colours (1–21). */
export function contrastRatio(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (light + 0.05) / (dark + 0.05);
}
