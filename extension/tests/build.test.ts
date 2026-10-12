import { execFile } from "node:child_process";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { describe, it, expect } from "vitest";
const run = promisify(execFile);
describe("production extension artifact", () => {
  it("builds only TypeScript runtime sources and excludes mock content hosts", async () => {
    const directory = await mkdtemp(join(tmpdir(), "openside-extension-build-"));
    try {
      const sources = await readdir(join(process.cwd(), "src"));
      expect(sources.length).toBeGreaterThan(0);
      expect(sources.every(name => name.endsWith(".ts"))).toBe(true);
      const env: NodeJS.ProcessEnv = { ...process.env, OPENSIDE_EXTENSION_OUTDIR: directory };
      delete env.OPENSIDE_EXTENSION_TEST_MATCHES;
      await run("pnpm", ["build"], { cwd: process.cwd(), env });
      const manifest = JSON.parse(await readFile(join(directory, "manifest.json"), "utf8"));
      expect(manifest.manifest_version).toBe(3);
      expect(manifest.content_scripts[0].matches).toEqual(["https://mail.google.com/*", "https://app.slack.com/*", "https://*.slack.com/*"]);
      expect(manifest.host_permissions).toBeUndefined();
      expect(manifest.name).toBe("openside");
      expect(manifest.permissions).not.toContain("debugger");
      expect(manifest.optional_permissions).toEqual(["debugger"]);
      expect(manifest.omnibox.keyword).toBe("os");
      for (const font of ["fonts/wanted-sans/WantedSansVariable.css", "fonts/wanted-sans/OFL.txt", "fonts/wanted-sans/woff2/WantedSansVariable.split.0.woff2", "fonts/d2coding/D2Coding-Regular.woff2", "fonts/d2coding/OFL.txt"]) expect((await readFile(join(directory, font))).length).toBeGreaterThan(0);
      for (const page of ["panel.html", "options.html", "style.css"]) expect(await readFile(join(directory, page), "utf8")).not.toMatch(/(?:src|href)=["']https?:|url\(\s*["']?https?:/);
      for (const entry of ["background.js", "content.js", "panel.js", "options.js"]) expect((await readFile(join(directory, entry), "utf8")).length).toBeGreaterThan(0);
      await run("pnpm", ["build"], { cwd: process.cwd(), env: { ...env, OPENSIDE_EXTENSION_TEST_MATCHES: "1" } });
      const testManifest = JSON.parse(await readFile(join(directory, "manifest.json"), "utf8"));
      expect(testManifest.name).toBe("openside (test build)");
      expect(testManifest.permissions).toContain("debugger");
      expect(testManifest.content_scripts[0].matches).toContain("http://127.0.0.1/*");
      expect(testManifest.host_permissions).toEqual(["http://127.0.0.1/*"]);
    } finally { await rm(directory, { recursive: true, force: true }); }
  });
});
