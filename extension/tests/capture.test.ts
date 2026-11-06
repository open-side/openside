import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { base64Of, cropCapture, MAX_CAPTURE_BYTES } from "../src/capture";

/** The encoded size each canvas reports, by type and pixel count. */
let sizeOf: (type: string, width: number, height: number) => number;
const bitmaps: { args: unknown[]; closed: boolean }[] = [];
const encodes: { type: string; width: number; height: number; quality?: number }[] = [];

class FakeCanvas {
  constructor(readonly width: number, readonly height: number) {}
  getContext() { return { drawImage: vi.fn() }; }
  async convertToBlob(options: { type: string; quality?: number }) {
    encodes.push({ type: options.type, width: this.width, height: this.height, ...(options.quality ? { quality: options.quality } : {}) });
    const size = sizeOf(options.type, this.width, this.height);
    return { size, arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer };
  }
}

beforeEach(() => {
  bitmaps.length = 0;
  encodes.length = 0;
  sizeOf = () => 1000;
  vi.stubGlobal("fetch", vi.fn(async () => ({ blob: async () => "png-blob" })));
  vi.stubGlobal("createImageBitmap", vi.fn(async (...args: unknown[]) => {
    const bitmap = { args, closed: false, width: 2000, height: 1600, close() { bitmap.closed = true; } };
    bitmaps.push(bitmap);
    return bitmap;
  }));
  vi.stubGlobal("OffscreenCanvas", FakeCanvas);
});
afterEach(() => vi.unstubAllGlobals());

describe("cropCapture", () => {
  it("crops the selection out of the capture in image pixels", async () => {
    const upload = await cropCapture("data:image/png;base64,AAAA", { x: 10, y: 20, width: 100, height: 50 }, { width: 1000, height: 800 });
    expect(bitmaps[1]!.args).toEqual(["png-blob", 20, 40, 200, 100]);
    expect(encodes).toEqual([{ type: "image/png", width: 200, height: 100 }]);
    expect(upload).toEqual({ name: "capture.png", data_base64: "AQID" });
    expect(bitmaps.every(bitmap => bitmap.closed)).toBe(true);
  });

  it("scales a PNG that is over the limit down until it fits", async () => {
    sizeOf = (_type, width, height) => width * height * 2;
    const upload = await cropCapture("data:image/png;base64,AAAA", { x: 0, y: 0, width: 1000, height: 800 }, { width: 1000, height: 800 });
    expect(upload.name).toBe("capture.png");
    expect(encodes.length).toBeGreaterThan(1);
    const last = encodes.at(-1)!;
    expect(last.width * last.height * 2).toBeLessThanOrEqual(MAX_CAPTURE_BYTES);
    expect(last.width / last.height).toBeCloseTo(1.25, 1);
  });

  it("falls back to JPEG when PNG stays too large", async () => {
    sizeOf = type => type === "image/png" ? MAX_CAPTURE_BYTES * 4 : MAX_CAPTURE_BYTES - 1;
    const upload = await cropCapture("data:image/png;base64,AAAA", { x: 0, y: 0, width: 500, height: 400 }, { width: 1000, height: 800 });
    expect(upload.name).toBe("capture.jpg");
    expect(encodes.filter(entry => entry.type === "image/png")).toHaveLength(4);
    expect(encodes.at(-1)).toMatchObject({ type: "image/jpeg", quality: 0.9 });
  });

  it("refuses a capture that fits in no format", async () => {
    sizeOf = () => MAX_CAPTURE_BYTES * 4;
    await expect(cropCapture("data:image/png;base64,AAAA", { x: 0, y: 0, width: 500, height: 400 }, { width: 1000, height: 800 })).rejects.toThrow("Capture too large");
    expect(bitmaps.every(bitmap => bitmap.closed)).toBe(true);
  });
});

describe("base64Of", () => {
  it("encodes large buffers in chunks", () => {
    const bytes = new Uint8Array(0x8000 * 2 + 3).map((_, index) => index % 251);
    expect(base64Of(bytes)).toBe(Buffer.from(bytes).toString("base64"));
  });
});
