import { build } from "esbuild";
import { mkdir, copyFile, cp, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { EXTENSION_KEY } from "../web/src/desktopNotifyProtocol";
import { DEFAULT_DAEMON_ORIGIN, TEST_DAEMON_PATTERNS, daemonPattern } from "./src/daemonOrigin";
import { en } from "./src/i18n/en";
import { ko } from "./src/i18n/ko";
const outdir = resolve(process.env.OPENSIDE_EXTENSION_OUTDIR ?? "dist");
const testMatches = process.env.OPENSIDE_EXTENSION_TEST_MATCHES === "1";
// tools/openside-browser rewrites this to the daemon's resolved listener on every launch.
const daemonPatterns = process.env.OPENSIDE_DAEMON_ORIGIN ? [daemonPattern(process.env.OPENSIDE_DAEMON_ORIGIN)] : testMatches ? TEST_DAEMON_PATTERNS : [daemonPattern(DEFAULT_DAEMON_ORIGIN)];
await mkdir(outdir, { recursive: true });
await build({ entryPoints: ["src/background.ts", "src/content.ts", "src/panel.ts", "src/options.ts", "src/import-permissions.ts", "src/import-connect.ts", "src/site-access.ts", "src/lasso.ts", "src/ask.ts", "src/lasso-access.ts", "src/mic-access.ts"], outdir, bundle: true, format: "iife", target: "chrome126", define: { __TEST_MATCHES__: JSON.stringify(testMatches) } });
for (const file of ["panel.html", "options.html", "import-permissions.html", "import-connect.html", "site-access.html", "ask.html", "lasso-access.html", "mic-access.html", "style.css"]) await copyFile(`static/${file}`, `${outdir}/${file}`);
// icon-16 is drawn with a wider ring-to-slab gap so the two stay apart at toolbar size.
await cp("static/icons", `${outdir}/icons`, { recursive: true });
// Design tokens, the conversation cards and their stylesheet (and the avatar styles people blocks use) are shared with the web app; the notification icon is the app icon.
await copyFile("../web/src/tokens.css", `${outdir}/tokens.css`);
await copyFile("../web/src/cards/cards.css", `${outdir}/cards.css`);
await copyFile("../web/src/components/team.css", `${outdir}/team.css`);
await copyFile("../web/public/icons/icon-192.png", `${outdir}/icon.png`);
// Fonts are shared with the web app (OFL-1.1, texts included) and bundled so extension pages never load remote fonts. D2Coding Bold is not used here.
const fonts = resolve("../web/src/assets/fonts");
await cp(`${fonts}/wanted-sans`, `${outdir}/fonts/wanted-sans`, { recursive: true });
await cp(`${fonts}/d2coding`, `${outdir}/fonts/d2coding`, { recursive: true, filter: source => !source.endsWith("D2Coding-Bold.woff2") });
// Chrome reads manifest copy from _locales in the browser's UI language; the strings come from the extension catalog. The name is the product name in every language.
for (const [locale, catalog] of [["en", en], ["ko", ko]] as const) {
  await mkdir(`${outdir}/_locales/${locale}`, { recursive: true });
  await writeFile(`${outdir}/_locales/${locale}/messages.json`, JSON.stringify({
    description: { message: catalog["ext.manifest.description"] },
    actionTitle: { message: catalog["ext.manifest.actionTitle"] },
    miniPopupCommand: { message: catalog["ext.manifest.miniPopupCommand"] },
    paletteCommand: { message: catalog["ext.manifest.paletteCommand"] },
    lassoCommand: { message: catalog["ext.manifest.lassoCommand"] },
  }, null, 2));
}
await writeFile(`${outdir}/manifest.json`, JSON.stringify({
  manifest_version: 3, name: testMatches ? "openside (test build)" : "openside", version: "0.1.0", minimum_chrome_version: "141", key: EXTENSION_KEY, default_locale: "en",
  description: "__MSG_description__",
  // sessions, downloads and management let the person's openside shell list recently closed tabs, downloads and extensions;
  // browsingData backs its Delete browsing data dialog.
  permissions: ["sidePanel", "storage", "tabs", "tabGroups", "activeTab", "scripting", "contextMenus", "alarms", "notifications", "bookmarks", "history", "cookies", "sessions", "downloads", "management", "browsingData", ...(testMatches ? ["debugger"] : [])],
  ...(testMatches ? {} : { optional_permissions: ["debugger"] }),
  // Only the daemon on this computer is reachable without a prompt, so a browser opened by the desktop launcher pairs itself
  // (a prompt cannot be answered before anything is paired, nor at all in a headless run). Other daemons, and other
  // loopback ports, are asked for.
  host_permissions: daemonPatterns,
  // Reading or acting in a person tab attached to a chat needs access to its site, asked for per site. Lasso's selection
  // dot and double-press hotkey run on every page and capture without a click on the extension, which only <all_urls>
  // allows; the person grants it from lasso-access.html. The Capture menu item and the command key need none of it (activeTab).
  optional_host_permissions: ["https://*/*", "http://*/*", "<all_urls>"],
  background: { service_worker: "background.js" },
  // The web app's Settings toggle controls desktop notifications; only that daemon's UI may ask.
  externally_connectable: { matches: daemonPatterns },
  icons: { 16: "icons/icon-16.png", 32: "icons/icon-32.png", 48: "icons/icon-48.png", 128: "icons/icon-128.png" },
  action: { default_title: "__MSG_actionTitle__", default_icon: { 16: "icons/icon-16.png", 32: "icons/icon-32.png" } },
  side_panel: { default_path: "panel.html" }, options_page: "options.html", omnibox: { keyword: "os" },
  // The action opens the side panel (openPanelOnActionClick), and clicking it again closes it, so its shortcut toggles the panel.
  // Chrome keeps Ctrl+E for its own search shortcut and will not assign it from here; tools/openside-browser binds it in the profile.
  // The palette's Ctrl+Shift+A (⇧⌘A) is Chrome's tab search, so it carries no suggestion either and the launcher binds it the same
  // way. The mini popup works from anywhere (global); Chrome only suggests Ctrl+Shift+digit there, so Settings › Mini popup binds
  // Alt+Space through the patched browser when the person turns it on, and a stock browser takes it from chrome://extensions/shortcuts.
  commands: {
    _execute_action: { suggested_key: { default: "Ctrl+E", mac: "Command+E" } },
    "mini-popup": { description: "__MSG_miniPopupCommand__", global: true },
    "open-palette": { description: "__MSG_paletteCommand__" },
    // Lasso's capture from the keyboard. The command grants activeTab like the menu item, so it works without site access.
    "start-lasso": { suggested_key: { default: "Alt+Shift+L", mac: "Alt+Shift+L" }, description: "__MSG_lassoCommand__" },
  },
  content_scripts: [{ matches: ["https://mail.google.com/*", "https://app.slack.com/*", "https://*.slack.com/*", ...(testMatches ? ["http://127.0.0.1/*"] : [])], js: ["content.js"] }],
  // The lasso popover is an extension page framed into the page; only its document is listed (its scripts, styles and fonts
  // load from the extension's own origin), and the per-session URL keeps pages from probing for the extension.
  web_accessible_resources: [{ resources: ["ask.html"], matches: ["http://*/*", "https://*/*"], use_dynamic_url: true }],
  content_security_policy: { extension_pages: "script-src 'self'; object-src 'none'" },
}, null, 2));
