#!/usr/bin/env bun
// C-7 / PP-19 QA: password-manager extensions next to openside, against a local Vaultwarden.
// Not meant to be run directly; tools/pwmgr-qa.sh prepares the server, extensions and cleanup.
//
//   bun tools/pwmgr-qa/run.mjs <stage> --label stock|patched --browser /path/to/chrome
//
// Stages: static, headless, native, onepassword. Results are appended to
// $OUT/results.jsonl (one JSON object per check) and screenshots land in $OUT/<label>/.
import { spawn, execFileSync } from "node:child_process";
import { appendFileSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Cdp, Daemon, sleep, until } from "./lib.mjs";
import { BitwardenUi, BW_ORIGIN } from "./bitwarden.mjs";

const REPO = resolve(import.meta.dir, "../..");
const W = process.env.PWMGR_WORK ?? "/tmp/openside-qa/pwmgr-work";
const OUT = process.env.PWMGR_OUT ?? "/tmp/openside-qa/pwmgr";
const VAULT = "https://127.0.0.1:18222";
const MOCK = "http://127.0.0.1:18223/login.html";
const creds = JSON.parse(readFileSync(`${W}/creds.json`, "utf8"));
const SPKI = readFileSync(`${W}/tls/spki.b64`, "utf8").trim();
const OPENSIDE_EXT = join(REPO, "extension/dist");

const [stage, ...rest] = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = rest.indexOf(`--${name}`);
  return i >= 0 ? rest[i + 1] : fallback;
};
const label = opt("label", "stock");
const browser = opt("browser", "/usr/bin/chromium");
const shots = join(OUT, label);
mkdirSync(shots, { recursive: true });

function record(id, check, status, detail) {
  const row = { id, label, check, status, detail };
  appendFileSync(join(OUT, "results.jsonl"), JSON.stringify(row) + "\n");
  console.log(`${status.padEnd(5)} ${id} ${check} :: ${detail}`);
}

/** Flags `Browser::launch` passes (crates/openside-browser/src/browser.rs); headless is the only omission. */
const DAEMON_FLAGS = [
  "--remote-debugging-port=0", "--remote-debugging-address=127.0.0.1",
  "--disable-backgrounding-occluded-windows", "--disable-renderer-backgrounding", "--disable-background-timer-throttling",
  "--password-store=basic", "--disable-sync", "--no-first-run", "--no-default-browser-check",
  "--disable-background-networking", "--disable-component-update", "--disable-domain-reliability",
  "--disable-client-side-phishing-detection", "--disable-default-apps", "--no-pings", "--dns-prefetch-disable",
  "--disable-features=OptimizationHints,MediaRouter,DialMediaRouteProvider,AutofillServerCommunication,NetworkTimeServiceQuerying,Translate,AimEnabled,PreconnectToSearch,PreconnectFromKeyedService",
  "--openside-isolation=1",
  "--gcm-checkin-url=http://127.0.0.1:9/checkin", "--gcm-registration-url=http://127.0.0.1:9/register", "--gcm-mcs-endpoint=https://127.0.0.1:9",
];

function assertFlagsMirrorSource() {
  const source = readFileSync(join(REPO, "crates/openside-browser/src/browser.rs"), "utf8");
  const stale = DAEMON_FLAGS.filter((flag) => !source.includes(`"${flag}"`) && !flag.startsWith("--openside-isolation"));
  if (stale.length) throw new Error(`DAEMON_FLAGS drifted from browser.rs: ${stale.join(" ")}`);
}

const secretValues = () => [creds.master, creds.site_pass, creds.site_user];

/** Fresh tree under W for a run; returns paths. */
function freshRoot(name) {
  const root = join(W, name);
  rmSync(root, { recursive: true, force: true });
  mkdirSync(root, { recursive: true });
  return root;
}

function killGroup(pid, signal = "SIGKILL") {
  try { process.kill(-pid, signal); } catch { /* gone */ }
}

async function waitForPort(profile) {
  await until("DevToolsActivePort", () => existsSync(`${profile}/DevToolsActivePort`) && readFileSync(`${profile}/DevToolsActivePort`, "utf8").includes("/devtools/"), 40000);
  const [port, path] = readFileSync(`${profile}/DevToolsActivePort`, "utf8").split("\n");
  return `ws://127.0.0.1:${port}${path}`;
}

/** Headless browser with the daemon's flags in a daemon-shaped profile (same flags as Browser::launch). */
async function launchBrowser({ exe, root, extensions, mirrorXdg = false, extraEnv = {} }) {
  const profile = join(root, "data/openside/browser-profile");
  mkdirSync(join(profile, "xdg-data"), { recursive: true });
  mkdirSync(join(profile, "xdg-cache"), { recursive: true });
  for (const dir of ["home", "config", "run"]) mkdirSync(join(root, dir), { recursive: true });
  const env = {
    PATH: process.env.PATH, HOME: join(root, "home"), XDG_CONFIG_HOME: join(root, "config"),
    XDG_RUNTIME_DIR: join(root, "run"),
    ...(mirrorXdg ? { XDG_DATA_HOME: join(profile, "xdg-data"), XDG_CACHE_HOME: join(profile, "xdg-cache") } : {}),
    ...extraEnv,
  };
  const paths = extensions.join(",");
  const args = [
    ...DAEMON_FLAGS, "--headless=new", `--ignore-certificate-errors-spki-list=${SPKI}`,
    `--user-data-dir=${profile}`, `--load-extension=${paths}`, `--disable-extensions-except=${paths}`, "about:blank",
  ];
  const child = spawn(exe, args, { env, stdio: ["ignore", "ignore", openSync(join(root, "browser.stderr"), "a")], detached: true });
  const wsUrl = await waitForPort(profile);
  return { child, pid: child.pid, profile, wsUrl };
}

const mockCredsMatch = (cdp, sid) => cdp.evaluate(sid, "[document.getElementById('username').value, document.getElementById('password').value]")
  .then(([user, pass]) => ({ user: user === creds.site_user, pass: pass === creds.site_pass, empty: user === "" && pass === "" }));

/**
 * Autofill through the popup opened as a tab. The activation requests below are refused in the daemon-managed
 * (agent) browser, where every DevTools client is the agent, so they do not decide which tab is active; the
 * results that follow are recorded as observations, not judged.
 */
async function autofillViaPopup(cdp, ui, mock) {
  await cdp.call("Target.activateTarget", { targetId: mock.targetId }).catch(() => {});
  await cdp.call("Page.bringToFront", {}, mock.sessionId).catch(() => {});
  await sleep(500);
  await ui.ev("location.hash = '#/tabs/vault'; true");
  await ui.ev("location.reload(); true").catch(() => {});
  await until("popup vault list", async () => (await ui.text()).includes("All items"), 40000, 500);
  await until("view item", () => ui.ev(`(()=>{const e=[...document.querySelectorAll('button')].find(e=>(e.getAttribute('aria-label')||'').startsWith('View item')); if(!e) return false; e.click(); return true})()`), 20000);
  await until("autofill button", () => ui.click("Autofill"), 20000);
  await sleep(2500);
}

function listTargets(cdp) {
  return cdp.targets();
}

// ---------------------------------------------------------------------------------------------

async function stageStatic() {
  const manifest = JSON.parse(readFileSync(join(OPENSIDE_EXT, "manifest.json"), "utf8"));
  const matches = (manifest.content_scripts ?? []).flatMap((s) => s.matches);
  const touches = (pattern) => matches.some((m) => m.startsWith(pattern));
  record("C-7", "openside extension content scripts never match a password-manager origin or localhost",
    matches.every((m) => /^https:\/\/(mail\.google\.com|app\.slack\.com|\*\.slack\.com)\//.test(m)) && !touches("http://127.0.0.1") ? "pass" : "fail", matches.join(", "));
  const perms = [...(manifest.permissions ?? []), ...(manifest.optional_permissions ?? [])];
  record("C-7", "openside extension has no management/nativeMessaging/cookies/webRequest permission (cannot read other extensions or their traffic)",
    ["management", "nativeMessaging", "cookies", "webRequest", "history", "clipboardRead"].every((p) => !perms.includes(p)) ? "pass" : "fail", perms.join(", "));
  const hits = execFileSync("grep", ["-rniE", "-l", "bitwarden|1password|nngceckbapebfimnlniiiahkandclblb|aeblfdkhhhdcdjpifhhbdiojplfjncoa", "crates", "extension/src", "agent/src"], { cwd: REPO, encoding: "utf8" }).split("\n").filter(Boolean)
    .filter((f) => !f.endsWith("native_messaging.rs"));
  record("C-7", "no openside crate/extension/agent source reads Bitwarden or 1Password state (only doctor's filename probe mentions them)", hits.length === 0 ? "pass" : "info", hits.join(", ") || "no references outside crates/openside-config/src/native_messaging.rs");
}

let scanned = 0;
/** Plain-text/binary scan of everything under `root` except `exclude` dirs for any of the secrets. */
function scanSecrets(root, exclude) {
  const found = [];
  const needles = secretValues().map((s) => Buffer.from(s));
  scanned = 0;
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (exclude.some((e) => path.startsWith(e))) continue;
      let info;
      try { info = statSync(path); } catch { continue; }
      if (info.isDirectory()) walk(path);
      else if (info.isFile() && info.size < 64 * 1024 * 1024) {
        scanned += 1;
        const data = readFileSync(path);
        needles.forEach((needle, i) => { if (data.includes(needle)) found.push(`${path} contains secret#${i}`); });
      }
    }
  };
  walk(root);
  return found;
}

function saveDoctor(root, file) {
  const env = { PATH: process.env.PATH, HOME: join(root, "home"), XDG_CONFIG_HOME: join(root, "config"), XDG_DATA_HOME: join(root, "data"), XDG_STATE_HOME: join(root, "state"), XDG_CACHE_HOME: join(root, "cache"), XDG_RUNTIME_DIR: join(root, "run") };
  let out;
  try {
    out = execFileSync(join(REPO, "target/debug/openside"), ["--socket", join(root, "run/opensided.sock"), "doctor"], { env, encoding: "utf8", timeout: 60000 });
  } catch (error) {
    out = `${error.stdout ?? ""}${error.stderr ?? ""}`;
  }
  writeFileSync(file, out);
  return out;
}

async function stageHeadless() {
  const root = freshRoot(`run-${label}-headless`);
  const daemon = new Daemon({ root, repo: REPO, executable: browser, extensions: [OPENSIDE_EXT, `${W}/ext/bitwarden`] });
  let cdp;
  try {
    await daemon.start();
    const session = await daemon.newSession();
    const opened = await daemon.repl(session, `await page.goto(${JSON.stringify(MOCK)}); page.url()`);
    const agentTab = opened.body?.result?.tab_id;
    record("C-7", `${label}: daemon-launched (headless) browser opens an agent tab with both extensions configured`, opened.body?.status === "done" && agentTab ? "pass" : "fail", `repl ${opened.status} ${opened.body?.status}`);
    cdp = await Cdp.connect(daemon.debugEndpoint());
    await sleep(2500);
    for (const t of (await listTargets(cdp)).filter((t) => t.url.includes("browser-start") || t.type === "iframe")) await cdp.call("Target.closeTarget", { targetId: t.targetId }).catch(() => {});
    const workers = (await listTargets(cdp)).filter((t) => t.type === "service_worker").map((t) => t.url.split("/")[2]);
    record("C-7", `${label}: managed browser loads openside's and Bitwarden's service workers side by side`, workers.length >= 2 && workers.includes("nngceckbapebfimnlniiiahkandclblb") ? "pass" : "fail", `workers: ${workers.join(", ")}`);
    const cmdline = execFileSync("ps", ["-o", "args=", "-p", String(await browserPid(daemon))], { encoding: "utf8" });
    record("C-7", `${label}: daemon launches the browser with --disable-extensions-except for exactly openside + Bitwarden`, cmdline.includes("--disable-extensions-except=") && cmdline.includes("/extension/dist,") && cmdline.includes("/ext/bitwarden") ? "pass" : "fail", cmdline.match(/--disable-extensions-except=\S+/)?.[0] ?? "flag missing");
    if (label === "patched") record("C-7", `patched: gateway switches present next to both extensions`, cmdline.includes("--openside-gateway-instance-id=") && cmdline.includes("--openside-agent-session-id=") ? "pass" : "fail", cmdline.match(/--openside-gateway-instance-id=\S+/)?.[0] ?? "no gateway switch");

    const agentSession = await cdp.attach(agentTab);
    const [agentUser, agentPass] = await cdp.evaluate(agentSession, "[document.getElementById('username').value, document.getElementById('password').value]");
    record("C-7", `${label}: agent tab's login form stays empty (Bitwarden never autofills on its own)`, agentUser === "" && agentPass === "" ? "pass" : "fail", "username and password inputs empty after load");
    await daemon.repl(session, `await page.locator('#username').click(); true`);
    await sleep(2000);
    const targets = await listTargets(cdp);
    const frames = targets.filter((t) => t.url.includes("overlay/menu")).map((t) => t.url.split("/").pop());
    record("C-7", `${label}: Bitwarden's content script is active in the agent tab (inline-menu frames appear on field focus)`, frames.length ? "pass" : "info", frames.join(",") || "no inline-menu frame (vault locked / not logged in)");
    const snap = await daemon.repl(session, `await page.locator('body').innerText()`);
    const text = snap.body?.result?.return_value;
    record("C-7", `${label}: page text returned to the agent is the page's own text (no injected PM UI)`, typeof text === "string" && text.includes("Sign in") && !/bitwarden/i.test(text) ? "pass" : "fail", JSON.stringify(text ?? snap.body?.result?.error).slice(0, 120));

    // A tab the user would own: created by this script over CDP, not through the daemon.
    const userTab = await cdp.page(MOCK);
    await sleep(1000);
    const ledger = await daemon.api("GET", "/v1/browser/tabs");
    const owned = (ledger.body?.tabs ?? []).map((t) => t.tab_id);
    record("C-7", `${label}: ownership ledger lists the agent tab and neither the user's tab nor extension pages`, owned.length === 1 && owned[0] === agentTab && !owned.includes(userTab.targetId) ? "pass" : "fail", `ledger=${JSON.stringify(ledger.body).slice(0, 260)}`);
    let ui = null;
    let popup = "responsive";
    try {
      ui = await new BitwardenUi(cdp, shots).open();
      await ui.text();
    } catch (error) {
      popup = `unusable: ${error.message}`;
      ui = null;
    }
    record("C-7", `${label}: Bitwarden popup UI is usable inside the daemon's --headless=new browser`, ui ? "pass" : "info", ui ? "popup rendered its login screen" : `${popup}; the page stays on the loading spinner, then Runtime.evaluate gets no reply (same in a plain headless Chromium without openside)`);
    let unlocked = false;
    if (ui) {
      // The daemon cannot pass --ignore-certificate-errors-spki-list, so trust the QA server's self-signed cert via CDP.
      await cdp.call("Security.setIgnoreCertificateErrors", { ignore: true });
      try {
        const route = await ui.login({ server: VAULT, email: creds.email, password: creds.master });
        unlocked = route.includes("/tabs/vault");
        record("C-7", `${label}: Bitwarden logs in to local Vaultwarden inside the daemon-managed browser`, unlocked ? "pass" : "fail", `route ${route}`);
      } catch (error) {
        record("C-7", `${label}: Bitwarden logs in to local Vaultwarden inside the daemon-managed browser`, "fail", error.message);
      }
    }
    if (unlocked) {
      const agentRef = { targetId: agentTab, sessionId: agentSession };
      const clear = (t) => cdp.evaluate(t.sessionId, "document.getElementById('username').value=''; document.getElementById('password').value=''; true");
      // Pass A: CDP asks for the user's tab to be in front (refused in an agent browser) before the user autofills.
      await autofillViaPopup(cdp, ui, userTab);
      const user = await mockCredsMatch(cdp, userTab.sessionId);
      const agentA = await mockCredsMatch(cdp, agentSession);
      // An agent browser refuses Target.activateTarget and Page.bringToFront for every tab, so neither pass controls
      // which tab is active; Bitwarden resolves "the active tab" itself. Recorded, not judged.
      record("C-7", `${label}: autofill after a refused CDP activation of the user's tab: which tab was filled`, "info", `user tab ${JSON.stringify(user)}; agent tab ${JSON.stringify(agentA)}`);
      if (user.user && user.pass) {
        await cdp.evaluate(userTab.sessionId, "document.getElementById('submit').click(); true");
        const submitted = JSON.parse(await cdp.evaluate(userTab.sessionId, "JSON.stringify(window.__submitted ?? null)"));
        record("C-7", `${label}: the filled user-tab form submits the stored credentials`, submitted?.password === creds.site_pass ? "pass" : "fail", "submitted payload equals vault item");
      }
      // Pass B (worst case): the user autofills while the agent's tab is the one in front. The page then holds real secrets.
      await clear(userTab);
      await autofillViaPopup(cdp, ui, agentRef);
      const agentB = await mockCredsMatch(cdp, agentSession);
      record("C-7", `${label}: autofill after a refused CDP activation of the agent's tab: which tab was filled`, "info", JSON.stringify(agentB));
      if (agentB.user && agentB.pass) {
        const reads = await daemon.repl(session, `({ pass: await page.locator('#password').inputValue(), user: await page.locator('#username').inputValue(), text: await page.locator('body').innerText() })`);
        const readsJson = JSON.stringify(reads.body);
            const has = secretValues().map((s) => readsJson.includes(s));
        record("C-7", `${label}: after autofill into an agent tab, REPL inputValue() on the password field does not return the password (Chromium masks password AX values)`, has[1] || has[0] ? "fail" : "pass", `repl status=${reads.body?.status}; reply contains master=${has[0]} item_password=${has[1]}`);
        record("C-7", `${label}: ...but a PM-filled non-password field (username) is returned to the agent and recorded in events.db`, has[2] ? "finding" : "pass", `reply contains item_username=${has[2]}; password-type fields are masked, other PM-filled fields are plain page state`);
      }
    }
    const leaks = scanSecrets(root, [daemon.profile]);
    const fatal = leaks.filter((l) => !l.endsWith("secret#2"));
    record("C-7", `${label}: scan of the openside-owned tree (data, audit, events, config, stderr; browser profile excluded): master password and item password absent`, fatal.length === 0 ? "pass" : "fail", leaks.length ? leaks.map((l) => l.replace(`${root}/`, "")).join("; ") : `scanned ${scanned} files for ${secretValues().length} secrets; vault ${unlocked ? "was unlocked, autofilled and submitted in this browser" : "was NOT unlocked in this run (popup unusable), so this only shows the scan is clean of anything the daemon had"}`);
    const planted = join(root, "state/planted-secret.txt");
    writeFileSync(planted, `x${creds.site_pass}x`);
    record("C-7", `${label}: scanner self-test (a planted secret is found)`, scanSecrets(root, [daemon.profile]).some((l) => l.startsWith(planted)) ? "pass" : "fail", "planted file detected");
    rmSync(planted);
    saveDoctor(root, `${shots}/doctor-headless.txt`);
  } finally {
    cdp?.close();
    await daemon.stop();
    await sleep(500);
    cleanupOwned(root);
  }
}

async function browserPid(daemon) {
  const state = JSON.parse(readFileSync(join(daemon.dataDir, "browser-state.json"), "utf8"));
  return state.endpoint.pid;
}

function cleanupOwned(root) {
  for (const name of readdirSync("/proc")) {
    if (!/^\d+$/.test(name)) continue;
    try {
      const cmd = readFileSync(`/proc/${name}/cmdline`, "utf8");
      if (cmd.includes(root)) process.kill(Number(name), "SIGKILL");
    } catch { /* gone */ }
  }
}

// ---- native messaging: official Bitwarden proxy binary against a stand-in for the desktop app -----------
//
// Running the Electron desktop app needs a window, which this suite must not open on a working session.
// The browser-side half of the connection is what openside influences, so it is exercised with the real
// `desktop_proxy` from the official release and a plain unix-socket listener where the desktop app's
// IPC socket would be ($XDG_CACHE_HOME/com.bitwarden.desktop/s.bw). The manifest content mirrors what the real
// desktop app wrote on 2026-10-12 (kept as observed-bitwarden-desktop-manifest.json in the evidence dir).

const NH = `${W}/nm-home`;
const DESKTOP = `${W}/desktop`;
const PROXY_NAME = ".bitwarden_desktop_proxy";

function startStandIn() {
  rmSync(NH, { recursive: true, force: true });
  const nmDir = `${NH}/.config/chromium/NativeMessagingHosts`;
  mkdirSync(nmDir, { recursive: true });
  mkdirSync(`${NH}/.cache/com.bitwarden.desktop`, { recursive: true });
  copyFileSync(`${DESKTOP}/desktop_proxy`, `${nmDir}/${PROXY_NAME}`);
  execFileSync("chmod", ["755", `${nmDir}/${PROXY_NAME}`]);
  writeFileSync(`${nmDir}/com.8bit.bitwarden.json`, JSON.stringify({
    name: "com.8bit.bitwarden", description: "Bitwarden desktop <-> browser bridge", path: `${nmDir}/${PROXY_NAME}`, type: "stdio",
    allowed_origins: ["chrome-extension://nngceckbapebfimnlniiiahkandclblb/", "chrome-extension://hccnnhgbibccigepcmlgppchkpfdophk/", "chrome-extension://jbkfoedolllekgbhcbcoahefnbanhhlh/", "chrome-extension://ccnckbpmaceehanjmeomladnmlffdjgn/"],
  }, null, 2));
  const listener = spawn("python3", ["-c", `import socket,sys,os
p=sys.argv[1]
s=socket.socket(socket.AF_UNIX); s.bind(p); s.listen(8)
while True:
    c,_=s.accept(); open(os.path.join(os.path.dirname(p),'accepted'),'a').write('1')
`, `${NH}/.cache/com.bitwarden.desktop/s.bw`], { stdio: "ignore", detached: true });
  return { stop: () => killGroup(listener.pid), accepted: () => (existsSync(`${NH}/.cache/com.bitwarden.desktop/accepted`) ? readFileSync(`${NH}/.cache/com.bitwarden.desktop/accepted`, "utf8").length : 0) };
}

const NM_PROBE = `new Promise((resolve) => { const out = { messages: [] }; let port;
  try { port = chrome.runtime.connectNative('com.8bit.bitwarden'); } catch (e) { resolve({ threw: String(e) }); return; }
  port.onMessage.addListener((m) => out.messages.push(JSON.stringify(m).slice(0, 80)));
  port.onDisconnect.addListener(() => { out.disconnected = (chrome.runtime.lastError && chrome.runtime.lastError.message) || 'no error'; resolve(out); });
  setTimeout(() => { out.disconnected = out.disconnected || false; try { port.disconnect(); } catch {} resolve(out); }, 4000); })`;

async function probeNative(cdp) {
  // The service worker can be mid-restart right after startup; retry until the extension API is reachable.
  return until("bitwarden sw with chrome.runtime", async () => {
    const worker = (await cdp.targets()).find((t) => t.type === "service_worker" && t.url.includes("nngceck"));
    if (!worker) return null;
    const sid = await cdp.attach(worker.targetId);
    if (!(await cdp.evaluate(sid, "typeof chrome !== 'undefined' && !!chrome.runtime && !!chrome.runtime.connectNative").catch(() => false))) return null;
    return cdp.evaluate(sid, NM_PROBE);
  }, 60000, 1000);
}

function parseDoctor(text) {
  const lines = text.split("\n").filter((l) => /^\[(ok|warn|fail)\] native_messaging\.(bitwarden|1password|managed_profile)/.test(l));
  return lines.map((l) => l.replace(/\s+/g, " ").slice(0, 230));
}

async function stageNative() {
  assertFlagsMirrorSource();
  const desktop = startStandIn();
  const nmExt = `${W}/ext/bitwarden-nm`;
  const manifestSrc = `${NH}/.config/chromium/NativeMessagingHosts/com.8bit.bitwarden.json`;
  try {
    const manifest = JSON.parse(readFileSync(manifestSrc, "utf8"));
    record("C-7", "host manifest (as the Bitwarden desktop app writes it) allows the store extension id", manifest.allowed_origins.includes("chrome-extension://nngceckbapebfimnlniiiahkandclblb/") ? "pass" : "fail", `${manifestSrc} -> ${manifest.path}`);

    // Doctor against the HOME the desktop app wrote into, before the managed profile has any manifest.
    let root = freshRoot(`run-${label}-nm`);
    mkdirSync(join(root, "data/openside/browser-profile/NativeMessagingHosts"), { recursive: true });
    const doctorEnv = (r) => ({ PATH: process.env.PATH, HOME: NH, XDG_CONFIG_HOME: `${NH}/.config`, XDG_DATA_HOME: join(r, "data"), XDG_STATE_HOME: join(r, "state"), XDG_CACHE_HOME: join(r, "cache"), XDG_RUNTIME_DIR: join(r, "run") });
    const doctor = (r, file) => {
      let out;
      try { out = execFileSync(join(REPO, "target/debug/openside"), ["--socket", join(r, "run/opensided.sock"), "doctor"], { env: doctorEnv(r), encoding: "utf8", timeout: 60000 }); } catch (e) { out = `${e.stdout ?? ""}${e.stderr ?? ""}`; }
      writeFileSync(file, out);
      return parseDoctor(out);
    };
    const before = doctor(root, `${shots}/doctor-nm-1-desktop-only.txt`);
    record("C-7", "openside doctor: user-profile manifest found for bitwarden, managed profile still reported missing", before.some((l) => l.startsWith("[ok] native_messaging.bitwarden") && l.includes(".config/chromium/NativeMessagingHosts/com.8bit.bitwarden.json")) && before.some((l) => l.startsWith("[warn] native_messaging.managed_profile")) ? "pass" : "fail", before.join(" || ").slice(0, 600));

    // A/B on the same browser binary and manifest: only XDG_CACHE_HOME differs.
    const ab = async (name, cacheHome) => {
      const profile = freshRoot(`nm-${label}-${name}`);
      mkdirSync(`${profile}/NativeMessagingHosts`, { recursive: true });
      copyFileSync(manifestSrc, `${profile}/NativeMessagingHosts/com.8bit.bitwarden.json`);
      const child = spawn(browser, ["--headless=new", "--remote-debugging-port=0", "--no-first-run", "--password-store=basic", `--user-data-dir=${profile}`, `--load-extension=${nmExt}`, `--disable-extensions-except=${nmExt}`, "about:blank"], { env: { PATH: process.env.PATH, HOME: NH, XDG_CONFIG_HOME: `${NH}/.config`, XDG_CACHE_HOME: cacheHome ?? `${NH}/.cache` }, stdio: "ignore", detached: true });
      try {
        const cdp = await Cdp.connect(await waitForPort(profile));
        const result = await probeNative(cdp);
        cdp.close();
        return result;
      } finally { killGroup(child.pid); }
    };
    const control = await ab("cache-inherited", null);
    record("C-7", `${label}: native host connects to the desktop app (profile manifest present, XDG_CACHE_HOME untouched)`, control.messages?.some((m) => m.includes("connected")) ? "pass" : "fail", JSON.stringify(control));
    const overridden = await ab("cache-profile", join(W, `nm-${label}-cache-profile/xdg-cache`));
    record("C-7", `${label}: cause isolation: same manifest but XDG_CACHE_HOME=<profile>/xdg-cache (what Browser::launch used to set) -> the Bitwarden proxy cannot find the desktop socket`, overridden.disconnected && !overridden.messages?.length ? "info" : "fail", JSON.stringify(overridden));

    // End to end through the real daemon launcher.
    root = freshRoot(`run-${label}-nm-daemon`);
    const managedNm = join(root, "data/openside/browser-profile/NativeMessagingHosts");
    // The user's own environment: HOME and XDG_CACHE_HOME point at the tree where the "desktop app" listens.
    const daemon = new Daemon({ root, home: NH, repo: REPO, executable: browser, extensions: [nmExt], env: { XDG_CACHE_HOME: `${NH}/.cache` } });
    let cdp;
    try {
      await daemon.start();
      const session = await daemon.newSession();
      await daemon.repl(session, `await page.goto(${JSON.stringify(MOCK)}); page.url()`);
      cdp = await Cdp.connect(daemon.debugEndpoint());
      const missing = await probeNative(cdp);
      record("C-7", `${label}: daemon-managed profile with only ~/.config/chromium manifest -> 'host not found' (Chromium reads <user-data-dir>/NativeMessagingHosts)`, String(missing.disconnected).includes("not found") ? "pass" : "fail", JSON.stringify(missing));
      const warn = doctor(root, `${shots}/doctor-nm-2-managed-missing.txt`);
      record("C-7", "openside doctor agrees: managed profile warn while the user-profile manifest exists", warn.some((l) => l.startsWith("[warn] native_messaging.managed_profile")) ? "pass" : "fail", warn.find((l) => l.includes("managed_profile")) ?? "");
      mkdirSync(managedNm, { recursive: true });
      copyFileSync(manifestSrc, join(managedNm, "com.8bit.bitwarden.json"));
      const afterCopy = await probeNative(cdp);
      record("C-7", `${label}: after copying the manifest into the managed profile, Chromium finds the host (no 'not found'); proxy starts`, !String(afterCopy.disconnected).includes("not found") ? "pass" : "fail", JSON.stringify(afterCopy));
      record("C-7", `${label}: daemon-managed browser reaches the desktop app through the native host`, afterCopy.messages?.some((m) => m.includes("connected")) ? "pass" : "bug", `Browser::launch must inherit the user's XDG_CACHE_HOME -> ${JSON.stringify(afterCopy)}`);
      const ok = doctor(root, `${shots}/doctor-nm-3-managed-present.txt`);
      record("C-7", "openside doctor: managed profile reports ok once the manifest is in <data>/browser-profile/NativeMessagingHosts", ok.some((l) => l.startsWith("[ok] native_messaging.managed_profile")) ? "pass" : "fail", ok.find((l) => l.includes("managed_profile")) ?? "");
    } finally {
      cdp?.close();
      await daemon.stop();
      await sleep(500);
      cleanupOwned(root);
    }
  } finally {
    desktop.stop();
    cleanupOwned(NH);
  }
}

// ---- 1Password: load only ------------------------------------------------------------------------

async function stageOnePassword() {
  assertFlagsMirrorSource();
  const root = freshRoot(`run-${label}-1p`);
  const live = await launchBrowser({ exe: browser, root, extensions: [OPENSIDE_EXT, `${W}/ext/1password`] });
  let cdp;
  try {
    cdp = await Cdp.connect(live.wsUrl);
    await sleep(6000);
    const targets = await listTargets(cdp);
    const worker = targets.find((t) => t.url.includes("aeblfdkhhhdcdjpifhhbdiojplfjncoa") && t.type === "service_worker");
    record("PP-19", `${label}: 1Password extension (store build 8.12.41.1, pinned id) loads next to openside's extension; service worker alive`, worker && targets.some((t) => t.type === "service_worker" && !t.url.includes("aeblfdkhhhdcdjpifhhbdiojplfjncoa")) ? "pass" : "fail", targets.filter((t) => t.type === "service_worker" || t.type === "page").map((t) => `${t.type}:${t.url.slice(0, 70)}`).join(" | "));
    const pages = targets.filter((t) => t.type === "page" && t.url.includes("aeblfdkhhhdcdjpifhhbdiojplfjncoa"));
    record("PP-19", `${label}: 1Password pages opened on install`, "info", pages.map((p) => p.url.slice(0, 100)).join(" | ") || "none; onboarding only starts from the popup/web");
    if (worker) {
      const nm = await cdp.evaluate(await cdp.attach(worker.targetId), `new Promise((resolve) => { let port; try { port = chrome.runtime.connectNative('com.1password.1password'); } catch (e) { resolve({ threw: String(e) }); return; }
        port.onDisconnect.addListener(() => resolve({ disconnected: (chrome.runtime.lastError && chrome.runtime.lastError.message) || 'no error' })); setTimeout(() => { try { port.disconnect(); } catch {} resolve({ stillConnected: true }); }, 3000); })`);
      record("PP-19", `${label}: 1Password native host without the desktop app (nativeMessaging granted by its manifest)`, "info", JSON.stringify(nm));
    }
    const manifest = JSON.parse(readFileSync(`${W}/ext/1password/manifest.json`, "utf8"));
    record("PP-19", `${label}: 1Password manifest permissions (what it can do inside the shared browser)`, "info", JSON.stringify({ version: manifest.version, permissions: manifest.permissions, optional: manifest.optional_permissions, hosts: manifest.host_permissions }));
    let popupText;
    try {
      const popup = await cdp.page(`chrome-extension://aeblfdkhhhdcdjpifhhbdiojplfjncoa/${manifest.action?.default_popup ?? "popup/popup.html"}`);
      await sleep(5000);
      popupText = (await cdp.evaluate(popup.sessionId, "document.body.innerText.slice(0,300)")).replace(/\s+/g, " ");
    } catch (error) {
      popupText = `unusable in --headless=new: ${error.message}`;
    }
    record("PP-19", `${label}: 1Password popup without an account or desktop app`, "info", JSON.stringify(popupText));
  } finally {
    cdp?.close();
    killGroup(live.pid);
    await sleep(500);
  }
}

const stages = { static: stageStatic, headless: stageHeadless, native: stageNative, onepassword: stageOnePassword };
if (!stages[stage]) {
  console.error(`unknown stage ${stage}; one of ${Object.keys(stages).join(", ")}`);
  process.exit(2);
}
try {
  await stages[stage]();
} catch (error) {
  record("ERR", `${label}: stage ${stage} aborted`, "fail", String(error.stack ?? error).slice(0, 500));
  process.exitCode = 1;
} finally {
  process.exit(process.exitCode ?? 0);
}
