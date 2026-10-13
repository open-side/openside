import { ChevronRight, History, LayoutGrid, Settings2, X } from "lucide-react";
import { useState } from "react";
import { Link, useLocation, useMatch } from "react-router";
import type { AgentSummary } from "../api/types";
import { isUnread, people, type AgentsState } from "../hooks/useAgents";
import { manageLinkState } from "../pages/manageRoutes";
import { useSeen } from "../state/seen";
import { BrandMark, cx, ICON_STROKE, IconButton } from "../ui";
import { Avatar } from "./Avatar";
import "./team.css";

const icon = { size: 18, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

const TIME = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const DAY = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });

/** Messenger-style stamp: the time today, the weekday within a week, the date before that. */
export function rowTime(value: string | null | undefined, now = new Date()): string {
  if (!value) return "";
  const at = new Date(value);
  if (Number.isNaN(at.getTime())) return "";
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (at.getTime() >= startOfToday) return TIME.format(at);
  if (at.getTime() >= startOfToday - 6 * 86_400_000) return WEEKDAY.format(at);
  return DAY.format(at);
}

export interface SidebarProps {
  team: AgentsState;
  /** Present when rendered inside the phone drawer. */
  onClose?: () => void;
}

export function Sidebar({ team, onClose }: SidebarProps) {
  // Dialog routes render the shell with their background location, so this is the screen
  // underneath and links opened from here never stack dialogs.
  const location = useLocation();
  const openAgentId = useMatch("/agents/:id")?.params.id ?? null;
  const cursors = useSeen();
  const [showHidden, setShowHidden] = useState(false);
  const { chief, teammates, hidden } = people(team.agents ?? []);
  const row = (agent: AgentSummary) => <PersonRow key={agent.id} agent={agent} current={agent.id === openAgentId} unread={isUnread(agent, cursors, openAgentId)} />;

  return (
    <div className="sidebar">
      <div className="sidebar-head">
        <div className="sidebar-top">
          <Link to="/" className="brand">
            <BrandMark size={24} />
            <span>openside</span>
          </Link>
          {onClose && (
            <IconButton label="Close menu" size="small" onClick={onClose}>
              <X {...icon} />
            </IconButton>
          )}
        </div>
      </div>

      <div className="session-list people-list thin-scroll">
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
            {teammates.length === 0 && chief ? (
              <p className="sidebar-note people-hint">When work keeps coming back, {chief.name} can bring in a teammate to own it.</p>
            ) : null}
            {hidden.length > 0 ? (
              <section className="people-hidden">
                <button type="button" className="people-hidden-toggle" aria-expanded={showHidden} onClick={() => setShowHidden((open) => !open)}>
                  <ChevronRight size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  Hidden · {hidden.length}
                </button>
                {showHidden ? (
                  <ul className="people" aria-label="Hidden teammates">
                    {hidden.map(row)}
                  </ul>
                ) : null}
              </section>
            ) : null}
          </>
        )}
      </div>

      <nav className="manage-nav" aria-label="Sections">
        <Link to="/history" className="sidebar-nav-item">
          <History {...icon} />
          <span>History</span>
        </Link>
        <Link to="/workspace" state={manageLinkState(location)} className="sidebar-nav-item">
          <LayoutGrid {...icon} />
          <span>Workspace</span>
        </Link>
        <Link to="/settings" state={manageLinkState(location)} className="sidebar-nav-item">
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
  const second = activity || agent.last_activity.trim() || agent.persona.role;
  const time = rowTime(agent.last_message_at);
  const state = agent.needs_you ? "Needs you" : working ? "Working" : unread ? "Unread" : null;
  return (
    <li className={cx("person-row", current && "current", unread && "unread")} data-agent={agent.id}>
      <Link
        to={`/agents/${encodeURIComponent(agent.id)}`}
        className="person-link"
        aria-current={current ? "page" : undefined}
        aria-label={[agent.name, state].filter(Boolean).join(", ")}
      >
        <Avatar chief={agent.kind === "chief"} initials={agent.initials} color={agent.color} size={40} working={working} />
        <span className="person-text">
          <span className="person-name" title={agent.name}>
            {agent.name}
          </span>
          <span className={cx("person-second", activity && "live")} title={second}>
            {second}
          </span>
        </span>
        <span className="person-meta">
          {time ? <span className="person-time">{time}</span> : null}
          {agent.needs_you ? (
            <span className="row-badge">Needs you</span>
          ) : unread ? (
            <span className="unread-dot" title="New messages" />
          ) : null}
        </span>
      </Link>
    </li>
  );
}
