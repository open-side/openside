import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dropAsksForTab, handleLassoMessage } from "../src/asks";

const mocks = vi.hoisted(() => ({
  cropCapture: vi.fn(async () => ({ name: "capture.png", data_base64: "AQID" })),
  startChat: vi.fn(async () => "chat-1"),
  openConversation: vi.fn(async () => undefined),
}));
vi.mock("../src/capture", () => ({ cropCapture: mocks.cropCapture }));
vi.mock("../src/chat", async importOriginal => ({ ...await importOriginal<Record<string, unknown>>(), startChat: mocks.startChat }));
vi.mock("../src/desktop", () => ({ openConversation: mocks.openConversation }));

const EXTENSION = "chrome-extension://ext/";
let session: Record<string, unknown>;
let local: Record<string, unknown>;
let front: number;
let closed: Set<number>;
const tab = { id: 5, windowId: 9, active: true, url: "https://example.com/a", title: "Example" } as chrome.tabs.Tab;
const page = { id: "ext", frameId: 0, url: "https://example.com/a", tab } as chrome.runtime.MessageSender;
const popover = { id: "ext", frameId: 3, url: `${EXTENSION}ask.html`, tab } as chrome.runtime.MessageSender;
const panel = { id: "ext", url: `${EXTENSION}panel.html` } as chrome.runtime.MessageSender;
const rect = { x: 10, y: 20, width: 100, height: 50 };
const viewport = { width: 1000, height: 800 };

const pending = () => Object.entries(session).filter(([key]) => key.startsWith("ask:"));
const captureVisibleTab = vi.fn(async () => "data:image/png;base64,SHOT");
const sessionSet = vi.fn(async (items: Record<string, unknown>) => { Object.assign(session, items); });
const executeScript = vi.fn(async () => []);

beforeEach(() => {
  session = {};
  local = {};
  front = 5;
  closed = new Set();
  vi.clearAllMocks();
  vi.stubGlobal("chrome", {
    runtime: { id: "ext", getURL: (path: string) => `${EXTENSION}${path}` },
    storage: {
      session: {
        get: vi.fn(async (key: string | null) => key === null ? { ...session } : key in session ? { [key]: session[key] } : {}),
        set: sessionSet,
        remove: vi.fn(async (keys: string | string[]) => { for (const key of [keys].flat()) delete session[key]; }),
      },
      local: { get: vi.fn(async (key: string) => key in local ? { [key]: local[key] } : {}) },
    },
    tabs: { captureVisibleTab, sendMessage: vi.fn(async () => undefined), query: vi.fn(async () => [{ id: front }]), get: vi.fn(async (id: number) => { if (closed.has(id)) throw new Error(`No tab with id: ${id}.`); return { id }; }) },
    scripting: { executeScript },
  });
});
afterEach(() => vi.unstubAllGlobals());

async function capture(sender = page): Promise<string> {
  const reply = await handleLassoMessage({ type: "lasso.capture", rect, viewport }, sender) as { token: string };
  return reply.token;
}

describe("handleLassoMessage", () => {
  it("leaves other messages to other listeners", () => {
    for (const message of [null, "lasso.capture", { type: "start" }, { type: 4 }]) expect(handleLassoMessage(message, page)).toBeNull();
  });

  it("captures the sender's visible tab and keeps the crop out of the page", async () => {
    const token = await capture();
    expect(token).toMatch(/^[0-9a-f-]{36}$/);
    expect(chrome.tabs.captureVisibleTab).toHaveBeenCalledWith(9, { format: "png" });
    expect(mocks.cropCapture).toHaveBeenCalledWith("data:image/png;base64,SHOT", rect, viewport);
    expect(session[`ask:${token}`]).toMatchObject({ tabId: 5, kind: "region", context: { url: "https://example.com/a", title: "Example", selection: "" }, image: { name: "capture.png", data_base64: "AQID" } });
  });

  it("refuses a capture from a tab that is not in front", async () => {
    front = 6;
    // sender.tab says active, but it is a snapshot from before the person switched tabs.
    await expect(handleLassoMessage({ type: "lasso.capture", rect, viewport }, page)).rejects.toThrow("not_visible");
    expect(chrome.tabs.query).toHaveBeenCalledWith({ active: true, windowId: 9 });
    expect(chrome.tabs.captureVisibleTab).not.toHaveBeenCalled();
  });

  it("drops a capture when the tab lost the front while it was taken", async () => {
    captureVisibleTab.mockImplementationOnce(async () => { front = 6; return "data:image/png;base64,OTHER"; });
    await expect(handleLassoMessage({ type: "lasso.capture", rect, viewport }, page)).rejects.toThrow("not_visible");
    expect(mocks.cropCapture).not.toHaveBeenCalled();
    expect(pending()).toEqual([]);
  });

  it("refuses an ask that would not fit in session storage, after dropping expired ones", async () => {
    const big = "A".repeat(4 * 1024 * 1024);
    mocks.cropCapture.mockResolvedValueOnce({ name: "capture.png", data_base64: big }).mockResolvedValueOnce({ name: "capture.png", data_base64: big });
    session["panelAsk:7"] = { text: "", context: { url: "", title: "", selection: "" }, attachments: [{ name: "capture.png", data_base64: big }] };
    await expect(capture()).rejects.toThrow("storage_full");
    expect(pending()).toEqual([]);
    delete session["panelAsk:7"];
    session["ask:old"] = { tabId: 8, kind: "region", context: {}, image: { name: "capture.png", data_base64: big }, created: Date.now() - 11 * 60_000 };
    const token = await capture();
    expect(pending().map(([key]) => key)).toEqual([`ask:${token}`]);
    expect(session[`ask:${token}`]).toMatchObject({ tabId: 5, image: { data_base64: big } });
  });

  it("reports a session storage quota error as no room", async () => {
    sessionSet.mockRejectedValueOnce(new Error("Session storage quota bytes exceeded. Values were not stored."));
    await expect(capture()).rejects.toThrow("storage_full");
  });

  it("leaves nothing behind when the tab closes while its capture is cropped", async () => {
    mocks.cropCapture.mockImplementationOnce(async () => {
      closed.add(5);
      void dropAsksForTab(5);
      return { name: "capture.png", data_base64: "AQID" };
    });
    await expect(capture()).rejects.toThrow("tab_closed");
    expect(pending()).toEqual([]);
  });

  it("drops a closed tab's ask and keeps its hand-off from landing afterwards", async () => {
    const token = await capture();
    const handoff = handleLassoMessage({ type: "lasso.ask", token, shortcut: "copy-code", panel: true }, popover);
    closed.add(5);
    await dropAsksForTab(5);
    await expect(handoff).rejects.toThrow(/tab_closed|Ask expired/);
    expect(session["panelAsk:5"]).toBeUndefined();
    expect(pending()).toEqual([]);
  });

  it("drops an unread hand-off with the tab", async () => {
    const token = await capture();
    await handleLassoMessage({ type: "lasso.ask", token, shortcut: "copy-code", panel: true }, popover);
    expect(session["panelAsk:5"]).toBeDefined();
    await dropAsksForTab(5);
    expect(session["panelAsk:5"]).toBeUndefined();
  });

  it("hands an ask to the panel within the budget, swapping it for the pending ask", async () => {
    const big = "A".repeat(1.5 * 1024 * 1024);
    mocks.cropCapture.mockResolvedValueOnce({ name: "capture.png", data_base64: big }).mockResolvedValueOnce({ name: "capture.png", data_base64: big });
    // The pending ask's own bytes do not count against its hand-off: the swap fits.
    session["panelAsk:7"] = { text: "", context: { url: "", title: "", selection: "" }, attachments: [{ name: "capture.png", data_base64: big }] };
    const token = await capture();
    await handleLassoMessage({ type: "lasso.ask", token, shortcut: "copy-code", panel: true }, popover);
    expect(session["panelAsk:5"]).toMatchObject({ attachments: [{ data_base64: big }] });
    expect(pending()).toEqual([]);
    // Other tabs' unread hand-offs do count: with three waiting, one more hand-off is refused and its ask kept for a retry.
    front = 9;
    const second = await capture({ ...page, tab: { ...tab, id: 9 } });
    session["panelAsk:8"] = session["panelAsk:7"];
    await expect(handleLassoMessage({ type: "lasso.ask", token: second, shortcut: "copy-code", panel: true }, popover)).rejects.toThrow("storage_full");
    expect(session["panelAsk:9"]).toBeUndefined();
    expect(session[`ask:${second}`]).toBeDefined();
  });

  it("takes page messages only from the extension's own top-frame script with a sane selection", () => {
    for (const sender of [{ ...page, id: "other" }, { ...page, frameId: 2 }, { ...page, tab: undefined }, { ...page, tab: { ...tab, id: 1.5 } }]) {
      expect(handleLassoMessage({ type: "lasso.capture", rect, viewport }, sender as chrome.runtime.MessageSender)).toBeNull();
    }
    for (const value of [{ ...rect, width: 2 }, { ...rect, x: Number.NaN }, null]) expect(handleLassoMessage({ type: "lasso.capture", rect: value, viewport }, page)).toBeNull();
    expect(handleLassoMessage({ type: "lasso.capture", rect, viewport: { width: 0, height: 800 } }, page)).toBeNull();
    // The popover is an extension page; it cannot pose as the page script.
    expect(handleLassoMessage({ type: "lasso.capture", rect, viewport }, popover)).toBeNull();
  });

  it("stores a text selection from the page, capped", async () => {
    const { token } = await handleLassoMessage({ type: "lasso.selection", selection: "x".repeat(30_000) }, page) as { token: string };
    expect(session[`ask:${token}`]).toMatchObject({ tabId: 5, kind: "text", context: { url: "https://example.com/a", title: "Example", selection: "x".repeat(20_000) } });
    expect(handleLassoMessage({ type: "lasso.selection", selection: "  " }, page)).toBeNull();
  });

  it("keeps one pending ask per tab when two arrive at once", async () => {
    const replies = await Promise.all([
      handleLassoMessage({ type: "lasso.selection", selection: "First" }, page),
      handleLassoMessage({ type: "lasso.selection", selection: "Second" }, page),
      capture(),
    ]) as [{ token: string }, { token: string }, string];
    const pending = Object.keys(session).filter(key => key.startsWith("ask:"));
    expect(pending).toEqual([`ask:${replies[2]}`]);
  });

  it("keeps one pending ask per tab and drops expired ones", async () => {
    session["ask:00000000-0000-0000-0000-000000000000"] = { tabId: 77, kind: "text", context: { url: "", title: "", selection: "old" }, created: Date.now() - 11 * 60_000 };
    const first = await capture();
    const second = await capture();
    expect(pending().map(([key]) => key)).toEqual([`ask:${second}`]);
    expect(first).not.toBe(second);
  });

  it("gives the popover the shortcuts for the kind of ask, and nothing to the page", async () => {
    const token = await capture();
    expect(handleLassoMessage({ type: "lasso.details", token }, page)).toBeNull();
    await expect(handleLassoMessage({ type: "lasso.details", token }, popover)).resolves.toEqual({ tabId: 5, kind: "region", shortcuts: [{ id: "copy-code", label: "Copy code" }, { id: "search-image", label: "Search image" }] });
    local.lassoSettings = { text: { enabled: true, shortcuts: [{ id: "custom-a", label: "Explain", prompt: "Explain like I'm new." }] }, region: { enabled: true, hotkey: "alt", shortcuts: [] } };
    const { token: text } = await handleLassoMessage({ type: "lasso.selection", selection: "Words" }, page) as { token: string };
    await expect(handleLassoMessage({ type: "lasso.details", token: text }, popover)).resolves.toMatchObject({ kind: "text", shortcuts: [{ id: "custom-a", label: "Explain" }] });
  });

  it("hands the ask to the side panel when the popover opened it", async () => {
    const token = await capture();
    await handleLassoMessage({ type: "lasso.ask", token, shortcut: "copy-code", panel: true }, popover);
    expect(session["panelAsk:5"]).toEqual({
      text: "Write out the code in this image as a code block, exactly as shown.",
      context: { url: "https://example.com/a", title: "Example", selection: "" },
      attachments: [{ name: "capture.png", data_base64: "AQID" }],
    });
    expect(pending()).toEqual([]);
    expect(mocks.startChat).not.toHaveBeenCalled();
    expect(chrome.tabs.sendMessage).toHaveBeenCalledWith(5, { type: "lasso.close" }, { frameId: 0 });
  });

  it("starts a chat in the web app when the panel did not open", async () => {
    const { token } = await handleLassoMessage({ type: "lasso.selection", selection: "Bonjour" }, page) as { token: string };
    await handleLassoMessage({ type: "lasso.ask", token, prompt: "  What language is this?  ", panel: false }, popover);
    expect(mocks.startChat).toHaveBeenCalledWith(5, "What language is this?", { url: "https://example.com/a", title: "Example", selection: "Bonjour" }, undefined);
    expect(mocks.openConversation).toHaveBeenCalledWith("chat-1");
    expect(session["panelAsk:5"]).toBeUndefined();
    expect(pending()).toEqual([]);
  });

  it("keeps the ask when sending fails, so the popover can retry", async () => {
    const token = await capture();
    mocks.startChat.mockRejectedValueOnce(new Error("offline"));
    await expect(handleLassoMessage({ type: "lasso.ask", token, shortcut: "search-image", panel: false }, popover)).rejects.toThrow("offline");
    expect(pending()).toHaveLength(1);
    await handleLassoMessage({ type: "lasso.ask", token, shortcut: "search-image", panel: false }, popover);
    expect(mocks.startChat).toHaveBeenCalledTimes(2);
    expect(pending()).toEqual([]);
  });

  it("rejects malformed, unknown and expired asks", async () => {
    const token = await capture();
    for (const message of [{ type: "lasso.ask", token, panel: true }, { type: "lasso.ask", token, prompt: "x".repeat(1001), panel: true }, { type: "lasso.ask", token: "nope", prompt: "Hi", panel: true }, { type: "lasso.ask", token, prompt: "Hi" }]) {
      expect(handleLassoMessage(message, popover)).toBeNull();
    }
    expect(handleLassoMessage({ type: "lasso.ask", token, prompt: "Hi", panel: true }, page)).toBeNull();
    await expect(handleLassoMessage({ type: "lasso.ask", token, shortcut: "summarize", panel: true }, popover)).rejects.toThrow("Nothing to ask");
    await expect(handleLassoMessage({ type: "lasso.ask", token: "11111111-1111-1111-1111-111111111111", prompt: "Hi", panel: true }, popover)).rejects.toThrow("Ask expired");
  });

  it("treats an ask past its ten minutes as gone, for details and for sending", async () => {
    const token = await capture();
    (session[`ask:${token}`] as { created: number }).created = Date.now() - 10 * 60_000;
    await expect(handleLassoMessage({ type: "lasso.details", token }, popover)).rejects.toThrow("Ask expired");
    expect(pending()).toEqual([]);
    const second = await capture();
    (session[`ask:${second}`] as { created: number }).created = Date.now() - 10 * 60_000 - 1;
    await expect(handleLassoMessage({ type: "lasso.ask", token: second, prompt: "Hi", panel: false }, popover)).rejects.toThrow("Ask expired");
    expect(mocks.startChat).not.toHaveBeenCalled();
    expect(pending()).toEqual([]);
  });

  it("closes the page overlay when the popover cancels", async () => {
    const token = await capture();
    await handleLassoMessage({ type: "lasso.cancel", token }, popover);
    expect(pending()).toEqual([]);
    expect(chrome.tabs.sendMessage).toHaveBeenCalledWith(5, { type: "lasso.close" }, { frameId: 0 });
  });

  it("lets the page cancel only its own tab's ask", async () => {
    const token = await capture();
    await handleLassoMessage({ type: "lasso.cancel", token }, { ...page, tab: { ...tab, id: 6 } });
    expect(pending()).toHaveLength(1);
    await handleLassoMessage({ type: "lasso.cancel", token }, page);
    expect(pending()).toEqual([]);
    expect(chrome.tabs.sendMessage).not.toHaveBeenCalled();
  });

  it("starts a lasso in a tab for the side panel", async () => {
    expect(handleLassoMessage({ type: "lasso.start", tabId: 5 }, page)).toBeNull();
    expect(handleLassoMessage({ type: "lasso.start", tabId: -1 }, panel)).toBeNull();
    await handleLassoMessage({ type: "lasso.start", tabId: 5 }, panel);
    expect(chrome.scripting.executeScript).toHaveBeenCalledWith({ target: { tabId: 5 }, files: ["lasso.js"] });
    expect(chrome.tabs.sendMessage).toHaveBeenCalledWith(5, { type: "lasso.start" }, { frameId: 0 });
  });

  it("says the panel needs a gesture or a site grant when Chrome refuses the page", async () => {
    executeScript.mockRejectedValueOnce(new Error("Cannot access contents of the page. Extension manifest must request permission to access the respective host."));
    await expect(handleLassoMessage({ type: "lasso.start", tabId: 5 }, panel)).rejects.toThrow("no_access");
    executeScript.mockRejectedValueOnce(new Error("Frame with ID 0 was removed."));
    await expect(handleLassoMessage({ type: "lasso.start", tabId: 5 }, panel)).rejects.toThrow("Frame with ID 0 was removed.");
    expect(chrome.tabs.sendMessage).not.toHaveBeenCalled();
  });
});
