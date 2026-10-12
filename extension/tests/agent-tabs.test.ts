import { afterEach, describe, it, expect, vi } from "vitest";
import { ownedChromeTabs, clearAgentTabs } from "../src/agent-tabs";
afterEach(() => vi.unstubAllGlobals());
describe("Agent Tabs identity", () => {
  it("groups only exact owned CDP targets, never user tabs with an identical URL", () => {
    const tabs = { tabs: [
      { tab_id: "owned", url: "https://same.example.test", title: "Owned", opened_by: "agent" as const, owner_session_id: "session" },
      { tab_id: "user", url: "https://same.example.test", title: "User", opened_by: "user" as const, owner_session_id: "session" },
    ] };
    const targets: chrome.debugger.TargetInfo[] = [ { id: "owned", tabId: 7, type: "page", title: "Owned", url: "https://same.example.test", attached: true }, { id: "user", tabId: 8, type: "page", title: "User", url: "https://same.example.test", attached: false }, { id: "other", tabId: 9, type: "page", title: "Other", url: "https://same.example.test", attached: false } ];
    expect(ownedChromeTabs(tabs, targets)).toEqual([7]);
  });
  it("disabling grouping releases only groups tracked by the extension without debugger access", async () => {
    const query = vi.fn(async ({ groupId }: { groupId: number }) => groupId === 11 ? [{ id: 7 }, { id: 8 }] : []);
    const ungroup = vi.fn(async () => undefined);
    const remove = vi.fn(async () => undefined);
    vi.stubGlobal("chrome", { tabs: { query, ungroup }, storage: { session: { get: vi.fn(async () => ({ agentGroups: { 1: 11, 2: 12 } })), remove } } });
    await clearAgentTabs();
    expect(ungroup).toHaveBeenCalledExactlyOnceWith([7, 8]);
    expect(query.mock.calls.map(([value]) => value.groupId)).toEqual([11, 12]);
    expect(remove).toHaveBeenCalledWith("agentGroups");
  });
});
