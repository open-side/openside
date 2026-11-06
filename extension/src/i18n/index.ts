import { createTranslator, formatParts, getLocale, isLocale, resolveLocale, setLocale, type Locale, type LocalePreference } from "../../../web/src/i18n/core";
import { en } from "./en";
import { ko } from "./ko";

export type MessageKey = keyof typeof en;

export const t = createTranslator({ en, ko });

/** Shared by every extension page and the service worker; absent means follow the browser's UI language. */
export const LOCALE_KEY = "openside.locale";

function preferenceOf(value: unknown): LocalePreference {
  return isLocale(value) ? value : "system";
}

export async function loadLocalePreference(): Promise<LocalePreference> {
  return preferenceOf((await chrome.storage.local.get(LOCALE_KEY))[LOCALE_KEY]);
}

/** Every open extension page and the service worker pick the change up through storage.onChanged. */
export async function saveLocalePreference(preference: LocalePreference): Promise<void> {
  if (preference === "system") await chrome.storage.local.remove(LOCALE_KEY);
  else await chrome.storage.local.set({ [LOCALE_KEY]: preference });
}

/**
 * Applies the stored choice to this bundle (so the web renderers it imports follow too) and calls
 * `onChange` after a later change made anywhere in the extension.
 */
export async function initLocale(onChange: () => void): Promise<Locale> {
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area !== "local" || !(LOCALE_KEY in changes)) return;
    setLocale(resolveLocale(preferenceOf(changes[LOCALE_KEY]?.newValue)));
    onChange();
  });
  setLocale(resolveLocale(await loadLocalePreference()));
  return getLocale();
}

/** Static pages mark text with `data-i18n` and attributes with `data-i18n-<attribute>`; children marked `data-slot` fill `{slot}` placeholders. */
const ATTRIBUTES = ["placeholder", "aria-label", "title"] as const;

export function localizeDocument(root: ParentNode = document): void {
  for (const element of root.querySelectorAll<HTMLElement>("[data-i18n]")) {
    const key = element.dataset.i18n as MessageKey;
    const slots = new Map<string, Element>();
    for (const slot of element.querySelectorAll<HTMLElement>("[data-slot]")) slots.set(slot.dataset.slot!, slot);
    if (!slots.size) { element.textContent = t(key); continue; }
    const message = (getLocale() === "ko" ? ko[key] : undefined) ?? en[key];
    element.replaceChildren(...formatParts<Element>(message, Object.fromEntries(slots)));
  }
  for (const attribute of ATTRIBUTES) {
    for (const element of root.querySelectorAll<HTMLElement>(`[data-i18n-${attribute}]`)) element.setAttribute(attribute, t(element.getAttribute(`data-i18n-${attribute}`) as MessageKey));
  }
}
