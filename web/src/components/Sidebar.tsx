import { PanelLeft, X } from "lucide-react";
import type { KeyboardEvent } from "react";
import type { Session } from "../api/types";
import type { OwnedTab } from "../hooks/useAgentTabs";
import type { AgentsState } from "../hooks/useAgents";
import { t } from "../i18n";
import { useShell, useSidebarPrefs } from "../shell";
import { ICON_STROKE, IconButton } from "../ui";
import { SidebarAccount } from "./SidebarAccount";
import { SidebarBookmarks } from "./SidebarBookmarks";
import { SidebarChats } from "./SidebarChats";
import { SidebarTabs } from "./SidebarTabs";

export interface SidebarProps {
  team: AgentsState;
  /** The agents' open tabs, polled once for the shell. */
  tabs: readonly OwnedTab[];
  /** The shell's chat list; `null` while loading. */
  sessions: readonly Session[] | null;
  /** Names openside's own pages in the Tabs list. */
  pageLabel: (pathname: string) => string | null;
  /** Present when rendered inside the phone drawer. */
  onClose?: () => void;
}

/** Rows the arrow keys walk, in reading order; a row's own trailing buttons stay on Tab. */
const ROWS = ".sidebar-row:not(:disabled)";

/** ArrowUp/ArrowDown/Home/End move between rows, as in a list; Tab still visits every control. */
function moveFocus(event: KeyboardEvent<HTMLElement>) {
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key) || event.altKey || event.metaKey || event.ctrlKey) return;
  if (!(event.target instanceof HTMLElement) || !event.target.matches(ROWS)) return;
  const rows = [...event.currentTarget.querySelectorAll<HTMLElement>(ROWS)];
  const at = rows.indexOf(event.target);
  const next = event.key === "Home" ? 0 : event.key === "End" ? rows.length - 1 : at + (event.key === "ArrowDown" ? 1 : -1);
  const row = rows[Math.max(0, Math.min(rows.length - 1, next))];
  if (!row) return;
  event.preventDefault();
  row.focus();
}

/** The window's left column (`L 10`, `L 20`): Bookmarks, Chats and Tabs over the account bar. */
export function Sidebar({ team, tabs, sessions, pageLabel, onClose }: SidebarProps) {
  const shell = useShell();
  const [prefs] = useSidebarPrefs();
  // Delete is offered only once the roster says which chats are agents' homes.
  const homes = team.agents === null ? null : new Set(team.agents.flatMap((agent) => (agent.home_session_id ? [agent.home_session_id] : [])));

  return (
    <div className="sidebar">
      <div className="sidebar-head">
        {onClose ? (
          <IconButton label={t("shell.sidebar.closeMenu")} size="small" onClick={onClose}>
            <X size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        ) : (
          <IconButton label={t("shell.sidebar.collapse")} title={t("shell.sidebar.collapse")} size="small" className="sidebar-toggle" onClick={shell.toggleSidebar}>
            <PanelLeft size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        )}
      </div>
      <nav className="sidebar-scroll thin-scroll" aria-label={t("shell.sidebar.sections")} onKeyDown={moveFocus}>
        {prefs.bookmarks ? <SidebarBookmarks onPicked={onClose} /> : null}
        {prefs.chats ? <SidebarChats sessions={sessions} count={prefs.recentChats} homes={homes} /> : null}
        <SidebarTabs agentTabs={tabs} pageLabel={pageLabel} onPicked={onClose} />
      </nav>
      <SidebarAccount onClose={onClose} />
    </div>
  );
}
