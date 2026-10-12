import { useSyncExternalStore } from "react";
import { MAX_ATTACHMENT_BYTES } from "../components/Composer";
import { idbDelete, idbGet, SHARE_STORE } from "./idb";

/** What public/sw.js parks in IndexedDB for a share-target POST. */
export interface SharedItem {
  id: string;
  title: string;
  text: string;
  url: string;
  files: { name: string; type: string; data: ArrayBuffer }[];
  received_at: number;
}

/** What the new-chat composer starts with after something was shared to the app. */
export interface ShareDraft {
  id: string;
  text: string;
  files: File[];
}

/** A share nobody opened within this long is dropped instead of surprising the person later. */
const SHARE_TTL_MS = 60 * 60 * 1000;
const MIN_QUALITY = 0.5;
const MAX_EDGE = 2048;

/**
 * Android shares a page as a title plus its URL, sometimes with the URL already inside `text`;
 * keep each distinct piece once, in reading order.
 */
export function shareDraftText({ title, text, url }: Pick<SharedItem, "title" | "text" | "url">): string {
  const kept: string[] = [];
  for (const part of [title, text, url].map((piece) => piece.trim())) {
    if (part && !kept.some((other) => other.includes(part))) kept.push(part);
  }
  return kept.join("\n");
}

/** Reads a parked share and removes it; `null` when it was already taken, expired or never existed. */
export async function takeShare(id: string, now = Date.now()): Promise<ShareDraft | null> {
  const item = await idbGet<SharedItem>(SHARE_STORE, id);
  if (!item) return null;
  await idbDelete(SHARE_STORE, id);
  if (now - item.received_at > SHARE_TTL_MS) return null;
  return { id, text: shareDraftText(item), files: item.files.map((file) => new File([file.data], file.name, { type: file.type })) };
}

/**
 * Phone photos are routinely several MiB while a message may carry 2 MiB in total, so a shared
 * image is re-encoded as JPEG (shrinking dimensions, then quality) until it fits its share of
 * the budget. Non-images and images that already fit are returned untouched.
 */
export async function fitFiles(files: File[], budget = MAX_ATTACHMENT_BYTES): Promise<File[]> {
  if (files.reduce((sum, file) => sum + file.size, 0) <= budget) return files;
  const each = Math.floor(budget / files.length);
  return Promise.all(files.map((file) => (file.size > each && file.type.startsWith("image/") ? shrinkImage(file, each) : file)));
}

async function shrinkImage(file: File, limit: number): Promise<File> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    return file;
  }
  try {
    let scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height));
    for (;;) {
      const canvas = new OffscreenCanvas(Math.max(1, Math.round(bitmap.width * scale)), Math.max(1, Math.round(bitmap.height * scale)));
      const context = canvas.getContext("2d");
      if (!context) return file;
      // JPEG has no alpha; paint white so transparent screenshots do not turn black.
      context.fillStyle = "#fff";
      context.fillRect(0, 0, canvas.width, canvas.height);
      context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
      for (let quality = 0.85; quality >= MIN_QUALITY; quality -= 0.15) {
        const blob = await canvas.convertToBlob({ type: "image/jpeg", quality });
        if (blob.size <= limit) return new File([blob], file.name.replace(/\.[^.]+$/, "") + ".jpg", { type: "image/jpeg" });
      }
      scale *= 0.7;
      if (scale * Math.max(bitmap.width, bitmap.height) < 320) return file;
    }
  } finally {
    bitmap.close();
  }
}

// The draft outlives the route change that delivers it to the new-chat screen.
let draft: ShareDraft | null = null;
const listeners = new Set<() => void>();

export function setShareDraft(next: ShareDraft | null): void {
  draft = next;
  listeners.forEach((listener) => listener());
}

export function useShareDraft(): ShareDraft | null {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => void listeners.delete(listener);
    },
    () => draft,
  );
}
