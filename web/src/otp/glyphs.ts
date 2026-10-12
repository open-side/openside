// Sign-in provider marks drawn locally: the login card never fetches an icon from a provider or
// a CDN (CSP and privacy). Brand colours stay where the mark is unrecognisable without them
// (Google, Kakao, Naver, Microsoft, Facebook); the rest follow `currentColor`.
export type GlyphName =
  | "google"
  | "apple"
  | "kakao"
  | "naver"
  | "facebook"
  | "github"
  | "microsoft"
  | "passkey"
  | "qr"
  | "key"
  | "eye"
  | "eye-off"
  | "close"
  | "check"
  | "chevron"
  | "phone"
  | "alert";

const STROKE = 'fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"';

const GLYPHS: Record<GlyphName, string> = {
  google:
    '<svg viewBox="0 0 48 48"><path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z"/><path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z"/><path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z"/><path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z"/></svg>',
  apple:
    '<svg viewBox="0 0 24 24"><path fill="currentColor" d="M12.15 6.9c-.95 0-2.42-1.08-3.96-1.04-2.04.03-3.91 1.18-4.96 3.01-2.12 3.68-.55 9.1 1.52 12.09 1.01 1.45 2.21 3.09 3.79 3.04 1.52-.07 2.09-.99 3.94-.99 1.83 0 2.35.99 3.96.95 1.64-.03 2.68-1.48 3.68-2.95 1.16-1.69 1.64-3.33 1.66-3.42-.04-.01-3.18-1.22-3.22-4.86-.03-3.04 2.48-4.49 2.6-4.56-1.43-2.09-3.62-2.32-4.39-2.38-2-.16-3.68 1.09-4.62 1.09zm3.38-3.07c.84-1.01 1.4-2.43 1.25-3.83-1.21.05-2.66.81-3.53 1.82-.78.9-1.45 2.34-1.27 3.71 1.34.1 2.71-.69 3.55-1.7"/></svg>',
  kakao:
    '<svg viewBox="0 0 24 24"><rect width="24" height="24" rx="6" fill="#FEE500"/><path fill="#191919" d="M12 6.5c-3.87 0-7 2.46-7 5.5 0 1.96 1.3 3.68 3.26 4.65l-.66 2.43c-.06.22.19.4.38.27l2.9-1.92c.37.05.74.07 1.12.07 3.87 0 7-2.46 7-5.5S15.87 6.5 12 6.5z"/></svg>',
  naver:
    '<svg viewBox="0 0 24 24"><rect width="24" height="24" rx="6" fill="#03C75A"/><path fill="#fff" d="M13.56 12.28 10.3 7.5H7.5v9h2.94v-4.78l3.26 4.78h2.8v-9h-2.94z"/></svg>',
  facebook:
    '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="12" fill="#1877F2"/><path fill="#fff" d="M16.67 15.47 17.2 12h-3.33V9.75c0-.95.47-1.88 1.95-1.88h1.51V4.92s-1.37-.23-2.68-.23c-2.74 0-4.53 1.66-4.53 4.66V12H7.08v3.47h3.04V24a12.1 12.1 0 0 0 3.75 0v-8.53z"/></svg>',
  github:
    '<svg viewBox="0 0 24 24"><path fill="currentColor" d="M12 .3a12 12 0 0 0-3.8 23.38c.6.11.82-.26.82-.58l-.02-2.04c-3.34.73-4.04-1.61-4.04-1.61-.55-1.39-1.33-1.76-1.33-1.76-1.09-.74.08-.73.08-.73 1.2.08 1.84 1.24 1.84 1.24 1.07 1.83 2.81 1.3 3.5 1 .1-.78.42-1.3.76-1.61-2.67-.3-5.47-1.33-5.47-5.93 0-1.31.47-2.38 1.24-3.22-.14-.3-.54-1.52.1-3.18 0 0 1.01-.32 3.3 1.23a11.5 11.5 0 0 1 6 0c2.28-1.55 3.29-1.23 3.29-1.23.64 1.66.24 2.88.12 3.18a4.65 4.65 0 0 1 1.23 3.22c0 4.61-2.8 5.63-5.48 5.92.43.37.81 1.1.81 2.22l-.01 3.29c0 .32.21.69.82.57A12 12 0 0 0 12 .3"/></svg>',
  microsoft:
    '<svg viewBox="0 0 23 23"><path fill="#F25022" d="M1 1h10v10H1z"/><path fill="#7FBA00" d="M12 1h10v10H12z"/><path fill="#00A4EF" d="M1 12h10v10H1z"/><path fill="#FFB900" d="M12 12h10v10H12z"/></svg>',
  passkey: `<svg viewBox="0 0 24 24" ${STROKE}><circle cx="9" cy="7.5" r="3.5"/><path d="M3 20c0-3.31 2.69-6 6-6 1.2 0 2.31.35 3.25.96"/><circle cx="18" cy="12.5" r="2.5"/><path d="M18 15v6m0-2.5h2"/></svg>`,
  qr: `<svg viewBox="0 0 24 24" ${STROKE}><rect x="3.5" y="3.5" width="6" height="6" rx="1.2"/><rect x="14.5" y="3.5" width="6" height="6" rx="1.2"/><rect x="3.5" y="14.5" width="6" height="6" rx="1.2"/><path d="M14.5 14.5h2.5v2.5M20.5 14.5v.01M14.5 20.5h.01M17.5 20.5h3v-3"/></svg>`,
  key: `<svg viewBox="0 0 24 24" ${STROKE}><circle cx="8" cy="15" r="4.5"/><path d="m11.2 11.8 8.3-8.3m-3 3 2.5 2.5m-5-0.5 2 2"/></svg>`,
  eye: `<svg viewBox="0 0 24 24" ${STROKE}><path d="M2.06 12.35a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 19.88 0 1 1 0 0 1 0 .7 10.75 10.75 0 0 1-19.88 0"/><circle cx="12" cy="12" r="3"/></svg>`,
  "eye-off": `<svg viewBox="0 0 24 24" ${STROKE}><path d="M10.73 5.08A10.74 10.74 0 0 1 21.94 11.65a1 1 0 0 1 0 .7 10.75 10.75 0 0 1-1.44 2.49M14.08 14.16a3 3 0 0 1-4.24-4.24M17.48 17.5a10.75 10.75 0 0 1-15.42-5.15 1 1 0 0 1 0-.7 10.75 10.75 0 0 1 4.45-5.14M2 2l20 20"/></svg>`,
  close: `<svg viewBox="0 0 24 24" ${STROKE}><path d="M18 6 6 18M6 6l12 12"/></svg>`,
  check: `<svg viewBox="0 0 24 24" ${STROKE}><path d="M20 6 9 17l-5-5"/></svg>`,
  chevron: `<svg viewBox="0 0 24 24" ${STROKE}><path d="m9 18 6-6-6-6"/></svg>`,
  phone: `<svg viewBox="0 0 24 24" ${STROKE}><rect x="6" y="2.5" width="12" height="19" rx="2.5"/><path d="M10.5 18.5h3"/></svg>`,
  alert: `<svg viewBox="0 0 24 24" ${STROKE}><circle cx="12" cy="12" r="9.5"/><path d="M12 7.5v5.5M12 16.5v.01"/></svg>`,
};

const BRAND =
  '<svg viewBox="-96 -96 192 192"><path fill="currentColor" d="M0 96A96 96 0 1 1 90.18 -32.92L61.28 -18.47A64 64 0 1 0 0 64ZM46 16L94 -8L94 56L46 80Z"/></svg>';

function svg(markup: string, size: number): SVGSVGElement {
  // The markup is the constant table above, never page or ticket text.
  const template = document.createElement("template");
  template.innerHTML = markup;
  const node = template.content.firstElementChild as SVGSVGElement;
  node.setAttribute("width", String(size));
  node.setAttribute("height", String(size));
  node.setAttribute("aria-hidden", "true");
  node.setAttribute("focusable", "false");
  return node;
}

export function glyph(name: GlyphName, size = 20): SVGSVGElement {
  return svg(GLYPHS[name], size);
}

export function brandMark(size = 14): SVGSVGElement {
  return svg(BRAND, size);
}

/** A provider's mark when the page named one; the site name or host as a fallback guess. */
export function providerGlyph(name: string | null | undefined): GlyphName | null {
  const key = (name ?? "").toLowerCase();
  for (const provider of ["google", "apple", "kakao", "naver", "facebook", "github", "microsoft"] as const) {
    if (key.includes(provider)) return provider;
  }
  return null;
}
