import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

// jsdom has no object URLs; components only need distinct opaque strings.
let objectUrls = 0;
URL.createObjectURL ??= () => `blob:test/${++objectUrls}`;
URL.revokeObjectURL ??= () => undefined;

// jsdom lacks PointerEvent, so fireEvent.pointer* would drop clientX/button; a MouseEvent
// subclass carries the same coordinates the takeover screen reads.
globalThis.PointerEvent ??= class PointerEvent extends MouseEvent {} as typeof globalThis.PointerEvent;

// On Node >= 25, Response.blob() yields jsdom's Blob, which has no text().
Blob.prototype.text ??= function text(this: Blob) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error);
    reader.readAsText(this);
  });
};
