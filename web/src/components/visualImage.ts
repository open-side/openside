/**
 * Visual blocks (```html visual / ```svg visual) drawn as inert images. An image never runs
 * script, loads nothing and takes no input, so model-written markup cannot reach the page; it
 * also has a transparent backdrop and an intrinsic size, which a sandboxed frame cannot offer:
 * the parent can neither measure an opaque frame nor restyle it, and Chromium paints a light
 * frame white in a dark page. SVG becomes the image itself; HTML is laid out inside an SVG
 * `<foreignObject>` as XHTML.
 */

const FONT = 'system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif';
/** Tallest HTML visual; taller content is cut rather than scrolled. */
export const MAX_HTML_HEIGHT = 1600;
const DROP = ["script", "iframe", "frame", "object", "embed", "link", "meta", "base", "noscript", "template"].join(",");
const SVG_NS = "http://www.w3.org/2000/svg";

const dataUrl = (svg: string) => `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;

/*
 * Any document the page builds, even an inert parsed one, is held to the app's CSP
 * (`style-src 'self'`), so every `style` attribute or element in it raises a CSP violation.
 * The markup is parsed with those renamed and given back their names once serialized, so the
 * page never holds an inline style; only the image document, which has no CSP, applies them.
 */
const HIDDEN_STYLE = "data-osv-style";
const hideStyles = (source: string) =>
  source.replace(/<(\/?)style\b/gi, "<$1osv-style").replace(/<[a-z][^<>]*>/gi, (tag) => tag.replace(/(\s)style(\s*=)/gi, `$1${HIDDEN_STYLE}$2`));
const restoreStyles = (markup: string) => markup.replaceAll(` ${HIDDEN_STYLE}=`, " style=").replace(/<(\/?)osv-style\b/g, "<$1style");

export interface SvgPicture {
  url: string;
  /** Its own width in CSS px (`width` or the viewBox width); absent when it fills the column. */
  width?: number;
}

function parseSvg(source: string): Document | null {
  const parse = (text: string) => new DOMParser().parseFromString(text, "image/svg+xml");
  let doc = parse(source);
  // Models often leave out the namespaces, which an SVG file needs to be drawn at all.
  if (doc.documentElement.namespaceURI !== SVG_NS || doc.getElementsByTagName("parsererror").length) {
    doc = parse(source.replace(/<svg\b(?![^>]*\sxmlns=)/i, `<svg xmlns="${SVG_NS}"`).replace(/<svg\b(?![^>]*\sxmlns:xlink=)/i, '<svg xmlns:xlink="http://www.w3.org/1999/xlink"'));
  }
  const root = doc.documentElement;
  return root.localName === "svg" && root.namespaceURI === SVG_NS && !doc.getElementsByTagName("parsererror").length ? doc : null;
}

/** The SVG with text in `ink` and the system font; `null` when it is not an SVG document. */
export function svgPicture(source: string, ink: string): SvgPicture | null {
  const doc = parseSvg(hideStyles(source));
  if (!doc) return null;
  const root = doc.documentElement;
  root.querySelectorAll("script").forEach((node) => node.remove());
  const width = root.getAttribute("width")?.trim() ?? "";
  const own = /^\d+(\.\d+)?(px)?$/.test(width) ? parseFloat(width) : width ? undefined : Number(root.getAttribute("viewBox")?.trim().split(/[\s,]+/)[2]) || undefined;
  // Unpainted shapes and text default to black, which a dark page cannot show; they take the
  // page's text colour instead, and anything the visual colours itself keeps its colour.
  const style = `<style>:root{color:${ink};font-family:${FONT}}:root:not([fill]){fill:currentColor}</style>`;
  const svg = restoreStyles(new XMLSerializer().serializeToString(root)).replace(/^(<svg\b[^>]*?)(\/?)>/, (_, open: string, empty: string) => `${open}>${style}${empty ? "</svg>" : ""}`);
  return { url: dataUrl(svg), ...(own ? { width: own } : {}) };
}

/** The HTML as XHTML for a `<foreignObject>`, with the page's ink and font as its defaults. */
export function htmlDocument(source: string, ink: string): string {
  const doc = new DOMParser().parseFromString(hideStyles(source), "text/html");
  doc.querySelectorAll(DROP).forEach((node) => node.remove());
  const serialize = (node: Node) => restoreStyles(new XMLSerializer().serializeToString(node));
  const base = `html,body{margin:0;background:transparent;color:${ink};font:15px/1.5 ${FONT};overflow:hidden}img,svg,video,canvas,table{max-width:100%}`;
  // The visual's own styles come after these defaults, so they win.
  return `<html xmlns="http://www.w3.org/1999/xhtml"><head><style>${base}</style>${[...doc.head.childNodes].map(serialize).join("")}</head>${serialize(doc.body)}</html>`;
}

export function htmlPicture(xhtml: string, width: number, height: number): string {
  return dataUrl(
    `<svg xmlns="${SVG_NS}" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><foreignObject x="0" y="0" width="${width}" height="${height}">${xhtml}</foreignObject></svg>`,
  );
}

function load(url: string): Promise<HTMLImageElement> {
  const image = new Image();
  image.src = url;
  return image.decode().then(() => image);
}

/**
 * Height the HTML actually paints at `width`: it is drawn once at {@link MAX_HTML_HEIGHT} and
 * the lowest painted row found. `null` when the browser will not let the pixels be read or the
 * content fills the whole height (sized to the viewport), so no natural height exists.
 */
export async function paintedHeight(xhtml: string, width: number): Promise<number | null> {
  const scale = 0.5;
  const image = await load(htmlPicture(xhtml, width, MAX_HTML_HEIGHT));
  const canvas = document.createElement("canvas");
  canvas.width = Math.ceil(width * scale);
  canvas.height = Math.ceil(MAX_HTML_HEIGHT * scale);
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return null;
  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  let pixels: Uint8ClampedArray;
  try {
    pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
  } catch {
    return null;
  }
  for (let row = canvas.height - 1; row >= 0; row--) {
    for (let column = 0; column < canvas.width; column++) {
      if (pixels[(row * canvas.width + column) * 4 + 3]! > 8) {
        return row === canvas.height - 1 ? null : Math.ceil((row + 1) / scale) + 4;
      }
    }
  }
  return 0;
}
