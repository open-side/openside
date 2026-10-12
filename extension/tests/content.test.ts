import { afterEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
const observers: MutationObserver[] = [];
afterEach(() => { observers.forEach(observer => observer.disconnect()); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it("keeps the action in a closed shadow root and rejects synthetic page clicks", async () => {
  document.body.innerHTML = readFileSync("tests/fixtures/gmail.html", "utf8");
  const sendMessage = vi.fn();
  vi.stubGlobal("chrome", { runtime: { sendMessage } });
  vi.stubGlobal("location", { hostname: "mail.google.com", pathname: "/mail/u/0/", href: "https://mail.google.com/mail/u/0/#inbox/mock" });
  const NativeObserver = MutationObserver;
  vi.stubGlobal("MutationObserver", class extends NativeObserver {
    constructor(callback: MutationCallback) { super(callback); observers.push(this); }
  });
  const attachShadow = Element.prototype.attachShadow;
  let shadow: ShadowRoot | undefined;
  vi.spyOn(Element.prototype, "attachShadow").mockImplementation(function (this: Element, options) { shadow = attachShadow.call(this, options); return shadow; });
  await import("../src/content");
  const host = document.querySelector<HTMLElement>("#openside-handle-host")!;
  expect(host.shadowRoot).toBeNull();
  host.click();
  const button = shadow!.querySelector<HTMLButtonElement>("button")!;
  button.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  button.click();
  expect(sendMessage).not.toHaveBeenCalled();
});
