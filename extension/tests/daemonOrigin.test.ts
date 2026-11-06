import { describe, expect, it } from "vitest";
import { daemonPattern, trustedDaemonOrigin } from "../src/daemonOrigin";

describe("trusted daemon origin", () => {
  it("trusts exactly the origin and port the manifest names", () => {
    const patterns = ["http://127.0.0.1:47820/*"];
    expect(trustedDaemonOrigin("http://127.0.0.1:47820", patterns)).toBe("http://127.0.0.1:47820");
    expect(trustedDaemonOrigin("http://127.0.0.1:47820/", patterns)).toBe("http://127.0.0.1:47820");
    // Another loopback port, host spelling, scheme or a path is someone else.
    for (const other of ["http://127.0.0.1:47821", "http://127.0.0.1", "http://localhost:47820", "http://[::1]:47820", "https://127.0.0.1:47820", "http://127.0.0.1:47820/page", "http://evil.test:47820", undefined, "", "not a url"]) {
      expect(trustedDaemonOrigin(other, patterns), String(other)).toBeNull();
    }
  });
  it("treats IPv6 loopback the same way, brackets included", () => {
    const patterns = [daemonPattern("http://[::1]:47901")];
    expect(patterns).toEqual(["http://[::1]:47901/*"]);
    expect(trustedDaemonOrigin("http://[::1]:47901", patterns)).toBe("http://[::1]:47901");
    expect(trustedDaemonOrigin("http://[::1]:47902", patterns)).toBeNull();
    expect(trustedDaemonOrigin("http://127.0.0.1:47901", patterns)).toBeNull();
  });
  it("lets a port-less test pattern match any port of that host only", () => {
    const patterns = ["http://127.0.0.1/*"];
    expect(trustedDaemonOrigin("http://127.0.0.1:39123", patterns)).toBe("http://127.0.0.1:39123");
    expect(trustedDaemonOrigin("http://localhost:39123", patterns)).toBeNull();
  });
  it("builds a pattern only for a loopback http origin with a port", () => {
    for (const bad of ["http://127.0.0.1", "https://127.0.0.1:47820", "http://192.0.2.1:47820", "http://127.0.0.1:47820/path", "http://example.test:47820"]) {
      expect(() => daemonPattern(bad), bad).toThrow();
    }
  });
});
