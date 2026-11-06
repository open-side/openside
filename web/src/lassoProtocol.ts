/*
 * Settings › Lasso, shared by the web UI and the openside MV3 extension (which imports this file).
 * The extension owns the settings: the selection dot, the lasso overlay and their hotkey run in the
 * person's browser, so the web page reads and writes them over `externally_connectable`.
 */

/** The modifier pressed twice to start a lasso (Aside's ⌥ + ⌥). */
export type LassoHotkey = "alt" | "control" | "shift";
export const LASSO_HOTKEYS: readonly LassoHotkey[] = ["alt", "control", "shift"];

/** Shortcuts openside ships; their names and instructions come from each side's catalog. */
export type BuiltinShortcut = "summarize" | "translate" | "copy-code" | "search-image";
export const TEXT_BUILTINS: readonly BuiltinShortcut[] = ["summarize", "translate"];
export const REGION_BUILTINS: readonly BuiltinShortcut[] = ["copy-code", "search-image"];
const BUILTINS: readonly string[] = [...TEXT_BUILTINS, ...REGION_BUILTINS];

/**
 * A built-in keeps its id and has no `label`/`prompt` until the person edits it; an added shortcut
 * has a `custom-…` id and both fields.
 */
export interface LassoShortcut {
  id: string;
  label?: string;
  prompt?: string;
}

export interface LassoSettings {
  text: { enabled: boolean; shortcuts: LassoShortcut[] };
  region: { enabled: boolean; hotkey: LassoHotkey; shortcuts: LassoShortcut[] };
}

export const DEFAULT_LASSO: LassoSettings = {
  text: { enabled: true, shortcuts: [{ id: "summarize" }, { id: "translate" }] },
  region: { enabled: true, hotkey: "alt", shortcuts: [{ id: "copy-code" }, { id: "search-image" }] },
};

export const MAX_SHORTCUTS = 12;
export const MAX_LABEL_CHARS = 40;
export const MAX_PROMPT_CHARS = 1000;

export function isBuiltin(id: string): id is BuiltinShortcut {
  return BUILTINS.includes(id);
}

function text(value: unknown, max: number): string | undefined | null {
  if (value === undefined) return undefined;
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed && [...trimmed].length <= max ? trimmed : null;
}

function shortcuts(value: unknown): LassoShortcut[] | null {
  if (!Array.isArray(value) || value.length > MAX_SHORTCUTS) return null;
  const seen = new Set<string>();
  const parsed: LassoShortcut[] = [];
  for (const raw of value) {
    if (typeof raw !== "object" || raw === null) return null;
    const entry = raw as Record<string, unknown>;
    if (typeof entry.id !== "string" || seen.has(entry.id)) return null;
    const builtin = isBuiltin(entry.id);
    if (!builtin && !/^custom-[a-z0-9]{1,32}$/.test(entry.id)) return null;
    const label = text(entry.label, MAX_LABEL_CHARS);
    const prompt = text(entry.prompt, MAX_PROMPT_CHARS);
    if (label === null || prompt === null || (!builtin && (label === undefined || prompt === undefined))) return null;
    seen.add(entry.id);
    parsed.push({ id: entry.id, ...(label === undefined ? {} : { label }), ...(prompt === undefined ? {} : { prompt }) });
  }
  return parsed;
}

/** The settings when `value` is well formed, else null; both sides check what they receive. */
export function parseLassoSettings(value: unknown): LassoSettings | null {
  if (typeof value !== "object" || value === null) return null;
  const { text: textPart, region } = value as Record<string, unknown>;
  if (typeof textPart !== "object" || textPart === null || typeof region !== "object" || region === null) return null;
  const textSettings = textPart as Record<string, unknown>;
  const regionSettings = region as Record<string, unknown>;
  const textShortcuts = shortcuts(textSettings.shortcuts);
  const regionShortcuts = shortcuts(regionSettings.shortcuts);
  const hotkey = LASSO_HOTKEYS.find((key) => key === regionSettings.hotkey);
  if (typeof textSettings.enabled !== "boolean" || typeof regionSettings.enabled !== "boolean" || !textShortcuts || !regionShortcuts || !hotkey) return null;
  return { text: { enabled: textSettings.enabled, shortcuts: textShortcuts }, region: { enabled: regionSettings.enabled, hotkey, shortcuts: regionShortcuts } };
}

export type LassoRequest =
  | { type: "openside.lasso.get" }
  | { type: "openside.lasso.set"; settings: LassoSettings }
  /** Opens the extension's page that asks the browser for access to every site. */
  | { type: "openside.lasso.access" };

export type LassoReply =
  | {
      ok: true;
      settings: LassoSettings;
      /** The extension may read pages everywhere, which the selection dot and the double-press hotkey need. */
      access: boolean;
      /** The browser's key for the extension's Capture command ("Alt+Shift+L"), or "" when none is assigned. */
      command: string;
    }
  | { ok: false; message: string };
