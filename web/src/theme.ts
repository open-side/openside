// public/theme-boot.js mirrors applyTheme/applyAccent/applyPalettes so the first paint already
// has the stored appearance (CSP allows only same-origin script files, no inline boot script).

import { THEME_PRESETS } from "./theme-presets";

export type ThemePreference = "system" | "light" | "dark";
export type Scheme = "light" | "dark";

const STORAGE_KEY = "openside.theme";
const ACCENT_KEY = "openside.accent";
const PALETTE_KEY = "openside.palette";

/** openside's default blue first; the rest are the aside-relay accent swatches. */
export const DEFAULT_ACCENT = "#2f6fed";
export const ACCENT_COLORS: { name: string; value: string }[] = [
  { name: "Blue", value: DEFAULT_ACCENT },
  { name: "Purple", value: "#a34dff" },
  { name: "Green", value: "#00a240" },
  { name: "Yellow", value: "#f0b429" },
  { name: "Orange", value: "#f97316" },
  { name: "Pink", value: "#ef476f" },
  { name: "Teal", value: "#00a6a6" },
  { name: "Gray", value: "#888888" },
];

/** The palette openside ships with; every other id names a preset in theme-presets.ts. */
export const DEFAULT_PALETTE = "openside";

export type ThemePreset = (typeof THEME_PRESETS)[number];

export function presetsFor(scheme: Scheme): readonly ThemePreset[] {
  return THEME_PRESETS.filter((preset) => preset.scheme === scheme);
}

const CANVAS = { light: "#ffffff", dark: "#000000" } as const;
const HEX = /^#[0-9a-f]{6}$/i;

export function loadTheme(storage: Storage = localStorage): ThemePreference {
  const stored = storage.getItem(STORAGE_KEY);
  return stored === "light" || stored === "dark" ? stored : "system";
}

/** `light` or `dark` as painted right now. */
export function resolvedTheme(preference: ThemePreference): Scheme {
  if (preference !== "system") return preference;
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function syncThemeColor(root: HTMLElement, scheme: Scheme) {
  const canvas = getComputedStyle(root).getPropertyValue("--canvas").trim();
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", HEX.test(canvas) ? canvas : CANVAS[scheme]);
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
  syncThemeColor(root, resolvedTheme(preference));
}

/** The custom accent, or `""` when the palette's own accent applies ("Theme default"). */
export function loadAccent(storage: Storage = localStorage): string {
  const stored = storage.getItem(ACCENT_KEY);
  return stored && HEX.test(stored) ? stored.toLowerCase() : "";
}

/**
 * Text on the accent must stay legible for any swatch: pick black or white by WCAG relative
 * luminance (yellow gets black text, blue gets white).
 */
export function accentForeground(hex: string): "#000000" | "#ffffff" {
  const channel = (offset: number) => {
    const value = parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  const luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
  return luminance > 0.4 ? "#000000" : "#ffffff";
}

/** A custom accent overrides every palette; `""` restores the palette's own accent. */
export function applyAccent(color: string, root: HTMLElement = document.documentElement, storage: Storage = localStorage): void {
  if (!HEX.test(color)) {
    root.style.removeProperty("--primary");
    root.style.removeProperty("--primary-foreground");
    storage.removeItem(ACCENT_KEY);
    return;
  }
  const accent = color.toLowerCase();
  root.style.setProperty("--primary", accent);
  root.style.setProperty("--primary-foreground", accentForeground(accent));
  storage.setItem(ACCENT_KEY, accent);
}

export type Palettes = Record<Scheme, string>;

/** Stored palette choices; unknown ids or a preset of the wrong scheme fall back to the default. */
export function loadPalettes(storage: Storage = localStorage): Palettes {
  let stored: unknown = null;
  try {
    stored = JSON.parse(storage.getItem(PALETTE_KEY) ?? "null");
  } catch {
    stored = null;
  }
  const pick = (scheme: Scheme) => {
    const entry = stored && typeof stored === "object" ? Reflect.get(stored, scheme) : null;
    const id = entry && typeof entry === "object" ? Reflect.get(entry, "id") : null;
    return presetsFor(scheme).some((preset) => preset.id === id) ? String(id) : DEFAULT_PALETTE;
  };
  return { light: pick("light"), dark: pick("dark") };
}

/**
 * Writes each scheme's preset colours as --light-* / --dark-* custom properties (styles.css
 * derives the other surfaces from them) and stores the resolved colours, so theme-boot.js can
 * repaint them before React mounts without a copy of the preset table.
 */
export function applyPalettes(palettes: Palettes, root: HTMLElement = document.documentElement, storage: Storage = localStorage): void {
  const stored: Partial<Record<Scheme, { id: string; background: string; foreground: string; accent: string }>> = {};
  for (const scheme of ["light", "dark"] as const) {
    const preset = presetsFor(scheme).find((item) => item.id === palettes[scheme]);
    const flag = scheme === "light" ? "lightPalette" : "darkPalette";
    const names = ["canvas", "text", "accent", "accent-fg"];
    if (!preset) {
      delete root.dataset[flag];
      names.forEach((name) => root.style.removeProperty(`--${scheme}-${name}`));
      continue;
    }
    const values: Record<string, string> = {
      canvas: preset.background,
      text: preset.foreground,
      accent: preset.accent,
      "accent-fg": accentForeground(preset.accent),
    };
    names.forEach((name) => root.style.setProperty(`--${scheme}-${name}`, values[name] ?? ""));
    root.dataset[flag] = preset.id;
    stored[scheme] = { id: preset.id, background: preset.background, foreground: preset.foreground, accent: preset.accent };
  }
  if (stored.light || stored.dark) storage.setItem(PALETTE_KEY, JSON.stringify(stored));
  else storage.removeItem(PALETTE_KEY);
  syncThemeColor(root, resolvedTheme(loadTheme(storage)));
}
