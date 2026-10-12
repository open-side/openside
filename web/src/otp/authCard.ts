// Plain DOM keeps the web app and extension on the same card implementation.
import { brandMark, glyph, providerGlyph } from "./glyphs";

export type AuthButton = "primary" | "secondary" | "ghost";

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

/** Host shown to the person: `https://www.example.com:443` → `example.com`. */
export function siteOf(origin: string | null | undefined): string {
  try {
    return new URL(origin ?? "").host.replace(/^www\./, "") || "this site";
  } catch {
    return "this site";
  }
}

/**
 * The daemon-verified origin as written by the URL parser: scheme, punycode host (so a lookalike
 * IDN is spelled out) and a non-default port. Never the page's own name for itself.
 */
export function verifiedOrigin(origin: string | null | undefined): string | null {
  try {
    const parsed = new URL(origin ?? "");
    return parsed.origin === "null" ? null : parsed.origin;
  } catch {
    return null;
  }
}

/**
 * The page's favicon as the daemon fetched it (a `data:image/…` URL; anything else is ignored so a
 * ticket can never make the UI load a remote image), else a known provider's mark, else the
 * site's initial.
 */
export function siteIcon(name: string, icon?: string | null): HTMLElement {
  const holder = element("span", { class: "auth-icon", "aria-hidden": "true" });
  if (icon && /^data:image\/(png|x-icon|vnd\.microsoft\.icon|gif|jpeg|webp);base64,/i.test(icon)) {
    holder.append(element("img", { src: icon, alt: "", width: "28", height: "28" }));
    return holder;
  }
  const provider = providerGlyph(name);
  if (provider) {
    holder.classList.add("auth-icon--glyph");
    holder.append(glyph(provider, 28));
    return holder;
  }
  holder.classList.add("auth-icon--letter");
  holder.textContent = name && name !== "this site" ? name[0]!.toUpperCase() : "?";
  return holder;
}

/**
 * Header shared by every card. The second line is always the daemon-verified origin; whatever
 * name the page gave itself (`pageName`) is shown below it, labelled as the page's own claim.
 */
export function authHead(options: {
  titleId: string;
  title: string;
  /** Display name derived from the verified origin (`siteOf`); drives the icon fallback. */
  site: string;
  origin: string | null | undefined;
  pageName?: string | null;
  icon?: string | null;
  closeLabel: string;
  onClose: () => void;
}): { head: HTMLElement; close: HTMLButtonElement } {
  const close = element("button", { type: "button", class: "auth-close", "aria-label": options.closeLabel, title: options.closeLabel }, glyph("close", 16));
  close.addEventListener("click", options.onClose);
  const verified = verifiedOrigin(options.origin);
  const heading = element(
    "div",
    { class: "auth-heading" },
    element("strong", { id: options.titleId, class: "auth-title" }, options.title),
    element("span", { id: `${options.titleId}-origin`, class: "auth-origin", "data-verified": verified ? "true" : "false", ...(verified ? { title: verified } : {}) }, verified ?? "Unknown site"),
  );
  const pageName = options.pageName?.trim();
  if (pageName && pageName.toLowerCase() !== options.site.toLowerCase()) {
    heading.append(element("span", { class: "auth-page-name" }, `Page calls itself “${pageName}”`));
  }
  return { head: element("header", { class: "auth-head" }, siteIcon(options.site, options.icon), heading, close), close };
}

export function authFoot(): HTMLElement {
  return element("footer", { class: "auth-foot" }, "Handled by", element("span", { class: "auth-brand" }, brandMark(13), "openside"));
}

/** Busy indicator inside the primary button while a request is in flight. */
export function setPending(button: HTMLButtonElement, pending: boolean): void {
  button.toggleAttribute("data-pending", pending);
  button.setAttribute("aria-busy", String(pending));
}
