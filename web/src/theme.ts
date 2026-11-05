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

const CANVAS = { light: "#ffffff", dark: "#1f1f1f" } as const;
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

/** The canvas a scheme paints, from its stored palette when it is not the one on screen. */
function canvasOf(scheme: Scheme, palettes: Palettes): string {
  const preset = presetsFor(scheme).find((item) => item.id === palettes[scheme]);
  return preset?.background ?? CANVAS[scheme];
}

/**
 * index.html carries one theme-color meta per scheme (media query) so the browser chrome is right
 * before any script runs. A System preference keeps each meta on its own scheme; an explicit
 * Light/Dark pins both to the painted canvas, because the media query would follow the OS instead.
 */
function syncThemeColor(root: HTMLElement, scheme: Scheme, preference: ThemePreference, palettes: Palettes) {
  const painted = getComputedStyle(root).getPropertyValue("--canvas").trim();
  const current = HEX.test(painted) ? painted : canvasOf(scheme, palettes);
  document.querySelectorAll('meta[name="theme-color"]').forEach((meta) => {
    const own: Scheme = /dark/.test(meta.getAttribute("media") ?? "") ? "dark" : "light";
    meta.setAttribute("content", preference === "system" && own !== scheme ? canvasOf(own, palettes) : current);
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
  syncThemeColor(root, resolvedTheme(preference), preference, loadPalettes(storage));
}

/** The custom accent, or `""` when the palette's own accent applies ("Theme default"). */
export function loadAccent(storage: Storage = localStorage): string {
  const stored = storage.getItem(ACCENT_KEY);
  return stored && HEX.test(stored) ? stored.toLowerCase() : "";
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

/** `color-mix(in srgb, a <share>, b)` as a hex colour. */
function mix(a: string, b: string, share: number): string {
  const at = (hex: string, offset: number) => parseInt(hex.slice(offset, offset + 2), 16);
  return `#${[1, 3, 5].map((offset) => Math.round(at(a, offset) * share + at(b, offset) * (1 - share)).toString(16).padStart(2, "0")).join("")}`;
}

/** The canvas, then the surface (94%) and raised (90%) colours tokens.css mixes from it and the text. */
function derivedSurfaces(canvas: string, text: string): string[] {
  return [canvas, mix(canvas, text, 0.94), mix(canvas, text, 0.9)];
}

/**
 * Body text for a palette preset. Some presets ship a foreground under 4.5:1 (Solarized light);
 * this moves it toward black or white until it reads at 4.5:1 on every surface tokens.css
 * derives from it. A preset that already passes keeps its own colour.
 */
export function readableText(canvas: string, text: string): string {
  const extreme = luminance(canvas) > 0.18 ? "#000000" : "#ffffff";
  for (let share = 0; share < 100; share += 5) {
    const candidate = mix(extreme, text, share / 100);
    if (derivedSurfaces(canvas, candidate).every((surface) => contrastRatio(candidate, surface) >= 4.5)) return candidate;
  }
  return extreme;
}

/**
 * Secondary text for a palette preset, given its (already readable) text. tokens.css would
 * derive it as 38% canvas into the text, which loses AA on low-contrast presets; this walks the
 * mix toward the text until it reads at 4.5:1 on every derived surface, and falls back to the
 * text itself.
 */
export function readableMuted(canvas: string, text: string): string {
  const surfaces = derivedSurfaces(canvas, text);
  for (let share = 38; share > 0; share -= 2) {
    const muted = mix(canvas, text, share / 100);
    if (surfaces.every((surface) => contrastRatio(muted, surface) >= 4.5)) return muted;
  }
  return text.toLowerCase();
}

/**
 * Text on the accent must stay legible for any swatch: pick black or white by WCAG relative
 * luminance (yellow gets black text, blue gets white).
 */
export function accentForeground(hex: string): "#000000" | "#ffffff" {
  return luminance(hex) > 0.4 ? "#000000" : "#ffffff";
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
 * Writes each scheme's preset colours as --light-* / --dark-* custom properties (tokens.css
 * derives the other surfaces from them) and stores the resolved colours, so theme-boot.js can
 * repaint them before React mounts without a copy of the preset table.
 */
export function applyPalettes(palettes: Palettes, root: HTMLElement = document.documentElement, storage: Storage = localStorage): void {
  const stored: Partial<Record<Scheme, { id: string; background: string; foreground: string; accent: string; muted: string }>> = {};
  for (const scheme of ["light", "dark"] as const) {
    const preset = presetsFor(scheme).find((item) => item.id === palettes[scheme]);
    const flag = scheme === "light" ? "lightPalette" : "darkPalette";
    const names = ["canvas", "text", "muted", "accent", "accent-fg"];
    if (!preset) {
      delete root.dataset[flag];
      names.forEach((name) => root.style.removeProperty(`--${scheme}-${name}`));
      continue;
    }
    const text = readableText(preset.background, preset.foreground);
    const muted = readableMuted(preset.background, text);
    const values: Record<string, string> = {
      canvas: preset.background,
      text,
      muted,
      accent: preset.accent,
      "accent-fg": accentForeground(preset.accent),
    };
    names.forEach((name) => root.style.setProperty(`--${scheme}-${name}`, values[name] ?? ""));
    root.dataset[flag] = preset.id;
    // The boot script paints `foreground` as --*-text, so it stores the clamped colour.
    stored[scheme] = { id: preset.id, background: preset.background, foreground: text, accent: preset.accent, muted };
  }
  if (stored.light || stored.dark) storage.setItem(PALETTE_KEY, JSON.stringify(stored));
  else storage.removeItem(PALETTE_KEY);
  const preference = loadTheme(storage);
  syncThemeColor(root, resolvedTheme(preference), preference, palettes);
}
