/**
 * The one daemon on this computer the extension trusts without asking: the manifest names its exact
 * origin in host_permissions and externally_connectable. tools/openside-browser rewrites both to the
 * daemon's resolved listener on every launch; a plain build uses the CLI's default listener.
 */
export const DEFAULT_DAEMON_ORIGIN = "http://127.0.0.1:47820";
/** Test builds load into disposable profiles next to daemons on random ports, so any port of these hosts. */
export const TEST_DAEMON_PATTERNS = ["http://127.0.0.1/*", "http://localhost/*"];
const PATTERN = /^http:\/\/(127\.0\.0\.1|localhost|\[::1\])(?::(\d{1,5}))?\/\*$/;

/** The match pattern for exactly this loopback http origin, or a thrown error for anything else. */
export function daemonPattern(origin: string): string {
  const url = new URL(origin);
  const pattern = `${url.origin}/*`;
  if (url.href !== `${url.origin}/` || !url.port || !PATTERN.test(pattern)) throw new Error(`not a loopback daemon origin with a port: ${origin}`);
  return pattern;
}

/**
 * `value` as an origin when the manifest trusts it, else null. A pattern with a port matches that
 * port only (an absent port in the page origin is 80); one without a port matches any port, as in Chrome.
 */
export function trustedDaemonOrigin(value: string | undefined, patterns: readonly string[] = chrome.runtime.getManifest().externally_connectable?.matches ?? []): string | null {
  if (!value) return null;
  let url: URL;
  try { url = new URL(value); } catch { return null; }
  if (url.protocol !== "http:" || url.origin !== value.replace(/\/$/, "")) return null;
  const port = url.port || "80";
  return patterns.some(pattern => {
    const match = PATTERN.exec(pattern);
    return match !== null && match[1] === url.hostname && (match[2] === undefined || match[2] === port);
  }) ? url.origin : null;
}
