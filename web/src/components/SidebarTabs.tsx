import { ChevronRight, Globe } from "lucide-react";
import { useEffect, useState } from "react";
import { Link } from "react-router";
import type { AgentSummary, Session } from "../api/types";
import { originOf, useFavicon } from "../hooks/useFavicon";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { ICON_STROKE } from "../ui";

/** Group keys the reader folded away; shared by the desktop rail and the phone drawer. */
export const COLLAPSED_TAB_GROUPS_KEY = "openside.sidebar.collapsedTabGroups";

/** A person's thread when the tab is theirs, otherwise the conversation that opened it; both open the browser sheet. */
export function tabTarget(tab: OwnedTab, agents: readonly AgentSummary[]): string {
  const agent = agents.find((candidate) => candidate.home_session_id === tab.owner_session_id);
  return agent ? `/agents/${encodeURIComponent(agent.id)}?browser=open` : `/sessions/${encodeURIComponent(tab.owner_session_id)}?browser=open`;
}

interface TabGroup {
  key: string;
  label: string;
  tabs: OwnedTab[];
}

/**
 * One group per teammate that owns tabs, in first-seen order: a tab belongs to a teammate when
 * its owner session is their home conversation or a listed session carrying their `agent_id`.
 * Sessions the shell's list has not loaded (older pages, a filtered list) cannot be attributed
 * and share one trailing group with every unowned conversation. A lone group is always
 * "Agent tabs" so the rail does not repeat a name the reader already sees in Chats.
 */
function groupTabs(tabs: readonly OwnedTab[], agents: readonly AgentSummary[], sessions: readonly Session[]): TabGroup[] {
  const byAgent = new Map<string, TabGroup>();
  const rest: OwnedTab[] = [];
  for (const tab of tabs) {
    const sessionAgent = sessions.find((session) => session.id === tab.owner_session_id)?.agent_id;
    const agent = agents.find((candidate) => candidate.home_session_id === tab.owner_session_id || (sessionAgent != null && candidate.id === sessionAgent));
    if (!agent) {
      rest.push(tab);
      continue;
    }
    const group = byAgent.get(agent.id) ?? { key: agent.id, label: agent.name, tabs: [] };
    group.tabs.push(tab);
    byAgent.set(agent.id, group);
  }
  const groups = [...byAgent.values()];
  if (rest.length > 0) groups.push({ key: "conversations", label: "Other conversations", tabs: rest });
  return groups.length === 1 ? groups.map((group) => ({ ...group, label: "Agent tabs" })) : groups;
}

function loadCollapsed(): Set<string> {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(COLLAPSED_TAB_GROUPS_KEY) ?? "[]");
    return new Set(Array.isArray(stored) ? stored.filter((key): key is string => typeof key === "string") : []);
  } catch {
    return new Set();
  }
}

function saveCollapsed(keys: ReadonlySet<string>) {
  if (keys.size === 0) localStorage.removeItem(COLLAPSED_TAB_GROUPS_KEY);
  else localStorage.setItem(COLLAPSED_TAB_GROUPS_KEY, JSON.stringify([...keys]));
}

/** The agents' open tabs (polled once for the shell by `useAgentTabs`); hidden when there are none. */
export function SidebarTabs({ tabs, agents, sessions = [] }: { tabs: readonly OwnedTab[]; agents: readonly AgentSummary[]; sessions?: readonly Session[] }) {
  const [collapsed, setCollapsed] = useState(loadCollapsed);
  const groups = groupTabs(tabs, agents, sessions);
  const live = groups.map((group) => group.key).join("\n");

  // Forget folds for groups that are gone, but only while some tabs are listed: an empty poll
  // (a failure, the browser restarting) must not unfold everything.
  useEffect(() => {
    if (live === "") return;
    const keys = new Set(live.split("\n"));
    setCollapsed((current) => {
      const kept = new Set([...current].filter((key) => keys.has(key)));
      if (kept.size === current.size) return current;
      saveCollapsed(kept);
      return kept;
    });
  }, [live]);

  if (tabs.length === 0) return null;
  const toggle = (key: string) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(key)) next.add(key);
      saveCollapsed(next);
      return next;
    });
  return (
    <section className="sidebar-section sidebar-tabs" aria-labelledby="sidebar-tabs-heading">
      <h2 id="sidebar-tabs-heading" className="section-heading">
        Tabs
      </h2>
      {groups.map((group) => (
        <TabGroupRows key={group.key} group={group} agents={agents} open={!collapsed.has(group.key)} onToggle={() => toggle(group.key)} />
      ))}
    </section>
  );
}

function TabGroupRows({ group, agents, open, onToggle }: { group: TabGroup; agents: readonly AgentSummary[]; open: boolean; onToggle: () => void }) {
  return (
    <div className="tab-group">
      <button type="button" className="sidebar-row tab-group-toggle" aria-expanded={open} aria-label={`${group.label}, ${group.tabs.length}`} onClick={onToggle}>
        <ChevronRight size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span className="tab-group-label">{group.label}</span>
        <span className="tab-group-count" aria-hidden="true">
          {group.tabs.length}
        </span>
      </button>
      {open ? (
        <ul className="sidebar-rows tab-group-rows" aria-label={group.label}>
          {group.tabs.map((tab) => (
            <li key={tab.tab_id}>
              <TabRow tab={tab} to={tabTarget(tab, agents)} />
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function TabRow({ tab, to }: { tab: OwnedTab; to: string }) {
  const origin = originOf(tab.url);
  const icon = useFavicon(origin);
  const host = origin ? new URL(origin).host : "";
  const title = tab.title.trim() || host || tab.url;
  return (
    <Link to={to} className="sidebar-row tab-row" title={title} aria-description={tab.url}>
      {icon ? <img className="tab-favicon" src={icon} alt="" width={16} height={16} /> : <Globe className="tab-favicon" size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />}
      <span className="tab-title">{title}</span>
      {host && host !== title ? <span className="tab-host">{host}</span> : null}
    </Link>
  );
}
