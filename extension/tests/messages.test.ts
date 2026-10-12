import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { messageSite, parseStart, sameDocument, validMenu } from "../src/messages";
const origin = "chrome-extension://extension-id/";
const url = "https://mail.google.com/mail/u/0/#inbox/message";
const context = { url, title: "Message", selection: "Selected message" };
const sender = { id: "extension-id", url, frameId: 0, tab: { id: 4, url } } as chrome.runtime.MessageSender;
beforeEach(() => vi.stubGlobal("chrome", { runtime: { id: "extension-id" } }));
afterEach(() => vi.unstubAllGlobals());
describe("extension message trust boundary", () => {
  it("accepts only own top-level content sender and a complete context", () => {
    expect(parseStart({ type: "start", text: "Handle", context }, sender, origin)?.tabId).toBe(4);
    for (const source of [{ ...sender, id: "other" }, { ...sender, frameId: 1 }, { ...sender, tab: { id: 1.2 } }, { ...sender, url: "https://attacker.test/" }]) expect(parseStart({ type: "start", text: "Handle", context }, source as chrome.runtime.MessageSender, origin)).toBeNull();
    for (const value of [null, "start", {}, { type: "start", text: 4 }, { type: "start", text: " " }, { type: "start", text: "Handle", context: { url } }]) expect(parseStart(value, sender, origin)).toBeNull();
  });
  it("keeps source-document validation strict while tolerating SPA hash changes", () => {
    expect(parseStart({ type: "start", text: "Handle", context: { ...context, url: url.replace("message", "another") } }, sender, origin)).not.toBeNull();
    expect(sameDocument(url, "https://mail.google.com/mail/u/1/#inbox/message")).toBe(false);
    expect(sameDocument(url, "https://mail.google.com/mail/u/0/?account=other")).toBe(false);
    expect(sameDocument(url, "not a URL")).toBe(false);
  });
  it("uses only an integer explicitly requested target from a trusted extension page", () => {
    const page = { id: "extension-id", url: `${origin}panel.html`, tab: { id: 99 } } as chrome.runtime.MessageSender;
    expect(parseStart({ type: "start", text: "Ask", tabId: 4 }, page, origin)?.tabId).toBe(4);
    for (const tabId of [-1, 1.5, "4", undefined]) expect(parseStart({ type: "start", text: "Ask", tabId }, page, origin)).toBeNull();
  });
  it("requires explicit test opt-in for loopback content", () => {
    const local = "http://127.0.0.1:43210/gmail.html";
    const localSender = { ...sender, url: local, tab: { id: 4, url: local } };
    const value = { type: "start", text: "Handle", context: { ...context, url: local } };
    expect(parseStart(value, localSender as chrome.runtime.MessageSender, origin)).toBeNull();
    expect(parseStart(value, localSender as chrome.runtime.MessageSender, origin, true)?.tabId).toBe(4);
  });
  it("recognizes production Gmail and Slack hosts without suffix spoofing", () => {
    expect(messageSite("mail.google.com")).toBe("gmail");
    expect(messageSite("app.slack.com")).toBe("slack");
    expect(messageSite("workspace.slack.com")).toBe("slack");
    for (const host of ["slack.com", "app.slack.com.attacker.test", "mail.google.com.attacker.test", "127.0.0.1"]) expect(messageSite(host)).toBeUndefined();
  });
  it("checks native selection menu source, host, frame and tab", () => {
    const tab = { id: 4, url, title: "Message" } as chrome.tabs.Tab;
    const info = { menuItemId: "handle-openside", pageUrl: url, selectionText: "Chosen text", frameId: 0 } as chrome.contextMenus.OnClickData;
    expect(validMenu(info, tab)).toBe(true);
    for (const value of [{ ...info, menuItemId: "other" }, { ...info, frameId: 2 }, { ...info, selectionText: " " }, { ...info, pageUrl: "https://attacker.test/" }]) expect(validMenu(value, tab)).toBe(false);
    expect(validMenu(info, { ...tab, id: 1.5 })).toBe(false);
    expect(validMenu(info, { ...tab, url: "https://mail.google.com/mail/u/1/" })).toBe(false);
    expect(validMenu({ ...info, pageUrl: url.replace("message", "other") }, tab)).toBe(true);
  });
});
