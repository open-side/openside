import { captureRect, fitPixels, type Rect, type Size } from "./lasso-geometry";

/** One attachment as the daemon takes it (`AttachmentUpload`). */
export interface Upload { name: string; data_base64: string }

/**
 * Under the daemon's 2 MiB attachment limit (web Composer MAX_ATTACHMENT_BYTES) with room for the
 * rest of the message.
 */
export const MAX_CAPTURE_BYTES = 1_500_000;
/** Above this many pixels a crop is scaled down before encoding; a 4K viewport crop stays legible. */
const MAX_PIXELS = 4_000_000;

export function base64Of(bytes: Uint8Array): string {
  let binary = "";
  for (let start = 0; start < bytes.length; start += 0x8000) binary += String.fromCharCode(...bytes.subarray(start, start + 0x8000));
  return btoa(binary);
}

async function encode(bitmap: ImageBitmap, size: Size, type: "image/png" | "image/jpeg"): Promise<Blob> {
  const canvas = new OffscreenCanvas(size.width, size.height);
  const context = canvas.getContext("2d");
  if (!context) throw new Error("No 2D canvas");
  context.drawImage(bitmap, 0, 0, size.width, size.height);
  return canvas.convertToBlob(type === "image/jpeg" ? { type, quality: 0.9 } : { type });
}

/**
 * Crops the visible-tab capture (a data: URL) to the person's selection, given in CSS pixels of
 * `viewport`. PNG keeps text and code sharp; a crop too large for the daemon is scaled down, and
 * a photo-like one that still does not fit becomes JPEG.
 */
export async function cropCapture(dataUrl: string, selection: Rect, viewport: Size): Promise<Upload> {
  const source = await (await fetch(dataUrl)).blob();
  const full = await createImageBitmap(source);
  const crop = captureRect(selection, viewport, { width: full.width, height: full.height });
  full.close();
  const bitmap = await createImageBitmap(source, crop.x, crop.y, crop.width, crop.height);
  try {
    let size = fitPixels(crop, MAX_PIXELS);
    for (let attempt = 0; attempt < 4; attempt += 1) {
      const png = await encode(bitmap, size, "image/png");
      if (png.size <= MAX_CAPTURE_BYTES) return { name: "capture.png", data_base64: base64Of(new Uint8Array(await png.arrayBuffer())) };
      size = fitPixels(size, Math.floor(size.width * size.height * (MAX_CAPTURE_BYTES / png.size) * 0.9));
    }
    const jpeg = await encode(bitmap, size, "image/jpeg");
    if (jpeg.size > MAX_CAPTURE_BYTES) throw new Error("Capture too large");
    return { name: "capture.jpg", data_base64: base64Of(new Uint8Array(await jpeg.arrayBuffer())) };
  } finally {
    bitmap.close();
  }
}
