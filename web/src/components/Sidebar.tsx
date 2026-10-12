import { Archive, Ellipsis, LayoutGrid, Search, Settings2, SquarePen, X } from "lucide-react";
import { Link, useLocation, useMatch } from "react-router";
import type { Session } from "../api/types";
import type { SessionListState } from "../hooks/useSessionList";
import { manageLinkState } from "../pages/manageRoutes";
import { BrandMark, cx, ICON_STROKE, IconButton, Spinner } from "../ui";
import { SessionMenu } from "./SessionMenu";

const icon = { size: 18, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

export interface SidebarProps {
  list: SessionListState;
  /** Present when rendered inside the phone drawer. */
  onClose?: () => void;
}

export function Sidebar({ list, onClose }: SidebarProps) {
  const { sessions, query, archived } = list;
  const pinned = sessions?.filter((session) => session.pinned) ?? [];
  const rest = sessions?.filter((session) => !session.pinned) ?? [];
  // Dialog routes render the shell with their background location, so this is the screen
  // underneath and links opened from here never stack dialogs.
  const location = useLocation();

  return (
    <div className="sidebar">
      <div className="sidebar-top">
        <Link to="/" className="brand">
          <BrandMark size={24} />
          <span>openside</span>
        </Link>
        {onClose && (
          <IconButton label="Close conversations" size="small" onClick={onClose}>
            <X {...icon} />
          </IconButton>
        )}
      </div>

      <div className="sidebar-actions">
        <Link to="/" className="sidebar-nav-item">
          <SquarePen {...icon} />
          <span>New chat</span>
        </Link>
        <div className="sidebar-search">
          <label className="sidebar-search-field">
            <Search {...icon} />
            <span className="sr-only">Search conversations</span>
            <input
              type="search"
              value={query}
              onChange={(event) => list.setQuery(event.target.value)}
              placeholder={archived ? "Search archived" : "Search"}
            />
          </label>
          <IconButton
            label="Show archived"
            size="small"
            className="archive-toggle"
            aria-pressed={archived}
            onClick={() => list.setArchived(!archived)}
          >
            <Archive {...icon} />
          </IconButton>
        </div>
      </div>

      <div className="session-list">
        {/* An unreachable daemon is announced once, by the shell's connection banner. */}
        {list.error && !list.offline && (
          <p role="alert" className="sidebar-note error">
            {list.error}
          </p>
        )}
        {sessions === null ? (
          list.offline ? null : <p className="sidebar-note">Loading conversations…</p>
        ) : sessions.length === 0 ? (
          <p className="sidebar-note">
            {query ? `No conversations match “${query}”.` : archived ? "Nothing archived yet." : "Your conversations will appear here."}
          </p>
        ) : (
          <>
            {pinned.length > 0 && <SessionGroup heading="Pinned" label="Pinned conversations" sessions={pinned} list={list} />}
            {rest.length > 0 && (
              <SessionGroup
                heading={archived ? "Archived" : query ? "Results" : "Conversations"}
                label="Conversations"
                sessions={rest}
                list={list}
              />
            )}
          </>
        )}
      </div>

      <nav className="manage-nav" aria-label="Sections">
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

function SessionGroup({ heading, label, sessions, list }: { heading: string; label: string; sessions: Session[]; list: SessionListState }) {
  return (
    <section className="session-group">
      <h2 className="session-group-heading">{heading}</h2>
      <ul className="sessions" aria-label={label}>
        {sessions.map((session) => (
          <SessionRow key={session.id} session={session} list={list} />
        ))}
      </ul>
    </section>
  );
}

function SessionRow({ session, list }: { session: Session; list: SessionListState }) {
  const href = `/sessions/${encodeURIComponent(session.id)}`;
  const current = useMatch("/sessions/:id")?.params.id === session.id;
  return (
    <li className={cx("session-row", current && "current")} data-session={session.id}>
      <Link to={href} className="session-link" aria-current={current ? "page" : undefined}>
        <span className="session-title" title={session.title}>
          {session.title}
        </span>
        {session.status === "running" && <Spinner label="Running" size={14} />}
        {session.status === "waiting_human" && <span className="row-badge">Needs you</span>}
      </Link>
      <SessionMenu
        session={session}
        label={`Options for ${session.title}`}
        onChanged={list.refresh}
        trigger={
          <IconButton label={`Options for ${session.title}`} size="small" className="row-options">
            <Ellipsis {...icon} />
          </IconButton>
        }
      />
    </li>
  );
}
