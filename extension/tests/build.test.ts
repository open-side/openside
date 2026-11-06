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
      const sources = (await readdir(join(process.cwd(), "src"), { withFileTypes: true, recursive: true })).filter(entry => entry.isFile());
      expect(sources.length).toBeGreaterThan(0);
      expect(sources.every(entry => entry.name.endsWith(".ts"))).toBe(true);
      const env: NodeJS.ProcessEnv = { ...process.env, OPENSIDE_EXTENSION_OUTDIR: directory };
      delete env.OPENSIDE_EXTENSION_TEST_MATCHES;
      delete env.OPENSIDE_DAEMON_ORIGIN;
      await run(process.platform === "win32" ? "cmd.exe" : "pnpm", process.platform === "win32" ? ["/d", "/s", "/c", "pnpm build"] : ["build"], { cwd: process.cwd(), env });
      const manifest = JSON.parse(await readFile(join(directory, "manifest.json"), "utf8"));
      expect(manifest.manifest_version).toBe(3);
      expect(manifest.content_scripts[0].matches).toEqual(["https://mail.google.com/*", "https://app.slack.com/*", "https://*.slack.com/*"]);
      // Only the daemon on this computer, at its exact origin, needs no prompt (a launcher-opened browser pairs itself);
      // every other host, other loopback ports included, is asked for. The launcher rewrites it to the resolved listener.
      expect(manifest.host_permissions).toEqual(["http://127.0.0.1:47820/*"]);
      // Ctrl+E / ⌘E runs the action, which toggles the side panel. The palette and the mini popup carry no suggested key:
      // the launcher binds the palette's Ctrl+Shift+A, and Settings › Mini popup binds the global Alt+Space.
      expect(manifest.commands).toMatchObject({
        _execute_action: { suggested_key: { default: "Ctrl+E", mac: "Command+E" } },
        "mini-popup": { description: "__MSG_miniPopupCommand__", global: true },
        "open-palette": { description: "__MSG_paletteCommand__" },
        // Lasso's capture key; the command grants activeTab, so it needs no site access.
        "start-lasso": { suggested_key: { default: "Alt+Shift+L", mac: "Alt+Shift+L" }, description: "__MSG_lassoCommand__" },
      });
      expect(manifest.commands["mini-popup"].suggested_key).toBeUndefined();
      expect(manifest.commands["open-palette"].suggested_key).toBeUndefined();
      // Pages may frame only the lasso popover, through a per-session URL; the launcher's one-time code file is never readable.
      expect(manifest.web_accessible_resources).toEqual([{ resources: ["ask.html"], matches: ["http://*/*", "https://*/*"], use_dynamic_url: true }]);
      expect(manifest.name).toBe("openside");
      // Description and toolbar title follow the browser's UI language through _locales.
      expect(manifest.default_locale).toBe("en");
      expect(manifest.description).toBe("__MSG_description__");
      expect(manifest.action.default_title).toBe("__MSG_actionTitle__");
      for (const locale of ["en", "ko"]) {
        const messages = JSON.parse(await readFile(join(directory, "_locales", locale, "messages.json"), "utf8"));
        expect(Object.keys(messages)).toEqual(expect.arrayContaining(["actionTitle", "description", "miniPopupCommand", "paletteCommand", "lassoCommand"]));
        for (const entry of Object.values(messages) as { message: string }[]) expect(entry.message.length).toBeGreaterThan(0);
      }
      expect(JSON.parse(await readFile(join(directory, "_locales/en/messages.json"), "utf8")).actionTitle.message).toBe("Ask openside");
      expect(manifest.permissions).not.toContain("debugger");
      expect(manifest.optional_permissions).toEqual(["debugger"]);
      expect(manifest.permissions).toEqual(expect.arrayContaining(["bookmarks", "history", "cookies", "sessions", "downloads", "management", "browsingData"]));
      // Every site is only ever asked for: per site for person tabs, all at once for Lasso's dot and hotkey.
      expect(manifest.optional_host_permissions).toEqual(["https://*/*", "http://*/*", "<all_urls>"]);
      expect(manifest.host_permissions).not.toContain("<all_urls>");
      expect(manifest.omnibox.keyword).toBe("os");
      // The web app addresses the extension by this id; only the daemon UI at that same origin may message it.
      expect(manifest.key).toBe(EXTENSION_KEY);
      expect(manifest.externally_connectable).toEqual({ matches: ["http://127.0.0.1:47820/*"] });
      expect(extensionIdOf(manifest.key)).toBe(EXTENSION_ID);
      for (const font of ["fonts/wanted-sans/WantedSansVariable.css", "fonts/wanted-sans/OFL.txt", "fonts/wanted-sans/woff2/WantedSansVariable.split.0.woff2", "fonts/d2coding/D2Coding-Regular.woff2", "fonts/d2coding/OFL.txt"]) expect((await readFile(join(directory, font))).length).toBeGreaterThan(0);
      // The panel draws cards with the web app's renderer and stylesheet, never a copy or a remote one.
      const panelPage = await readFile(join(directory, "panel.html"), "utf8");
      expect(panelPage).toContain('href="cards.css"');
      // Tokens come from the web app's only token source, and every page loads them before its own styles.
      expect(await readFile(join(directory, "tokens.css"), "utf8")).toBe(await readFile(join(process.cwd(), "../web/src/tokens.css"), "utf8"));
      for (const page of ["panel.html", "options.html", "import-permissions.html", "import-connect.html", "site-access.html", "ask.html", "lasso-access.html", "mic-access.html"]) {
        const html = await readFile(join(directory, page), "utf8");
        expect(html.indexOf('href="tokens.css"')).toBeGreaterThan(-1);
        expect(html.indexOf('href="tokens.css"')).toBeLessThan(html.indexOf('href="style.css"'));
      }
      for (const page of ["panel.html", "options.html", "ask.html", "lasso-access.html", "mic-access.html", "style.css", "cards.css", "tokens.css"]) expect(await readFile(join(directory, page), "utf8")).not.toMatch(/(?:src|href)=["']https?:|url\(\s*["']?https?:/);
      expect(await readFile(join(directory, "cards.css"), "utf8")).toContain(".oc-card");
      const options = await readFile(join(directory, "options.html"), "utf8");
      expect(options).toContain("openside token create --person --scope read --scope steer --scope approve");
      expect(options).toContain("never give it to an agent");
      for (const entry of ["background.js", "content.js", "panel.js", "options.js", "lasso.js", "ask.js", "lasso-access.js", "mic-access.js"]) expect((await readFile(join(directory, entry), "utf8")).length).toBeGreaterThan(0);
      await run(process.platform === "win32" ? "cmd.exe" : "pnpm", process.platform === "win32" ? ["/d", "/s", "/c", "pnpm build"] : ["build"], { cwd: process.cwd(), env: { ...env, OPENSIDE_EXTENSION_TEST_MATCHES: "1" } });
      const testManifest = JSON.parse(await readFile(join(directory, "manifest.json"), "utf8"));
      expect(testManifest.name).toBe("openside (test build)");
      expect(testManifest.permissions).toContain("debugger");
      expect(testManifest.content_scripts[0].matches).toContain("http://127.0.0.1/*");
      // Test daemons listen on random ports in disposable profiles.
      expect(testManifest.externally_connectable).toEqual({ matches: ["http://127.0.0.1/*", "http://localhost/*"] });
      expect(testManifest.host_permissions).toEqual(testManifest.externally_connectable.matches);
      await run(process.platform === "win32" ? "cmd.exe" : "pnpm", process.platform === "win32" ? ["/d", "/s", "/c", "pnpm build"] : ["build"], { cwd: process.cwd(), env: { ...env, OPENSIDE_DAEMON_ORIGIN: "http://[::1]:47901" } });
      const pinned = JSON.parse(await readFile(join(directory, "manifest.json"), "utf8"));
      expect(pinned.host_permissions).toEqual(["http://[::1]:47901/*"]);
      expect(pinned.externally_connectable).toEqual({ matches: ["http://[::1]:47901/*"] });
    } finally { await rm(directory, { recursive: true, force: true }); }
  });
});
