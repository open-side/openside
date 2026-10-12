import { build } from "esbuild";
import { mkdir, copyFile, cp, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
const outdir = resolve(process.env.OPENSIDE_EXTENSION_OUTDIR ?? "dist");
const testMatches = process.env.OPENSIDE_EXTENSION_TEST_MATCHES === "1";
await mkdir(outdir, { recursive: true });
await build({ entryPoints: ["src/background.ts", "src/content.ts", "src/panel.ts", "src/options.ts"], outdir, bundle: true, format: "iife", target: "chrome126", define: { __TEST_MATCHES__: JSON.stringify(testMatches) } });
for (const file of ["panel.html", "options.html", "style.css"]) await copyFile(`static/${file}`, `${outdir}/${file}`);
// icon-16 is drawn with a wider ring-to-slab gap so the two stay apart at toolbar size.
await cp("static/icons", `${outdir}/icons`, { recursive: true });
// The code, login and choice cards and their stylesheet are shared with the web app; the notification icon is the app icon.
await copyFile("../web/src/otp/auth.css", `${outdir}/auth.css`);
await copyFile("../web/public/icons/icon-192.png", `${outdir}/icon.png`);
// Fonts are shared with the web app (OFL-1.1, texts included) and bundled so extension pages never load remote fonts. D2Coding Bold is not used here.
const fonts = resolve("../web/src/assets/fonts");
await cp(`${fonts}/wanted-sans`, `${outdir}/fonts/wanted-sans`, { recursive: true });
await cp(`${fonts}/d2coding`, `${outdir}/fonts/d2coding`, { recursive: true, filter: source => !source.endsWith("D2Coding-Bold.woff2") });
await writeFile(`${outdir}/manifest.json`, JSON.stringify({
  manifest_version: 3, name: testMatches ? "openside (test build)" : "openside", version: "0.1.0", minimum_chrome_version: "126",
  description: "Ask openside from this tab, selected messages and the address bar.",
  permissions: ["sidePanel", "storage", "tabs", "tabGroups", "activeTab", "scripting", "contextMenus", "alarms", "notifications", ...(testMatches ? ["debugger"] : [])],
  ...(testMatches ? {} : { optional_permissions: ["debugger"] }),
  optional_host_permissions: ["http://127.0.0.1/*", "http://localhost/*", "http://[::1]/*", "https://*/*"],
  ...(testMatches ? { host_permissions: ["http://127.0.0.1/*"] } : {}),
  background: { service_worker: "background.js" },
  icons: { 16: "icons/icon-16.png", 32: "icons/icon-32.png", 48: "icons/icon-48.png", 128: "icons/icon-128.png" },
  action: { default_title: "Ask openside", default_icon: { 16: "icons/icon-16.png", 32: "icons/icon-32.png" } },
  side_panel: { default_path: "panel.html" }, options_page: "options.html", omnibox: { keyword: "os" },
  content_scripts: [{ matches: ["https://mail.google.com/*", "https://app.slack.com/*", "https://*.slack.com/*", ...(testMatches ? ["http://127.0.0.1/*"] : [])], js: ["content.js"] }],
  content_security_policy: { extension_pages: "script-src 'self'; object-src 'none'" },
}, null, 2));
