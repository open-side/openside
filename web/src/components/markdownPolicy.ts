// What assistant Markdown may turn into, shared by the web renderer (Markdown.tsx) and the
// extension side panel (extension/src/markdown.ts) so one security rule exists once. Pure
// functions only: no React, no DOM.

const SAFE_PROTOCOL = /^(https?|ircs?|mailto|xmpp)$/i;

/** react-markdown's defaultUrlTransform: relative URLs and safe schemes pass, anything else becomes "". */
export function safeUrl(value: string): string {
  const colon = value.indexOf(":");
  const question = value.indexOf("?");
  const hash = value.indexOf("#");
  const slash = value.indexOf("/");
  if (colon === -1 || (slash !== -1 && colon > slash) || (question !== -1 && colon > question) || (hash !== -1 && colon > hash) || SAFE_PROTOCOL.test(value.slice(0, colon))) return value;
  return "";
}

/**
 * Model output never makes a client fetch: an image would bypass the API client (no auth, no 401
 * handling) and external hosts violate img-src. It becomes its alt text; an absolute http(s)
 * image on another origin stays one click away as a plain link the reader opens deliberately.
 */
export function imageChip(src: string | undefined, alt: string | undefined, ownOrigin: string): { label: string; href: string | null } {
  const safe = typeof src === "string" ? safeUrl(src) : "";
  const external = /^https?:\/\//i.test(safe) && !safe.startsWith(`${ownOrigin}/`);
  return { label: alt?.trim() || "Image", href: external ? safe : null };
}
