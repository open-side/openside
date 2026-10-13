import type { CardValue, InputField } from "../api/types";

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

// A normalized icon is a PNG of at most 8 KiB (base64 ≈ 11 KiB). Anything else could make the UI
// decode an unbounded or non-raster payload, so it is ignored rather than drawn.
const DATA_ICON = /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/;
const DATA_ICON_MAX = 12 * 1024;

/** Only the daemon's normalized PNG data URL may be drawn as a site icon; a remote URL would make the UI fetch. */
export function isDataIcon(icon: string | null | undefined): icon is string {
  return !!icon && icon.length <= DATA_ICON_MAX && DATA_ICON.test(icon);
}

/** A base64 raster `data:` URL of any common icon type, for icons that are not card sites (connection favicons). */
export function isImageDataUrl(icon: string | null | undefined): icon is string {
  return !!icon && /^data:image\/(png|x-icon|vnd\.microsoft\.icon|gif|jpeg|webp);base64,/i.test(icon);
}

/** A navigable http(s) URL without credentials; `null` for everything else (javascript:, data:, userinfo). */
export function safeHref(url: string | null | undefined): string | null {
  try {
    const parsed = new URL(url ?? "");
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return null;
    if (parsed.username || parsed.password) return null;
    return parsed.href;
  } catch {
    return null;
  }
}

/** The host a link really goes to (punycode, with a non-default port); shown beside every link label, which the model chooses. */
export function linkHost(href: string): string {
  return new URL(href).host;
}

/** The artifact path the daemon materialized for a card image: session-relative, no traversal, raster only. */
export function isCardMediaPath(path: string): boolean {
  return /^cards\/media\/[A-Za-z0-9_-]{1,128}\.(webp|png)$/.test(path);
}

/** Letter shown when no icon is available; a glyph, not a claim of brand identity. */
export function initialOf(name: string): string {
  const letter = [...name.trim()].find((char) => /\p{L}|\p{N}/u.test(char));
  return letter ? letter.toUpperCase() : "?";
}

function fractionDigits(currency: string): number {
  try {
    return new Intl.NumberFormat("en-US", { style: "currency", currency }).resolvedOptions().maximumFractionDigits ?? 2;
  } catch {
    return 2;
  }
}

/** Captured amounts only: integer minor units, never recalculated. */
export function formatMoney(money: { minor_units: number; currency: string }): string {
  const digits = fractionDigits(money.currency);
  const amount = money.minor_units / 10 ** digits;
  try {
    return new Intl.NumberFormat("en-US", { style: "currency", currency: money.currency, minimumFractionDigits: digits, maximumFractionDigits: digits }).format(amount);
  } catch {
    return `${money.currency} ${amount.toFixed(digits)}`;
  }
}

/** "4.4 (231)", or "4.4/10" when the scale is not five. */
export function formatRating(rating: { milli_value: number; milli_max: number; count?: number | null }): string {
  const value = (rating.milli_value / 1000).toFixed(1);
  const scale = rating.milli_max === 5000 ? "" : `/${+(rating.milli_max / 1000).toFixed(1)}`;
  const count = rating.count != null ? ` (${rating.count.toLocaleString("en-US")})` : "";
  return `${value}${scale}${count}`;
}

export function ratingLabel(rating: { milli_value: number; milli_max: number; count?: number | null }): string {
  const count = rating.count != null ? `, ${rating.count.toLocaleString("en-US")} ratings` : "";
  return `Rated ${(rating.milli_value / 1000).toFixed(1)} out of ${+(rating.milli_max / 1000).toFixed(1)}${count}`;
}

/** Plain text for any typed value (clipboard, table cells without a richer form, text fallback). */
export function valueText(value: CardValue): string {
  switch (value.type) {
    case "text":
      return value.text;
    case "money":
      return formatMoney(value.money);
    case "link":
      return value.link.label;
    case "rating":
      return formatRating(value.rating);
    case "image":
      return value.alt;
  }
}

/** A field that never leaves the input as anything but the person's own keystrokes. */
export function isSecretField(field: InputField): boolean {
  return field.secret || field.otp || !!field.card || field.kind === "password";
}
