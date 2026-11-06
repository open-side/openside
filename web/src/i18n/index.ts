import { en, ko, type MessageKey } from "./catalog";
import { createTranslator, isLocale, resolveLocale, setLocale, type LocalePreference, type Params } from "./core";

export type { MessageKey } from "./catalog";
export { formatDate, formatList, formatNumber, formatRelativeTime, getLocale, type Locale, type LocalePreference } from "./core";

export const t = createTranslator({ en, ko });

/** Looks up a message by a key computed at runtime (a daemon code); unknown keys return `fallback`. */
export function tMaybe(key: string, params?: Params, fallback?: string): string | undefined {
  return key in en ? t(key as MessageKey, params) : fallback;
}

const PREFERENCE_KEY = "openside.locale";

export function loadLocalePreference(): LocalePreference {
  try {
    const stored = localStorage.getItem(PREFERENCE_KEY);
    return isLocale(stored) ? stored : "system";
  } catch {
    return "system";
  }
}

/** Saves the choice and switches the page now; the app re-renders through useLocale. */
export function saveLocalePreference(preference: LocalePreference): void {
  try {
    if (preference === "system") localStorage.removeItem(PREFERENCE_KEY);
    else localStorage.setItem(PREFERENCE_KEY, preference);
  } catch {
    // Storage can be unavailable (private mode); the choice still applies to this page.
  }
  setLocale(resolveLocale(preference));
}

export function applyLocalePreference(): void {
  setLocale(resolveLocale(loadLocalePreference()));
}
