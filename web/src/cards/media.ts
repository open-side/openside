import type { CardPort } from "./actions";
import { isCardMediaPath } from "./values";

const MAX_IMAGE_BYTES = 2 * 1024 * 1024;
const RASTER = /^image\/(png|webp|jpeg|gif)$/;

/**
 * Loads the images of one mounted card through the authenticated client and revokes every object
 * URL when the card goes away. Nothing here fetches a remote URL: an image is either a daemon
 * artifact or a daemon-normalized favicon.
 */
export class Media {
  private readonly urls = new Set<string>();
  private disposed = false;

  constructor(
    private readonly port: CardPort,
    private readonly sessionId: string | undefined,
  ) {}

  private async objectUrl(blob: Blob): Promise<string | null> {
    if (this.disposed || blob.size > MAX_IMAGE_BYTES || !RASTER.test(blob.type)) return null;
    const url = URL.createObjectURL(blob);
    this.urls.add(url);
    return url;
  }

  /** Daemon-cached icon for a verified origin, or `null` when there is none to draw. */
  async favicon(origin: string): Promise<string | null> {
    if (!this.port.faviconBlob) return null;
    return this.objectUrl(await this.port.faviconBlob(origin));
  }

  /** A card image artifact of the current session. */
  async artifact(path: string): Promise<string | null> {
    if (!this.port.artifactBlob || !this.sessionId || !isCardMediaPath(path)) return null;
    return this.objectUrl(await this.port.artifactBlob(this.sessionId, path));
  }

  /** Sets `img.src` when the load succeeds; `onFail` runs for every other outcome. */
  attach(img: HTMLImageElement, load: Promise<string | null>, onFail: () => void): void {
    load
      .then((url) => {
        if (this.disposed) return;
        if (url) img.src = url;
        else onFail();
      })
      .catch(() => {
        if (!this.disposed) onFail();
      });
  }

  dispose(): void {
    this.disposed = true;
    for (const url of this.urls) URL.revokeObjectURL(url);
    this.urls.clear();
  }
}
