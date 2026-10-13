import { avatarTone } from "../components/avatarTone";
import type { CardPerson, CardSite, MediaRef } from "../api/types";
import { element } from "./dom";
import { glyph, providerGlyph, type GlyphName } from "./glyphs";
import type { Media } from "./media";
import { initialOf, isDataIcon } from "./values";

/**
 * A site's icon. The daemon-normalized PNG data URL is drawn directly; an origin reference loads
 * through the authenticated client for that exact origin. Everything else (including a remote
 * URL) falls back to a glyph or the site's initial, so the UI never fetches an icon itself.
 */
export function siteMark(site: CardSite, media: Media, fallback: GlyphName | null, size = 20): HTMLElement {
  const mark = element("span", { class: "oc-mark", "aria-hidden": "true" });
  // CSSOM, not a `style` attribute: the daemon's style-src CSP refuses inline style attributes.
  mark.style.setProperty("--oc-mark-size", `${size}px`);
  const known = providerGlyph(site.name);
  mark.append(element("span", { class: "oc-mark-fallback" }, fallback ? glyph(fallback, Math.round(size * 0.7)) : known ? glyph(known, Math.round(size * 0.8)) : initialOf(site.name)));

  const ref = site.favicon_ref;
  const img = element("img", { alt: "", decoding: "async" });
  img.addEventListener("load", () => mark.setAttribute("data-loaded", ""));
  if (ref?.type === "data" && isDataIcon(ref.data_url)) {
    img.src = ref.data_url;
    mark.append(img);
  } else if (ref?.type === "origin" && site.origin && ref.origin === site.origin) {
    mark.append(img);
    media.attach(img, media.favicon(ref.origin), () => img.remove());
  }
  return mark;
}

export function avatar(person: CardPerson, size: 24 | 32 | 40 = 32): HTMLElement {
  return element("span", { class: `avatar avatar--${size} avatar--${avatarTone(person.color)}`, "aria-hidden": "true" }, person.initials);
}

/** An artifact image on a tile that holds its shape while loading and when the image is missing. */
export function thumb(image: MediaRef | null | undefined, alt: string | null | undefined, media: Media, size: "sm" | "lg" | "wide"): HTMLElement {
  const tile = element("span", { class: "oc-thumb", "data-size": size });
  if (!image) return tile;
  const img = element("img", { alt: alt ?? "", decoding: "async", loading: "lazy" });
  img.addEventListener("load", () => tile.setAttribute("data-loaded", ""));
  tile.append(img);
  media.attach(img, media.artifact(image.path), () => {
    img.remove();
    tile.setAttribute("data-failed", "");
  });
  return tile;
}
