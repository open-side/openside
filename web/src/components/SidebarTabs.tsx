import { Globe } from "lucide-react";
import { Link } from "react-router";
import type { AgentSummary } from "../api/types";
import { originOf, useFavicon } from "../hooks/useFavicon";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { ICON_STROKE } from "../ui";

/** A person's thread when the tab is theirs, otherwise the conversation that opened it; both open the browser sheet. */
export function tabTarget(tab: OwnedTab, agents: readonly AgentSummary[]): string {
  const agent = agents.find((candidate) => candidate.home_session_id === tab.owner_session_id);
  return agent ? `/agents/${encodeURIComponent(agent.id)}?browser=open` : `/sessions/${encodeURIComponent(tab.owner_session_id)}?browser=open`;
}

/** The agents' open tabs (polled once for the shell by `useAgentTabs`); hidden when there are none. */
export function SidebarTabs({ tabs, agents }: { tabs: readonly OwnedTab[]; agents: readonly AgentSummary[] }) {
  if (tabs.length === 0) return null;
  return (
    <section className="sidebar-section sidebar-tabs" aria-labelledby="sidebar-tabs-heading">
      <h2 id="sidebar-tabs-heading" className="section-heading">
        Tabs
      </h2>
      <ul className="sidebar-rows">
        {tabs.map((tab) => (
          <li key={tab.tab_id}>
            <TabRow tab={tab} to={tabTarget(tab, agents)} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function TabRow({ tab, to }: { tab: OwnedTab; to: string }) {
  const origin = originOf(tab.url);
  const icon = useFavicon(origin);
  const host = origin ? new URL(origin).host : "";
  const title = tab.title.trim() || host || tab.url;
  return (
    <Link to={to} className="sidebar-row tab-row" title={tab.url}>
      {icon ? <img className="tab-favicon" src={icon} alt="" width={16} height={16} /> : <Globe className="tab-favicon" size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />}
      <span className="tab-title">{title}</span>
      {host && host !== title ? <span className="tab-host">{host}</span> : null}
    </Link>
  );
}
