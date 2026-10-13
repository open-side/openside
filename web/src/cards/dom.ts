// Plain DOM keeps the web app and the extension on one card implementation.
export function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attributes: Record<string, string> = {},
  ...children: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  node.append(...children);
  return node;
}

/** Busy indicator inside a button while its request is in flight. */
export function setPending(button: HTMLElement, pending: boolean): void {
  button.toggleAttribute("data-pending", pending);
  button.setAttribute("aria-busy", String(pending));
}

let counter = 0;
/** A document-unique id for label/describedby wiring. */
export function uid(prefix: string): string {
  return `${prefix}-${++counter}`;
}
