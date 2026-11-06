import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_LASSO, type LassoSettings } from "../../web/src/lassoProtocol";

/*
 * lasso.ts guards every handler with `event.isTrusted`, which jsdom never sets on dispatched events,
 * so the listeners it adds are recorded here and called with trusted-looking event objects. Its
 * shadow root is closed, so attachShadow is wrapped to keep a handle on it.
 */
type Listener = (event: unknown) => void;
let listeners: { target: EventTarget; type: string; listener: Listener; signal?: AbortSignal }[];
let roots: ShadowRoot[];
let onMessage: (message: unknown) => void;
// The page script and its i18n both follow storage changes, so every listener is kept.
let storageListeners: ((changes: Record<string, { newValue?: unknown }>, area: string) => void)[];
const onChanged = (changes: Record<string, { newValue?: unknown }>, area: string) => { for (const listener of storageListeners) listener(changes, area); };
let replies: Record<string, unknown>;
let stored: LassoSettings | undefined;
let granted: boolean;
const sendMessage = vi.fn(async (message: { type: string }) => replies[message.type]);
const localGet = vi.fn(async (keys: string[]) => ({ ...(stored && keys.includes("lassoSettings") ? { lassoSettings: stored } : {}), ...(keys.includes("lassoAccess") ? { lassoAccess: granted } : {}) }));
const TOKEN = "0b0e3a10-1c1d-4c4d-8e8f-123456789abc";

const fire = (target: EventTarget, type: string, init: Record<string, unknown> = {}) => {
  for (const entry of listeners.filter(item => item.target === target && item.type === type && !item.signal?.aborted)) entry.listener({ type, isTrusted: true, preventDefault() {}, stopPropagation() {}, ...init });
};
const root = () => roots.at(-1)!;
const layer = () => root().querySelector<HTMLElement>(".layer");
const frame = () => root().querySelector<HTMLIFrameElement>("iframe");

/** Loads a fresh page script; `access` is the worker's `lassoAccess` flag (the `<all_urls>` grant). */
async function load(settings?: LassoSettings, access = true): Promise<void> {
  stored = settings;
  granted = access;
  listeners = [];
  roots = [];
  storageListeners = [];
  // Each case needs a fresh copy of the page script: it installs itself when the module loads.
  vi.resetModules();
  delete globalThis.opensideLasso;
  await import("../src/lasso");
  const read = localGet.mock.calls.findLastIndex(([keys]) => keys.includes("lassoSettings"));
  // The script applies the stored settings in a callback on this same read; awaiting it after lets that run first.
  await localGet.mock.results[read]!.value;
}

function draw(points: [number, number][]): void {
  const surface = layer()!;
  const [first, ...rest] = points;
  fire(surface, "pointerdown", { button: 0, clientX: first![0], clientY: first![1], pointerId: 1 });
  for (const [x, y] of rest) fire(surface, "pointermove", { clientX: x, clientY: y });
  fire(surface, "pointerup", { clientX: points.at(-1)![0], clientY: points.at(-1)![1] });
}

beforeEach(() => {
  listeners = [];
  roots = [];
  replies = {};
  sendMessage.mockClear();
  document.documentElement.querySelectorAll("openside-lasso").forEach(node => node.remove());
  const addEventListener = EventTarget.prototype.addEventListener;
  vi.spyOn(EventTarget.prototype, "addEventListener").mockImplementation(function (this: EventTarget, type: string, listener: EventListenerOrEventListenerObject | null, options?: boolean | AddEventListenerOptions) {
    if (typeof listener === "function") listeners.push({ target: this, type, listener: listener as Listener, signal: typeof options === "object" ? options.signal : undefined });
    addEventListener.call(this, type, listener, options);
  });
  // The environment gives `window` its own bound addEventListener, which bypasses EventTarget.prototype.
  vi.spyOn(window, "addEventListener").mockImplementation((type: string, listener: EventListenerOrEventListenerObject | null, options?: boolean | AddEventListenerOptions) => {
    if (typeof listener === "function") listeners.push({ target: window, type, listener: listener as Listener, signal: typeof options === "object" ? options.signal : undefined });
  });
  const attachShadow = Element.prototype.attachShadow;
  vi.spyOn(Element.prototype, "attachShadow").mockImplementation(function (this: Element, init: ShadowRootInit) {
    const shadow = attachShadow.call(this, init);
    roots.push(shadow);
    return shadow;
  });
  HTMLElement.prototype.setPointerCapture = vi.fn();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => setTimeout(() => callback(0), 16));
  vi.stubGlobal("chrome", {
    runtime: {
      sendMessage,
      getURL: (path: string) => `chrome-extension://ext/${path}`,
      onMessage: { addListener: (callback: typeof onMessage) => { onMessage = callback; } },
    },
    storage: {
      local: { get: localGet },
      onChanged: { addListener: (callback: (typeof storageListeners)[number]) => { storageListeners.push(callback); } },
    },
  });
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("region lasso", () => {
  it("captures the drawn box without the overlay, then opens the popover next to it", async () => {
    await load();
    replies["lasso.capture"] = { token: TOKEN };
    onMessage({ type: "lasso.start" });
    expect(layer()?.dataset.phase).toBe("drawing");
    expect(root().querySelector(".hint")?.textContent).toBe("Draw around what you want to ask about. Press Esc to cancel.");
    draw([[100, 100], [300, 120], [280, 260]]);
    expect(layer()?.dataset.phase).toBe("capturing");
    expect(sendMessage).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(sendMessage).toHaveBeenCalledWith({ type: "lasso.capture", rect: { x: 100, y: 100, width: 200, height: 160 }, viewport: { width: innerWidth, height: innerHeight } }));
    await vi.waitFor(() => expect(frame()).not.toBeNull());
    expect(frame()!.src).toBe(`chrome-extension://ext/ask.html#${TOKEN}`);
    expect(layer()?.dataset.phase).toBe("chosen");
    expect(frame()!.style.width).toBe("280px");
  });

  it("ignores a click-sized stroke and keeps drawing", async () => {
    await load();
    onMessage({ type: "lasso.start" });
    draw([[100, 100], [103, 104]]);
    expect(layer()?.dataset.phase).toBe("drawing");
    await vi.runAllTimersAsync();
    expect(sendMessage).not.toHaveBeenCalled();
  });

  it("says so when the capture is refused, then closes", async () => {
    await load();
    replies["lasso.capture"] = { error: "storage_full" };
    onMessage({ type: "lasso.start" });
    draw([[100, 100], [300, 300]]);
    // Two animation frames (16 ms each here) pass before the capture is asked for.
    await vi.advanceTimersByTimeAsync(32);
    expect(layer()?.dataset.phase).toBe("failed");
    expect(root().querySelector(".hint")?.textContent).toBe("Couldn't capture this area. Try again.");
    expect(frame()).toBeNull();
    await vi.advanceTimersByTimeAsync(1999);
    expect(layer()).not.toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(layer()).toBeNull();
  });

  it("drops the pending ask on Escape", async () => {
    await load();
    replies["lasso.capture"] = { token: TOKEN };
    onMessage({ type: "lasso.start" });
    draw([[100, 100], [300, 300]]);
    await vi.waitFor(() => expect(frame()).not.toBeNull());
    fire(window, "keydown", { key: "Escape" });
    expect(sendMessage).toHaveBeenLastCalledWith({ type: "lasso.cancel", token: TOKEN });
    expect(layer()).toBeNull();
    expect(frame()).toBeNull();
  });

  it("closes without cancelling when the ask was sent", async () => {
    await load();
    replies["lasso.capture"] = { token: TOKEN };
    onMessage({ type: "lasso.start" });
    draw([[100, 100], [300, 300]]);
    await vi.waitFor(() => expect(frame()).not.toBeNull());
    onMessage({ type: "lasso.close" });
    expect(frame()).toBeNull();
    expect(sendMessage).not.toHaveBeenCalledWith(expect.objectContaining({ type: "lasso.cancel" }));
  });

  it("starts from the hotkey pressed twice, only while lasso selection is on", async () => {
    await load({ ...DEFAULT_LASSO, region: { ...DEFAULT_LASSO.region, hotkey: "shift" } });
    for (const [type, timeStamp] of [["keydown", 0], ["keyup", 40], ["keydown", 150]] as const) fire(window, type, { key: "Shift", repeat: false, timeStamp });
    expect(layer()?.dataset.phase).toBe("drawing");
    fire(window, "keydown", { key: "Escape" });

    await load({ ...DEFAULT_LASSO, region: { ...DEFAULT_LASSO.region, enabled: false } });
    for (const [type, timeStamp] of [["keydown", 0], ["keyup", 40], ["keydown", 150]] as const) fire(window, type, { key: "Alt", repeat: false, timeStamp });
    expect(roots.at(-1)?.querySelector(".layer") ?? null).toBeNull();
  });

  it("installs once however often it is injected", async () => {
    await load();
    const first = onMessage;
    vi.resetModules();
    await import("../src/lasso");
    expect(onMessage).toBe(first);
  });
});

describe("site access", () => {
  const pressTwice = (key: string) => { for (const [type, timeStamp] of [["keydown", 0], ["keyup", 40], ["keydown", 150]] as const) fire(window, type, { key, repeat: false, timeStamp }); };
  function selectText(): HTMLParagraphElement {
    const paragraph = document.createElement("p");
    paragraph.textContent = "Hello world";
    document.body.append(paragraph);
    vi.spyOn(window, "getSelection").mockReturnValue({
      toString: () => "Hello world",
      rangeCount: 1,
      anchorNode: paragraph.firstChild,
      getRangeAt: () => ({ getClientRects: () => [{ right: 200, top: 50, height: 20 }] }),
    } as unknown as Selection);
    return paragraph;
  }
  const dots = () => roots.flatMap(shadow => [...shadow.querySelectorAll(".dot")]);

  it("keeps the dot and the hotkey off when injected without the grant, but still draws on request", async () => {
    await load(undefined, false);
    const paragraph = selectText();
    fire(document, "mouseup", { button: 0, target: paragraph });
    await vi.runAllTimersAsync();
    expect(dots()).toEqual([]);
    pressTwice("Alt");
    expect(roots.at(-1)?.querySelector(".layer") ?? null).toBeNull();
    onMessage({ type: "lasso.start" });
    expect(layer()?.dataset.phase).toBe("drawing");
    paragraph.remove();
  });

  it("tears everything down when the grant is revoked, and serves only lasso.start after", async () => {
    await load();
    replies["lasso.capture"] = { token: TOKEN };
    const paragraph = selectText();
    fire(document, "mouseup", { button: 0, target: paragraph });
    await vi.runAllTimersAsync();
    expect(dots()).toHaveLength(1);
    onMessage({ type: "lasso.start" });
    draw([[100, 100], [300, 300]]);
    await vi.waitFor(() => expect(frame()).not.toBeNull());

    onChanged({ lassoAccess: { newValue: false } }, "local");
    expect(layer()).toBeNull();
    expect(frame()).toBeNull();
    expect(dots()).toEqual([]);
    expect(sendMessage).toHaveBeenLastCalledWith({ type: "lasso.cancel", token: TOKEN });
    fire(document, "mouseup", { button: 0, target: paragraph });
    await vi.runAllTimersAsync();
    expect(dots()).toEqual([]);
    pressTwice("Alt");
    expect(layer()).toBeNull();
    onMessage({ type: "lasso.start" });
    expect(layer()?.dataset.phase).toBe("drawing");
    fire(window, "keydown", { key: "Escape" });

    // Granting it again brings the dot back.
    onChanged({ lassoAccess: { newValue: true } }, "local");
    fire(document, "mouseup", { button: 0, target: paragraph });
    await vi.runAllTimersAsync();
    expect(dots()).toHaveLength(1);
    paragraph.remove();
  });

  it("closes what a feature opened when its toggle goes off", async () => {
    await load();
    pressTwice("Alt");
    expect(layer()?.dataset.phase).toBe("drawing");
    onChanged({ lassoSettings: { newValue: { ...DEFAULT_LASSO, region: { ...DEFAULT_LASSO.region, enabled: false } } } }, "local");
    expect(layer()).toBeNull();
    pressTwice("Alt");
    expect(layer()).toBeNull();
    // The Capture menu item works whatever the toggle says, and a settings change leaves its overlay alone.
    onMessage({ type: "lasso.start" });
    onChanged({ lassoSettings: { newValue: { ...DEFAULT_LASSO, region: { ...DEFAULT_LASSO.region, enabled: false }, text: { ...DEFAULT_LASSO.text, enabled: false } } } }, "local");
    expect(layer()?.dataset.phase).toBe("drawing");
  });
});

describe("selection dot", () => {
  function select(text: string, anchor: Node): void {
    vi.spyOn(window, "getSelection").mockReturnValue({
      toString: () => text,
      rangeCount: 1,
      anchorNode: anchor,
      getRangeAt: () => ({ getClientRects: () => [{ right: 200, top: 50, height: 20 }] }),
    } as unknown as Selection);
  }
  const settle = () => vi.runAllTimersAsync();

  it("shows the dot after a selection and opens the shortcuts for it", async () => {
    await load();
    replies["lasso.selection"] = { token: TOKEN };
    const paragraph = document.createElement("p");
    paragraph.textContent = "Hello world";
    document.body.append(paragraph);
    select("Hello world", paragraph.firstChild!);
    fire(document, "mouseup", { button: 0, target: paragraph });
    await settle();
    const dot = root().querySelector<HTMLButtonElement>(".dot")!;
    expect(dot.getAttribute("aria-label")).toBe("Ask openside about this selection");
    expect(dot.style.left).toBe("204px");
    fire(dot, "click");
    await vi.waitFor(() => expect(frame()).not.toBeNull());
    expect(sendMessage).toHaveBeenCalledWith({ type: "lasso.selection", selection: "Hello world" });
    expect(root().querySelector(".dot")).toBeNull();
    // A press elsewhere on the page closes the shortcuts and drops the ask.
    fire(document, "mousedown", { target: paragraph });
    expect(frame()).toBeNull();
    expect(sendMessage).toHaveBeenLastCalledWith({ type: "lasso.cancel", token: TOKEN });
    paragraph.remove();
  });

  it("stays away from editable fields and when text selection is off", async () => {
    await load();
    const field = document.createElement("textarea");
    document.body.append(field);
    select("typed", field);
    fire(document, "mouseup", { button: 0, target: field });
    await settle();
    expect(roots[0]?.querySelector(".dot") ?? null).toBeNull();
    field.remove();

    await load({ ...DEFAULT_LASSO, text: { ...DEFAULT_LASSO.text, enabled: false } });
    select("Hello", document.body);
    fire(document, "mouseup", { button: 0, target: document.body });
    await settle();
    expect(roots.flatMap(shadow => [...shadow.querySelectorAll(".dot")])).toEqual([]);
  });
});
