/**
 * Keeps a long-lived daemon connection (event stream, WebSocket) open only while this document
 * is the one on screen.
 *
 * Chromium moves a page it navigates away from into the back/forward cache with its fetch
 * streams and WebSockets still connected. The daemon speaks HTTP/1.1, so a handful of cached
 * pages each holding a session stream fill Chromium's six-connections-per-host pool, and every
 * later request to the daemon, the next document's included, queues forever. `pagehide` is
 * the last moment a page can let go; `pageshow` with `persisted` is its return from the cache.
 *
 * `connect` runs now and again after each restore; the function it returns tears that
 * connection down on `pagehide`. The returned function closes for good.
 */
export function whilePageShown(connect: () => () => void): () => void {
  let teardown: (() => void) | null = connect();
  if (typeof window === "undefined") return () => teardown?.();
  const hide = () => {
    teardown?.();
    teardown = null;
  };
  const show = (event: PageTransitionEvent) => {
    if (event.persisted && !teardown) teardown = connect();
  };
  window.addEventListener("pagehide", hide);
  window.addEventListener("pageshow", show);
  return () => {
    window.removeEventListener("pagehide", hide);
    window.removeEventListener("pageshow", show);
    hide();
  };
}
