import { DEFAULT_LASSO, parseLassoSettings, type LassoSettings } from "../../web/src/lassoProtocol";
import { initLocale, t } from "./i18n";
import { doublePress, lassoPath, popoverPosition, selectionRect, type Point, type Rect } from "./lasso-geometry";

/*
 * The page side of Lasso. Injected through activeTab by the Capture menu item, the command key and
 * the side panel; registered on every site while the person has granted that (Settings › Lasso).
 * The selection dot and the double-press hotkey watch the page only while that grant stands
 * (`lassoAccess`, kept by the worker) and their toggle is on, wherever the script was injected from.
 *
 * Everything drawn here lives in a closed shadow root. The popover with the shortcuts and the
 * question box is an extension page in an iframe, so what the person types never passes through
 * this page's event listeners and the capture never enters this page.
 */
declare global { var opensideLasso: true | undefined }

const POPOVER_WIDTH = 280;
const ROW_HEIGHT = 32;
const INPUT_HEIGHT = 44;
const POPOVER_PADDING = 6;
const FAILED_MS = 2000;

const STYLE = `
:host { all: initial; }
.layer { position: fixed; inset: 0; z-index: 2147483647; cursor: crosshair; background: rgb(0 0 0 / 0.14); touch-action: none; }
.layer[data-phase="chosen"] { background: transparent; cursor: default; }
.layer[data-phase="capturing"] { visibility: hidden; }
.layer[data-phase="failed"] { cursor: default; }
svg { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; pointer-events: none; }
path { fill: rgb(255 255 255 / 0.08); stroke: #ffffff; stroke-width: 3; stroke-linejoin: round; stroke-linecap: round; filter: drop-shadow(0 0 4px rgb(10 132 255 / 0.9)); }
.hint { position: absolute; top: 16px; left: 50%; transform: translateX(-50%); padding: 6px 12px; border-radius: 999px; background: rgb(23 23 23 / 0.9); color: #fafafa; font: 500 13px/18px system-ui, sans-serif; pointer-events: none; white-space: nowrap; }
.layer[data-phase="chosen"] .hint { display: none; }
.dot { position: fixed; z-index: 2147483647; width: 14px; height: 14px; padding: 0; border: 2px solid #ffffff; border-radius: 50%; background: #0a84ff; box-shadow: 0 1px 4px rgb(0 0 0 / 0.3); cursor: pointer; }
.dot:focus-visible { outline: 2px solid #0a84ff; outline-offset: 2px; }
iframe { position: fixed; z-index: 2147483647; border: 0; border-radius: 14px; box-shadow: 0 12px 32px rgb(0 0 0 / 0.28); color-scheme: normal; background: transparent; }
`;

let settings: LassoSettings = DEFAULT_LASSO;
let access = false;
let host: HTMLElement | undefined;
let root: ShadowRoot | undefined;
let layer: HTMLElement | undefined;
let dot: HTMLButtonElement | undefined;
let frame: HTMLIFrameElement | undefined;
let token: string | undefined;
/** Which ambient feature opened the current overlay or popover, so turning that feature off closes it. */
let ambient: "text" | "region" | undefined;
let watching: AbortController | undefined;

function shadow(): ShadowRoot {
  if (!host || !root) {
    host = document.createElement("openside-lasso");
    root = host.attachShadow({ mode: "closed" });
    const style = document.createElement("style");
    style.textContent = STYLE;
    root.append(style);
  }
  if (!host.isConnected) document.documentElement.append(host);
  return root;
}

function viewport() { return { width: window.innerWidth, height: window.innerHeight }; }

/** Removes the overlay, the dot and the popover; a pending ask is dropped unless it was sent. */
function close(sent = false): void {
  if (token && !sent) void chrome.runtime.sendMessage({ type: "lasso.cancel", token }).catch(() => undefined);
  token = undefined;
  ambient = undefined;
  layer?.remove(); layer = undefined;
  frame?.remove(); frame = undefined;
  hideDot();
  window.removeEventListener("keydown", escape, true);
}

function escape(event: KeyboardEvent): void {
  if (event.key !== "Escape" || !event.isTrusted) return;
  event.preventDefault();
  event.stopPropagation();
  close();
}

function popover(anchor: Rect, kind: "text" | "region", ask: string): void {
  const count = (kind === "text" ? settings.text : settings.region).shortcuts.length;
  const size = { width: POPOVER_WIDTH, height: POPOVER_PADDING * 2 + count * ROW_HEIGHT + INPUT_HEIGHT };
  const place = popoverPosition(anchor, size, viewport());
  frame?.remove();
  frame = document.createElement("iframe");
  frame.src = `${chrome.runtime.getURL("ask.html")}#${ask}`;
  frame.title = t("ext.lasso.popover.title");
  frame.style.cssText = `left:${place.x}px;top:${place.y}px;width:${size.width}px;height:${size.height}px`;
  frame.setAttribute("allow", "");
  shadow().append(frame);
  frame.focus();
}

function nextFrame(): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  return promise;
}

/** The region lasso: draw around something, then pick a shortcut or ask about it. */
function startRegion(): void {
  if (layer) return;
  hideDot();
  const surface = shadow();
  layer = document.createElement("div");
  layer.className = "layer";
  layer.dataset.phase = "drawing";
  const hint = document.createElement("p");
  hint.className = "hint";
  hint.textContent = t("ext.lasso.hint");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  svg.append(path);
  layer.append(svg, hint);
  surface.append(layer);
  window.addEventListener("keydown", escape, true);
  const current = layer;
  let points: Point[] = [];
  current.addEventListener("pointerdown", event => {
    if (!event.isTrusted || event.button !== 0 || current.dataset.phase !== "drawing") return;
    event.preventDefault();
    current.setPointerCapture(event.pointerId);
    points = [{ x: event.clientX, y: event.clientY }];
    path.setAttribute("d", lassoPath(points));
  });
  current.addEventListener("pointermove", event => {
    if (!points.length || current.dataset.phase !== "drawing") return;
    const last = points[points.length - 1]!;
    if (Math.hypot(event.clientX - last.x, event.clientY - last.y) < 2) return;
    points.push({ x: event.clientX, y: event.clientY });
    path.setAttribute("d", lassoPath(points));
  });
  current.addEventListener("pointerup", event => {
    if (!event.isTrusted || !points.length || current.dataset.phase !== "drawing") return;
    const rect = selectionRect(points, viewport());
    points = [];
    if (!rect) { path.setAttribute("d", ""); return; }
    // The capture must show the page, not the dimmed overlay over it.
    current.dataset.phase = "capturing";
    // A refused capture (tab switched, no room left for it) says so on the overlay before it goes away.
    const failed = () => {
      if (layer !== current) return;
      path.setAttribute("d", "");
      hint.textContent = t("ext.lasso.captureFailed");
      current.dataset.phase = "failed";
      setTimeout(() => { if (layer === current) close(); }, FAILED_MS);
    };
    void nextFrame().then(() => chrome.runtime.sendMessage({ type: "lasso.capture", rect, viewport: viewport() })).then((reply: { token?: string } | undefined) => {
      if (layer !== current) return;
      if (!reply?.token) { failed(); return; }
      token = reply.token;
      current.dataset.phase = "chosen";
      popover(rect, "region", reply.token);
    }, failed);
  });
  // A click outside the popover once it shows closes everything, like pressing Escape.
  current.addEventListener("click", event => { if (event.isTrusted && current.dataset.phase === "chosen") close(); });
}

function hideDot(): void { dot?.remove(); dot = undefined; }

function editable(node: Node | null): boolean {
  const element = node instanceof Element ? node : node?.parentElement;
  return !!element?.closest("input, textarea, [contenteditable=''], [contenteditable='true']");
}

/** After a text selection: the blue dot at its end, which opens the shortcuts for it. */
function showDot(): void {
  const selection = window.getSelection();
  const text = selection?.toString().trim() ?? "";
  if (!watching || !settings.text.enabled || !selection || selection.rangeCount === 0 || !text || editable(selection.anchorNode) || layer) { hideDot(); return; }
  const rects = selection.getRangeAt(selection.rangeCount - 1).getClientRects();
  const end = rects[rects.length - 1];
  if (!end) { hideDot(); return; }
  hideDot();
  dot = document.createElement("button");
  dot.className = "dot";
  dot.type = "button";
  dot.setAttribute("aria-label", t("ext.lasso.dot"));
  dot.style.left = `${Math.min(end.right + 4, window.innerWidth - 18)}px`;
  dot.style.top = `${Math.max(end.top + end.height / 2 - 7, 4)}px`;
  const anchor = { x: end.right, y: end.top, width: 14, height: end.height };
  // The selection is read when the dot is pressed, before the click could move it.
  dot.addEventListener("mousedown", event => { event.preventDefault(); event.stopPropagation(); });
  dot.addEventListener("click", event => {
    if (!event.isTrusted) return;
    const chosen = window.getSelection()?.toString().trim() ?? "";
    hideDot();
    if (!chosen) return;
    void chrome.runtime.sendMessage({ type: "lasso.selection", selection: chosen }).then((reply: { token?: string } | undefined) => {
      if (!reply?.token) return;
      close();
      token = reply.token;
      ambient = "text";
      window.addEventListener("keydown", escape, true);
      popover(anchor, "text", reply.token);
    }, () => undefined);
  });
  shadow().append(dot);
}

const pressed = doublePress(() => settings.region.hotkey);

function watchKeys(event: KeyboardEvent): void {
  if (!event.isTrusted || !pressed(event) || !settings.region.enabled || layer) return;
  startRegion();
  ambient = "region";
}

function watchMouseUp(event: MouseEvent): void {
  if (event.isTrusted && event.button === 0 && !host?.contains(event.target as Node)) setTimeout(showDot, 0);
}

// A press anywhere else on the page drops the dot, and the shortcuts a dot opened (a lasso's own layer catches its presses).
function watchMouseDown(event: MouseEvent): void {
  if (!event.isTrusted || event.target === host) return;
  hideDot();
  if (frame && !layer) close();
}

/** Attaches the page-wide listeners the dot and the hotkey need, or drops them all. */
function watch(on: boolean): void {
  if (on === !!watching) return;
  if (!on) { watching?.abort(); watching = undefined; return; }
  watching = new AbortController();
  const { signal } = watching;
  window.addEventListener("keydown", watchKeys, { capture: true, signal });
  window.addEventListener("keyup", watchKeys, { capture: true, signal });
  document.addEventListener("mouseup", watchMouseUp, { capture: true, signal });
  document.addEventListener("mousedown", watchMouseDown, { capture: true, signal });
  window.addEventListener("scroll", hideDot, { capture: true, passive: true, signal });
}

function update(next: LassoSettings, granted: boolean): void {
  // Only a revocation closes what is open: an injection without the grant still serves `lasso.start`.
  const revoked = access && !granted;
  settings = next;
  access = granted;
  if (revoked || (ambient === "text" && !settings.text.enabled) || (ambient === "region" && !settings.region.enabled)) close();
  if (!access || !settings.text.enabled) hideDot();
  watch(access && (settings.text.enabled || settings.region.enabled));
}

function install(): void {
  const parse = (value: unknown) => parseLassoSettings(value) ?? DEFAULT_LASSO;
  void chrome.storage.local.get(["lassoSettings", "lassoAccess"]).then(stored => update(parse(stored.lassoSettings), stored.lassoAccess === true), () => undefined);
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area !== "local" || !(changes.lassoSettings || changes.lassoAccess)) return;
    update(changes.lassoSettings ? parse(changes.lassoSettings.newValue) : settings, changes.lassoAccess ? changes.lassoAccess.newValue === true : access);
  });
  void initLocale(() => undefined).catch(() => undefined);
  chrome.runtime.onMessage.addListener((message: unknown) => {
    if (typeof message !== "object" || message === null || !("type" in message)) return;
    if (message.type === "lasso.start") { startRegion(); ambient = undefined; }
    if (message.type === "lasso.close") close(true);
  });
}

if (!globalThis.opensideLasso) {
  globalThis.opensideLasso = true;
  install();
}
