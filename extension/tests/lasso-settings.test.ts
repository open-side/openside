import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_LASSO, type LassoSettings } from "../../web/src/lassoProtocol";
import { handleLassoRequest, isLassoRequest, shortcutLabel, shortcutPrompt, syncPageScript } from "../src/lasso-settings";

const DAEMON = "http://127.0.0.1:47820";
let local: Record<string, unknown>;
let granted: boolean;
let registered: string[];
const getAll = vi.fn(async () => [{ name: "start-lasso", shortcut: "Alt+Shift+L" }, { name: "open-palette", shortcut: "" }]);

beforeEach(() => {
  local = {};
  granted = false;
  registered = [];
  vi.stubGlobal("chrome", {
    runtime: { getManifest: () => ({ externally_connectable: { matches: [`${DAEMON}/*`] } }), getURL: (path: string) => `chrome-extension://ext/${path}` },
    storage: { local: {
      get: vi.fn(async (key: string) => key in local ? { [key]: local[key] } : {}),
      set: vi.fn(async (items: Record<string, unknown>) => { Object.assign(local, items); }),
    } },
    permissions: { contains: vi.fn(async () => granted) },
    scripting: {
      getRegisteredContentScripts: vi.fn(async ({ ids }: { ids: string[] }) => registered.filter(id => ids.includes(id)).map(id => ({ id }))),
      registerContentScripts: vi.fn(async (scripts: { id: string }[]) => { registered.push(...scripts.map(script => script.id)); }),
      unregisterContentScripts: vi.fn(async ({ ids }: { ids: string[] }) => { registered = registered.filter(id => !ids.includes(id)); }),
    },
    commands: { getAll },
    tabs: { create: vi.fn(async () => ({})) },
  });
});
afterEach(() => vi.unstubAllGlobals());

const off: LassoSettings = { text: { ...DEFAULT_LASSO.text, enabled: false }, region: { ...DEFAULT_LASSO.region, enabled: false } };

describe("syncPageScript", () => {
  it("registers the page script on every site only with access and a page feature on", async () => {
    await syncPageScript();
    expect(chrome.scripting.registerContentScripts).not.toHaveBeenCalled();
    expect(local.lassoAccess).toBe(false);
    granted = true;
    await syncPageScript();
    expect(chrome.scripting.registerContentScripts).toHaveBeenCalledWith([{ id: "lasso-page", matches: ["https://*/*", "http://*/*"], js: ["lasso.js"], runAt: "document_idle", persistAcrossSessions: true }]);
    expect(local.lassoAccess).toBe(true);
    await syncPageScript();
    expect(chrome.scripting.registerContentScripts).toHaveBeenCalledTimes(1);
    // Unregistering leaves scripts already in pages running; the flag is what turns them off.
    granted = false;
    await syncPageScript();
    expect(registered).toEqual([]);
    expect(local.lassoAccess).toBe(false);
  });

  it("removes the page script when both features are off", async () => {
    granted = true;
    await syncPageScript();
    local.lassoSettings = off;
    await syncPageScript();
    expect(registered).toEqual([]);
    local.lassoSettings = { ...off, region: { ...off.region, enabled: true } };
    await syncPageScript();
    expect(registered).toEqual(["lasso-page"]);
    // The flag follows the grant alone; the page script reads the toggles itself.
    expect(local.lassoAccess).toBe(true);
  });
});

describe("handleLassoRequest", () => {
  it("answers only the daemon UI the manifest names", async () => {
    for (const origin of [undefined, "https://example.com", "http://127.0.0.1:9999", "http://localhost:47820"]) {
      await expect(handleLassoRequest({ type: "openside.lasso.get" }, origin)).resolves.toMatchObject({ ok: false });
    }
    await expect(handleLassoRequest({ type: "openside.lasso.get" }, DAEMON)).resolves.toEqual({ ok: true, settings: DEFAULT_LASSO, access: false, command: "Alt+Shift+L" });
  });

  it("saves valid settings and follows them with the page script", async () => {
    granted = true;
    const changed: LassoSettings = { ...DEFAULT_LASSO, text: { enabled: true, shortcuts: [{ id: "custom-a1", label: "Explain", prompt: "Explain it simply." }] } };
    await expect(handleLassoRequest({ type: "openside.lasso.set", settings: changed }, DAEMON)).resolves.toEqual({ ok: true, settings: changed, access: true, command: "Alt+Shift+L" });
    expect(local.lassoSettings).toEqual(changed);
    expect(registered).toEqual(["lasso-page"]);
    await handleLassoRequest({ type: "openside.lasso.set", settings: off }, DAEMON);
    expect(registered).toEqual([]);
  });

  it("refuses malformed settings without saving", async () => {
    for (const settings of [null, { ...DEFAULT_LASSO, region: { ...DEFAULT_LASSO.region, hotkey: "meta" } }, { ...DEFAULT_LASSO, text: { enabled: true, shortcuts: [{ id: "custom-x" }] } }]) {
      await expect(handleLassoRequest({ type: "openside.lasso.set", settings }, DAEMON)).resolves.toMatchObject({ ok: false });
    }
    expect(chrome.storage.local.set).not.toHaveBeenCalled();
  });

  it("opens the all-sites access page on request", async () => {
    await handleLassoRequest({ type: "openside.lasso.access" }, DAEMON);
    expect(chrome.tabs.create).toHaveBeenCalledWith({ url: "chrome-extension://ext/lasso-access.html" });
  });

  it("reports no key when the command has none", async () => {
    getAll.mockResolvedValueOnce([{ name: "start-lasso", shortcut: "" }]);
    await expect(handleLassoRequest({ type: "openside.lasso.get" }, DAEMON)).resolves.toMatchObject({ command: "" });
  });
});

describe("shortcuts", () => {
  it("name built-ins from the catalog until edited", () => {
    expect(shortcutLabel({ id: "translate" })).toBe("Translate");
    expect(shortcutPrompt({ id: "translate" })).toBe("Translate this into English.");
    expect(shortcutLabel({ id: "translate", label: "Auf Deutsch" })).toBe("Auf Deutsch");
    expect(shortcutPrompt({ id: "translate", prompt: "Translate into German." })).toBe("Translate into German.");
    expect(shortcutLabel({ id: "custom-a", label: "Mine", prompt: "Do it." })).toBe("Mine");
  });

  it("recognize lasso requests by type prefix", () => {
    expect(isLassoRequest({ type: "openside.lasso.get" })).toBe(true);
    for (const value of [null, {}, { type: "openside.desktop.get" }, { type: 1 }]) expect(isLassoRequest(value)).toBe(false);
  });
});
