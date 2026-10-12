import type { TabContext } from "./context";
export type Site = "gmail" | "slack";
export function messageSelector(site: Site): string {
  return site === "gmail" ? ".a3s.aiL, .a3s, [data-message-body]" : "[data-qa=message-text], .c-message_kit__text";
}
export function extractMessage(document: Document, site: Site, url: string, selection = document.getSelection()): TabContext | null {
  const selector = messageSelector(site);
  const anchor = selection?.anchorNode;
  const element = anchor?.nodeType === 1 ? anchor as Element : anchor?.parentElement;
  const selected = selection?.toString().trim();
  const messages = [...document.querySelectorAll<HTMLElement>(selector)].filter(e => !e.closest('[aria-hidden="true"]') && !e.hidden);
  const opened = element?.closest(selector) ?? messages.at(-1);
  const text = selected && element?.closest(selector) ? selected : opened?.textContent?.trim();
  return text ? { url, title: document.title, selection: text } : null;
}
