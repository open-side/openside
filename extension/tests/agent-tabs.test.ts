import { afterEach, describe, it, expect, vi } from "vitest";
import { ownedChromeTabs, clearAgentTabs, groupAgentTabs } from "../src/agent-tabs";
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
describe("Agent Tabs group", () => {
  const ledger = { tabs: [{ tab_id: "owned", url: "https://a.example.test", title: "A", opened_by: "agent" as const, owner_session_id: "session" }] };
  function stub(stored: Record<string, number>, alive: boolean) {
    const group = vi.fn(async () => 31);
    const update = vi.fn(async () => undefined);
    vi.stubGlobal("chrome", {
      debugger: { getTargets: vi.fn(async () => [{ id: "owned", tabId: 7, type: "page", title: "A", url: "https://a.example.test", attached: true }]) },
      tabs: { query: vi.fn(async ({ groupId }: { groupId?: number }) => groupId === undefined ? [{ id: 7, windowId: 1, incognito: false }] : [{ id: 7 }]), group, ungroup: vi.fn(async () => undefined) },
      tabGroups: { get: vi.fn(async () => alive ? { id: 31 } : Promise.reject(new Error("gone"))), update },
      storage: { session: { get: vi.fn(async () => ({ agentGroups: stored })), set: vi.fn(async () => undefined) } },
    });
    return { group, update };
  }
  it("names and colours a new group once", async () => {
    const { group, update } = stub({}, false);
    await groupAgentTabs(ledger);
    expect(group).toHaveBeenCalledWith({ tabIds: [7], createProperties: { windowId: 1 } });
    expect(update).toHaveBeenCalledExactlyOnceWith(31, { title: "Agent Tabs", color: "grey" });
  });
  it("leaves a rename, recolour or collapse alone when the group already exists", async () => {
    const { group, update } = stub({ 1: 31 }, true);
    await groupAgentTabs(ledger);
    expect(group).toHaveBeenCalledWith({ tabIds: [7], groupId: 31 });
    expect(update).not.toHaveBeenCalled();
  });
});
