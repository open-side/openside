import { Fragment, useSyncExternalStore, type ReactNode } from "react";
import { en, ko, type MessageKey } from "./catalog";
import { formatParts, getLocale, subscribeLocale, type Locale } from "./core";

/** Re-renders the caller when the language changes. */
export function useLocale(): Locale {
  return useSyncExternalStore(subscribeLocale, getLocale, getLocale);
}

/** A message with elements in its slots, e.g. `tNodes("x", { key: <kbd>@</kbd> })`. */
export function tNodes(key: MessageKey, params: Record<string, ReactNode | number>): ReactNode {
  const message = (getLocale() === "ko" ? ko : en)[key] ?? en[key];
  return formatParts<ReactNode>(message, params).map((part, index) => <Fragment key={index}>{part}</Fragment>);
}
