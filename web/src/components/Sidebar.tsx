import { ChevronRight, History, LayoutGrid, PanelLeftClose, Settings2, SquarePen, X } from "lucide-react";
import { useState } from "react";
import { Link, useLocation, useMatch } from "react-router";
import type { AgentSummary } from "../api/types";
import { isUnread, people, type AgentsState } from "../hooks/useAgents";
import { manageLinkState } from "../pages/manageRoutes";
import { useShell } from "../shell";
import { useSeen } from "../state/seen";
import { BrandMark, cx, ICON_STROKE, IconButton } from "../ui";
import { Avatar } from "./Avatar";
import { SidebarPinned } from "./SidebarPinned";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { SidebarTabs } from "./SidebarTabs";
import "./team.css";

const icon = { size: 18, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

export interface SidebarProps {
  team: AgentsState;
  /** The agents' open tabs, polled once for the shell. */
  tabs: readonly OwnedTab[];
  /** Present when rendered inside the phone drawer. */
  onClose?: () => void;
}

export function Sidebar({ team, tabs, onClose }: SidebarProps) {
  // Dialog routes render the shell with their background location, so this is the screen
  // underneath and links opened from here never stack dialogs.
  const location = useLocation();
  const shell = useShell();
  const openAgentId = useMatch("/agents/:id")?.params.id ?? null;
  const onHome = useMatch("/") !== null;
  const cursors = useSeen();
  const [showHidden, setShowHidden] = useState(false);
  const { chief, teammates, hidden } = people(team.agents ?? []);
  const row = (agent: AgentSummary) => <PersonRow key={agent.id} agent={agent} current={agent.id === openAgentId} unread={isUnread(agent, cursors, openAgentId)} />;

  return (
    <div className="sidebar">
      <div className="sidebar-head">
        <Link to="/" className="brand">
          <BrandMark size={20} />
          <span>openside</span>
        </Link>
        {onClose ? (
          <IconButton label="Close menu" size="small" onClick={onClose}>
            <X {...icon} />
          </IconButton>
        ) : (
          <IconButton label="Collapse sidebar" size="small" onClick={shell.toggleSidebar}>
            <PanelLeftClose {...icon} />
          </IconButton>
        )}
      </div>

      <Link to="/" className="sidebar-row sidebar-new-task" aria-current={onHome ? "page" : undefined}>
        <SquarePen {...icon} />
        <span>New task</span>
      </Link>

      <div className="session-list people-list thin-scroll">
        <SidebarPinned />
        <section className="sidebar-section" aria-labelledby="sidebar-chats-heading">
          <h2 id="sidebar-chats-heading" className="section-heading">
            Chats
          </h2>
          {/* An unreachable daemon is announced once, by the shell's connection banner. */}
          {team.error && !team.offline && (
            <p role="alert" className="sidebar-note error">
              {team.error}
            </p>
          )}
          {team.agents === null ? (
            team.offline ? null : <p className="sidebar-note">Loading your team…</p>
          ) : (
            <>
              <ul className="people" aria-label="Team">
                {chief ? row(chief) : null}
                {teammates.map(row)}
              </ul>
              {hidden.length > 0 ? (
                <div className="people-hidden">
                  <button type="button" className="sidebar-row people-hidden-toggle" aria-expanded={showHidden} onClick={() => setShowHidden((open) => !open)}>
                    <ChevronRight size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                    Hidden · {hidden.length}
                  </button>
                  {showHidden ? (
                    <ul className="people" aria-label="Hidden teammates">
                      {hidden.map(row)}
                    </ul>
                  ) : null}
                </div>
              ) : null}
              <Link to="/history" className="sidebar-row sidebar-show-all">
                Show all
              </Link>
            </>
          )}
        </section>
        <SidebarTabs tabs={tabs} agents={team.agents ?? []} />
      </div>

      <nav className="manage-nav" aria-label="Sections">
        <Link to="/history" className="sidebar-row sidebar-nav-item">
          <History {...icon} />
          <span>History</span>
        </Link>
        <Link to="/workspace" state={manageLinkState(location)} className="sidebar-row sidebar-nav-item">
          <LayoutGrid {...icon} />
          <span>Workspace</span>
        </Link>
        <Link to="/settings" state={manageLinkState(location)} className="sidebar-row sidebar-nav-item">
          <Settings2 {...icon} />
          <span>Settings</span>
        </Link>
      </nav>
    </div>
  );
}

/** Phrases the daemon leaves in `activity` once a run is over. */
const FINISHED_ACTIVITY: ReadonlySet<string> = new Set(["Done", "Stopped", "Couldn’t finish", "Couldn't finish"]);

function PersonRow({ agent, current, unread }: { agent: AgentSummary; current: boolean; unread: boolean }) {
  const working = agent.session_status === "running";
  const activity = working && !FINISHED_ACTIVITY.has(agent.activity) ? agent.activity.trim() : "";
  // One line per person: what they are doing (or said last) moves into the tooltip.
  const detail = activity || agent.last_activity.trim() || agent.persona.role;
  const state = agent.needs_you ? "Needs you" : working ? "Working" : unread ? "Unread" : null;
  return (
    <li className={cx("person-row", current && "current", unread && "unread")} data-agent={agent.id}>
      <Link
        to={`/agents/${encodeURIComponent(agent.id)}`}
        className="sidebar-row person-link"
        aria-current={current ? "page" : undefined}
        aria-label={[agent.name, state].filter(Boolean).join(", ")}
        title={detail ? `${agent.name} · ${detail}` : agent.name}
      >
        <Avatar chief={agent.kind === "chief"} initials={agent.initials} color={agent.color} size={24} />
        <span className="person-name">{agent.name}</span>
        {agent.needs_you ? (
          <span className="row-badge">Needs you</span>
        ) : working ? (
          <span className="live-dot" />
        ) : unread ? (
          <span className="unread-dot" />
        ) : null}
      </Link>
    </li>
  );
}
