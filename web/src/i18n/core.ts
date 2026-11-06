// Locale state, message formatting and Intl helpers shared by the web app and the extension.
// Catalogs live elsewhere (web/src/i18n/catalog.ts, extension/src/i18n.ts); this module holds
// the one active locale per bundle so shared renderers (web/src/cards) follow the page's choice.

export const LOCALES = ["en", "ko"] as const;
export type Locale = (typeof LOCALES)[number];
/** `system` follows the browser's language list. */
export type LocalePreference = Locale | "system";

/** Intl.PluralRules categories; English uses one/other, Korean only other. */
export type PluralMessage = { zero?: string; one?: string; two?: string; few?: string; many?: string; other: string };
export type Message = string | PluralMessage;
export type Messages = Record<string, Message>;
/** The shape a translation of `E` must have: every key, plural where the source is plural. */
export type Catalog<E extends Messages> = { [K in keyof E]: E[K] extends string ? string : PluralMessage };
export type Params = Record<string, string | number>;

export function isLocale(value: unknown): value is Locale {
  return typeof value === "string" && (LOCALES as readonly string[]).includes(value);
}

/** The browser's language list; some browsers report an empty `languages` but still set `language`. */
export function browserLanguages(nav: Pick<Navigator, "languages" | "language"> | undefined = typeof navigator === "undefined" ? undefined : navigator): readonly string[] {
  if (!nav) return [];
  if (nav.languages?.length) return nav.languages;
  return nav.language ? [nav.language] : [];
}

/** The first browser language we ship wins; anything else falls back to English. */
export function systemLocale(languages: readonly string[] = browserLanguages()): Locale {
  for (const tag of languages) {
    const base = tag.toLowerCase().split(/[-_]/)[0];
    if (isLocale(base)) return base;
  }
  return "en";
}

export function resolveLocale(preference: LocalePreference, languages?: readonly string[]): Locale {
  return preference === "system" ? systemLocale(languages) : preference;
}

let current: Locale = systemLocale();
const listeners = new Set<() => void>();

export function getLocale(): Locale {
  return current;
}

export function setLocale(locale: Locale): void {
  if (typeof document !== "undefined") document.documentElement.lang = locale;
  if (locale === current) return;
  current = locale;
  for (const listener of [...listeners]) listener();
}

export function subscribeLocale(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

const PLACEHOLDER = /\{(\w+)\}/g;

function pick(message: Message, params: Params | undefined, locale: Locale): string {
  if (typeof message === "string") return message;
  const count = params?.count;
  if (typeof count !== "number") return message.other;
  if (count === 0 && message.zero !== undefined) return message.zero;
  return message[new Intl.PluralRules(locale).select(count)] ?? message.other;
}

function display(value: string | number, locale: Locale): string {
  return typeof value === "number" ? new Intl.NumberFormat(locale).format(value) : value;
}

/** Placeholder names a message uses, across every plural form. */
export function placeholders(message: Message): Set<string> {
  const forms = typeof message === "string" ? [message] : Object.values(message);
  const names = new Set<string>();
  for (const form of forms) for (const match of form.matchAll(PLACEHOLDER)) names.add(match[1]!);
  return names;
}

/** Splits a message into text and parameter slots so callers can interleave elements (links, <kbd>). */
export function formatParts<T>(message: Message, params: Record<string, T | string | number>, locale: Locale = current): (string | T)[] {
  const numeric: Params = {};
  for (const [name, value] of Object.entries(params)) if (typeof value === "number") numeric[name] = value;
  const template = pick(message, numeric, locale);
  const parts: (string | T)[] = [];
  let last = 0;
  for (const match of template.matchAll(PLACEHOLDER)) {
    const index = match.index ?? 0;
    if (index > last) parts.push(template.slice(last, index));
    const value = params[match[1]!];
    if (value === undefined) parts.push(match[0]);
    else parts.push(typeof value === "number" || typeof value === "string" ? display(value, locale) : value);
    last = index + match[0].length;
  }
  if (last < template.length) parts.push(template.slice(last));
  return parts;
}

export function format(message: Message, params?: Params, locale: Locale = current): string {
  const template = pick(message, params, locale);
  if (!params) return template;
  return template.replace(PLACEHOLDER, (whole, name: string) => {
    const value = params[name];
    return value === undefined ? whole : display(value, locale);
  });
}

export type Translator<E extends Messages> = (key: keyof E & string, params?: Params) => string;

/** Binds a catalog pair; a key missing from the active catalog falls back to English. */
export function createTranslator<E extends Messages>(catalogs: { en: E; ko: Catalog<E> }): Translator<E> {
  return (key, params) => {
    const message: Message | undefined = (catalogs[current] as Messages)[key] ?? catalogs.en[key];
    return message === undefined ? key : format(message, params);
  };
}

export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(current, options).format(value);
}

export function formatDate(value: Date | number, options?: Intl.DateTimeFormatOptions): string {
  return new Intl.DateTimeFormat(current, options).format(value);
}

export function formatRelativeTime(value: number, unit: Intl.RelativeTimeFormatUnit, options?: Intl.RelativeTimeFormatOptions): string {
  return new Intl.RelativeTimeFormat(current, options).format(value, unit);
}

export function formatList(items: readonly string[], options?: Intl.ListFormatOptions): string {
  return new Intl.ListFormat(current, options).format(items);
}
