import type { TabLedger } from "./api";
export function ownedChromeTabs(ledger: TabLedger, targets: chrome.debugger.TargetInfo[]): number[] {
  const owned = new Set(ledger.tabs.filter(tab => tab.opened_by === "agent" && tab.owner_session_id).map(tab => tab.tab_id));
  return targets.filter(target => owned.has(target.id) && target.tabId !== undefined).map(target => target.tabId!);
}
// Grey is the quietest chip colour in Chromium's fixed palette, so it does not fight the tab strip's sky-gradient theme. The patched browser's own gateway creates a group with this title; keep the two in step.
const GROUP_STYLE = { title: "Agent Tabs", color: "grey" } as const;
export async function groupAgentTabs(tabs: TabLedger): Promise<void> {
  const owned = new Set(ownedChromeTabs(tabs, await chrome.debugger.getTargets()));
  const windows = new Map<number, number[]>();
  for (const tab of await chrome.tabs.query({})) {
    if (tab.id === undefined || tab.incognito || !owned.has(tab.id)) continue;
    const ids = windows.get(tab.windowId) ?? [];
    ids.push(tab.id);
    windows.set(tab.windowId, ids);
  }
  const stored = await chrome.storage.session.get("agentGroups");
  const groups = (stored.agentGroups ?? {}) as Record<string, number>;
  for (const [window, group] of Object.entries(groups)) {
    const members = await chrome.tabs.query({ groupId: group }).catch(() => []);
    const released = members.filter(tab => tab.id !== undefined && !owned.has(tab.id)).map(tab => tab.id!);
    if (released.length) await chrome.tabs.ungroup(released);
    if (!windows.has(Number(window))) delete groups[window];
  }
  for (const [windowId, tabIds] of windows) {
    const existing = groups[windowId];
    const alive = existing !== undefined && await chrome.tabGroups.get(existing).then(() => true, () => false);
    const groupId = await chrome.tabs.group(alive ? { tabIds, groupId: existing } : { tabIds, createProperties: { windowId } });
    // Only a new group is styled, so a rename, recolour or collapse the person made sticks across the periodic syncs.
    if (!alive) await chrome.tabGroups.update(groupId, GROUP_STYLE);
    groups[windowId] = groupId;
  }
  await chrome.storage.session.set({ agentGroups: groups });
}
export async function clearAgentTabs(): Promise<void> {
  const groups = (await chrome.storage.session.get("agentGroups")).agentGroups as Record<string, number> | undefined;
  for (const groupId of Object.values(groups ?? {})) {
    const members = await chrome.tabs.query({ groupId }).catch(() => []);
    const ids = members.flatMap(tab => tab.id === undefined ? [] : [tab.id]);
    if (ids.length) await chrome.tabs.ungroup(ids);
  }
  await chrome.storage.session.remove("agentGroups");
}
