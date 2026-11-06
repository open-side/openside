import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { actOnPage, answer, connectPersonBridge, outlinePage, resetPersonBridge, resetSiteAccessPrompts } from "../src/person-bridge";

const tab = (id: number, incognito: boolean, url = `https://site${id}.example/`) => ({
  id, windowId: incognito ? 2 : 1, index: id, title: `Tab ${id}`, url, active: id === 1, pinned: false,
  discarded: false, incognito, groupId: -1, mutedInfo: { muted: false }, audible: false,
});

function browser(tabs: ReturnType<typeof tab>[]) {
  const updated: unknown[] = [];
  const created: { url: string; index: number }[] = [];
  vi.stubGlobal("chrome", {
    runtime: { getURL: (path: string) => `chrome-extension://openside/${path}` },
    tabs: {
      query: async () => tabs,
      get: async (id: number) => { const found = tabs.find(item => item.id === id); if (!found) throw new Error("No tab"); return found; },
      update: async (id: number, change: object) => { updated.push([id, change]); return { ...tabs.find(item => item.id === id)!, ...change }; },
      create: async (properties: { url: string; index: number }) => { created.push(properties); return {}; },
    },
    windows: { getLastFocused: async () => ({ id: 1, incognito: false }), update: async () => ({}) },
    tabGroups: { query: async () => [{ id: 5, windowId: 1, title: "Work", color: "blue", collapsed: false }, { id: 6, windowId: 2, title: "Hidden", color: "red", collapsed: false }] },
    storage: { session: { get: async () => ({ pairing: { url: "http://127.0.0.1:4100", token: "t" } }) } },
    scripting: { executeScript: async () => { throw new Error("Cannot access contents of the page"); } },
    // site1 was allowed from site-access.html; the wildcards came from another feature's opt-in.
    permissions: { getAll: async () => ({ origins: ["<all_urls>", "https://*/*", "https://*.site2.example/*", "https://site1.example/*"] }) },
  });
  return { updated, created };
}

const call = async (method: string, params: object = {}) => JSON.parse((await answer(JSON.stringify({ id: "1", method, params })))!);

describe("person bridge", () => {
  afterEach(() => { vi.unstubAllGlobals(); resetSiteAccessPrompts(); });

  it("never reports incognito tabs or their groups", async () => {
    browser([tab(1, false), tab(2, true)]);
    const { result } = await call("tabs.list");
    expect(result.tabs.map((item: { tab_id: number }) => item.tab_id)).toEqual([1]);
    expect(result.groups.map((group: { group_id: number }) => group.group_id)).toEqual([5]);
    expect(result.focused_window_id).toBe(1);
  });

  it("refuses incognito tabs, unknown methods and non-web URLs", async () => {
    const { updated } = browser([tab(1, false), tab(2, true)]);
    expect((await call("tabs.command", { command: "set_pinned", tab_id: 2, pinned: true })).error.code).toBe("tab_not_found");
    expect((await call("tabs.command", { command: "create", url: "file:///etc/passwd" })).error.code).toBe("invalid_request");
    expect((await call("cookies.getAll")).error.code).toBe("invalid_request");
    expect(updated).toEqual([]);
    expect(await answer(JSON.stringify({ kind: "ping" }))).toBeNull();
  });

  it("clears only the kinds of browsing data the person picked", async () => {
    browser([tab(1, false)]);
    const removed: unknown[] = [];
    Object.assign(chrome, { browsingData: { remove: async (options: object, set: object) => { removed.push([options, set]); } } });
    expect((await call("browsing_data.clear", { types: ["history", "form_data"], since_ms: 1000 })).result).toEqual({});
    expect(removed).toEqual([[{ since: 1000 }, { history: true, formData: true }]]);
    for (const params of [{ types: ["toString"], since_ms: 0 }, { types: [], since_ms: 0 }, { types: ["cache"], since_ms: -1 }, { types: ["cache"] }]) {
      expect((await call("browsing_data.clear", params)).error.code).toBe("invalid_request");
    }
    expect(removed).toHaveLength(1);
  });

  it("guards page reads: same site only, never openside itself, asks for site access", async () => {
    const { created } = browser([tab(1, false), tab(3, false, "http://127.0.0.1:4100/chat"), tab(4, false, "http://localhost:4100/chat"), tab(5, false, "http://localhost.:4100/"), tab(6, false, "http://[::ffff:127.0.0.1]:4100/")]);
    expect((await call("tab.snapshot", { tab_id: 1, origin: "https://other.example" })).error.code).toBe("origin_changed");
    expect((await call("tab.snapshot", { tab_id: 3, origin: "http://127.0.0.1:4100" })).error.code).toBe("unsupported_page");
    expect((await call("tab.snapshot", { tab_id: 4, origin: "http://localhost:4100" })).error.code).toBe("unsupported_page");
    // Checked before the origin, so the claimed origin does not matter.
    for (const id of [5, 6]) expect((await call("tab.snapshot", { tab_id: id, origin: "http://localhost:4100" })).error.code).toBe("unsupported_page");
    expect((await call("tab.snapshot", { tab_id: 1, origin: "https://site1.example" })).error.code).toBe("site_access_required");
    // Site access needs a click, so a page asking for this one site opens; a retry does not open another.
    expect((await call("tab.act", { tab_id: 1, origin: "https://site1.example", action: "click", ref: "u1" })).error.code).toBe("site_access_required");
    expect(created.map(item => item.url)).toEqual(["chrome-extension://openside/site-access.html#https%3A%2F%2Fsite1.example"]);
    // Without a pairing nothing is read: there is no daemon UI to exclude.
    Object.assign(chrome.storage.session, { get: async () => ({}) });
    expect((await call("tab.snapshot", { tab_id: 1, origin: "https://site1.example" })).error.code).toBe("unsupported_page");
  });

  it("opens at most three site access pages a minute across all sites", async () => {
    const ids = [11, 12, 13, 14, 15];
    const { created } = browser(ids.map(id => tab(id, false)));
    for (const id of ids) {
      expect((await call("tab.snapshot", { tab_id: id, origin: `https://site${id}.example` })).error.code).toBe("site_access_required");
    }
    expect(created).toHaveLength(3);
  });

  it("never navigates a person tab onto openside itself", async () => {
    const { updated } = browser([tab(1, false)]);
    const navigate = (url: string) => call("tab.act", { tab_id: 1, origin: "https://site1.example", action: "navigate", url });
    for (const url of ["http://127.0.0.1:4100/settings", "http://localhost.:4100/", "http://[::ffff:127.0.0.1]:4100/", "http://127.1:4100/"]) {
      expect((await navigate(url)).error.code, url).toBe("unsupported_page");
    }
    expect(updated).toEqual([]);
    expect((await navigate("http://127.0.0.1:4101/")).result).toEqual({ url: "http://127.0.0.1:4101/" });
    expect(updated).toEqual([[1, { url: "http://127.0.0.1:4101/" }]]);
  });

  it("refuses unparsable URLs and pages that moved during a read", async () => {
    browser([tab(1, false)]);
    for (const url of ["http://[bad", "not a url", "javascript:alert(1)", `https://site1.example/${"a".repeat(9000)}`]) {
      expect((await call("tabs.command", { command: "create", url })).error.code).toBe("invalid_request");
      expect((await call("tab.act", { tab_id: 1, origin: "https://site1.example", action: "navigate", url })).error.code).toBe("invalid_request");
    }
    // The tab navigated between the check and the injection: the other site's outline is dropped.
    Object.assign(chrome.scripting, { executeScript: async () => [{ result: { url: "https://elsewhere.example/", title: "", text: "secret", refs: [], truncated: false } }] });
    const moved = await call("tab.snapshot", { tab_id: 1, origin: "https://site1.example" });
    expect(moved.error.code).toBe("origin_changed");
    expect(JSON.stringify(moved)).not.toContain("secret");
  });

  it("reads only sites the person allowed one by one, not through a broader grant", async () => {
    const { created } = browser([tab(1, false), tab(2, false)]);
    let injected = 0;
    Object.assign(chrome.scripting, { executeScript: async () => { injected += 1; return [{ result: { url: "https://site1.example/", title: "", text: "", refs: [], truncated: false } }]; } });
    expect((await call("tab.snapshot", { tab_id: 2, origin: "https://site2.example" })).error.code).toBe("site_access_required");
    expect(injected).toBe(0);
    expect(created.map(item => item.url)).toEqual(["chrome-extension://openside/site-access.html#https%3A%2F%2Fsite2.example"]);
    expect((await call("tab.snapshot", { tab_id: 1, origin: "https://site1.example" })).result).toBeDefined();
    expect(injected).toBe(1);
  });

  it("connects with the new pairing when it changes during a connection attempt", async () => {
    const opened: string[] = [];
    vi.stubGlobal("WebSocket", class {
      static OPEN = 1;
      readyState = 0;
      constructor(_url: string, protocols: string[]) { opened.push(protocols[1]); }
      send() {}
      close() { this.readyState = 3; }
    });
    const gate = Promise.withResolvers<void>();
    let current = { url: "http://127.0.0.1:4100", token: "old" };
    let first = true;
    vi.stubGlobal("chrome", { storage: { session: {
      get: async (key: string) => {
        if (key !== "pairing") return { personBridgeRun: "run" };
        const seen = current;
        if (first) { first = false; await gate.promise; }
        return { pairing: seen };
      },
      set: async () => undefined,
    } } });
    const attempt = connectPersonBridge();
    current = { url: "http://127.0.0.1:4100", token: "new" };
    resetPersonBridge();
    gate.resolve();
    await attempt;
    await vi.waitFor(() => expect(opened).toEqual(["openside.bearer.new"]));
  });
});

describe("page outline", () => {
  beforeAll(() => {
    // jsdom has no layout: every element counts as rendered, and contenteditable follows the attribute.
    Object.defineProperty(HTMLElement.prototype, "offsetParent", { configurable: true, get() { return document.body; } });
    Object.defineProperty(HTMLElement.prototype, "isContentEditable", { configurable: true, get(this: HTMLElement) { return this.getAttribute("contenteditable") === "true"; } });
    Element.prototype.scrollIntoView = () => undefined;
  });

  it("names fields by their label only and refuses a relabelled ref", () => {
    document.body.innerHTML = `<label for="n">Note</label><textarea id="n">secret draft</textarea>
      <div contenteditable="true" aria-label="Message">typed secret</div>
      <input type="password" value="hunter2" placeholder="Password"><button>Send</button>`;
    const outline = outlinePage(10_000);
    expect(outline.text).not.toMatch(/secret|hunter2/);
    expect(outline.refs.map(item => item.name)).toEqual(["Note", "Message", "Password", "Send"]);
    const send = outline.refs.find(item => item.name === "Send")!;
    expect(actOnPage({ action: "click", ref: send.ref, expect_name: "Send" })).toEqual({});
    // The injection ran after the tab moved to another site.
    expect(actOnPage({ action: "click", origin: "https://other.example", ref: send.ref, expect_name: "Send" })).toEqual({ error: "origin_changed" });
    document.querySelector("button")!.textContent = "Delete account";
    expect(actOnPage({ action: "click", origin: location.origin, ref: send.ref, expect_name: "Send" })).toEqual({ error: "stale_ref" });
  });
});
