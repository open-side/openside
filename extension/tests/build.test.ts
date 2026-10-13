import { execFile } from "node:child_process";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { promisify } from "node:util";
import { describe, it, expect } from "vitest";
import { EXTENSION_ID, EXTENSION_KEY } from "../../web/src/desktopNotifyProtocol";
const run = promisify(execFile);
/** Chromium's extension id: the first 128 bits of SHA-256 over the DER public key, hex digits shifted to a-p. */
const extensionIdOf = (key: string) => [...createHash("sha256").update(Buffer.from(key, "base64")).digest("hex").slice(0, 32)].map(digit => String.fromCharCode(97 + parseInt(digit, 16))).join("");
describe("production extension artifact", () => {
  it("builds only TypeScript runtime sources and excludes mock content hosts", async () => {
    const directory = await mkdtemp(join(tmpdir(), "openside-extension-build-"));
    try {
      const sources = await readdir(join(process.cwd(), "src"));
      expect(sources.length).toBeGreaterThan(0);
      expect(sources.every(name => name.endsWith(".ts"))).toBe(true);
      const env: NodeJS.ProcessEnv = { ...process.env, OPENSIDE_EXTENSION_OUTDIR: directory };
      delete env.OPENSIDE_EXTENSION_TEST_MATCHES;
      await run(process.platform === "win32" ? "cmd.exe" : "pnpm", process.platform === "win32" ? ["/d", "/s", "/c", "pnpm build"] : ["build"], { cwd: process.cwd(), env });
      const manifest = JSON.parse(await readFile(join(directory, "manifest.json"), "utf8"));
      expect(manifest.manifest_version).toBe(3);
      expect(manifest.content_scripts[0].matches).toEqual(["https://mail.google.com/*", "https://app.slack.com/*", "https://*.slack.com/*"]);
      expect(manifest.host_permissions).toBeUndefined();
      expect(manifest.name).toBe("openside");
      expect(manifest.permissions).not.toContain("debugger");
      expect(manifest.optional_permissions).toEqual(["debugger"]);
      expect(manifest.permissions).toEqual(expect.arrayContaining(["bookmarks", "history", "cookies"]));
      expect(manifest.optional_host_permissions).toEqual(["http://127.0.0.1/*", "http://localhost/*", "http://[::1]/*", "https://*/*"]);
      expect(manifest.omnibox.keyword).toBe("os");
      // The web app addresses the extension by this id; only the daemon UI on this computer may message it.
      expect(manifest.key).toBe(EXTENSION_KEY);
      expect(manifest.externally_connectable).toEqual({ matches: ["http://127.0.0.1/*", "http://localhost/*"] });
      expect(extensionIdOf(manifest.key)).toBe(EXTENSION_ID);
      for (const font of ["fonts/wanted-sans/WantedSansVariable.css", "fonts/wanted-sans/OFL.txt", "fonts/wanted-sans/woff2/WantedSansVariable.split.0.woff2", "fonts/d2coding/D2Coding-Regular.woff2", "fonts/d2coding/OFL.txt"]) expect((await readFile(join(directory, font))).length).toBeGreaterThan(0);
      // The panel draws cards with the web app's renderer and stylesheet, never a copy or a remote one.
      const panelPage = await readFile(join(directory, "panel.html"), "utf8");
      expect(panelPage).toContain('href="cards.css"');
      for (const page of ["panel.html", "options.html", "style.css", "cards.css"]) expect(await readFile(join(directory, page), "utf8")).not.toMatch(/(?:src|href)=["']https?:|url\(\s*["']?https?:/);
      expect(await readFile(join(directory, "cards.css"), "utf8")).toContain(".oc-card");
      const options = await readFile(join(directory, "options.html"), "utf8");
      expect(options).toContain("openside token create --person --scope read --scope steer --scope approve");
      expect(options).toContain("never give it to an agent");
      for (const entry of ["background.js", "content.js", "panel.js", "options.js"]) expect((await readFile(join(directory, entry), "utf8")).length).toBeGreaterThan(0);
      await run(process.platform === "win32" ? "cmd.exe" : "pnpm", process.platform === "win32" ? ["/d", "/s", "/c", "pnpm build"] : ["build"], { cwd: process.cwd(), env: { ...env, OPENSIDE_EXTENSION_TEST_MATCHES: "1" } });
      const testManifest = JSON.parse(await readFile(join(directory, "manifest.json"), "utf8"));
      expect(testManifest.name).toBe("openside (test build)");
      expect(testManifest.permissions).toContain("debugger");
      expect(testManifest.content_scripts[0].matches).toContain("http://127.0.0.1/*");
      expect(testManifest.externally_connectable).toEqual(manifest.externally_connectable);
      expect(testManifest.host_permissions).toEqual(["http://127.0.0.1/*"]);
    } finally { await rm(directory, { recursive: true, force: true }); }
  });
});
