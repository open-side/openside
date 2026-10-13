import { Ellipsis, Search } from "lucide-react";
import { Link } from "react-router";
import type { Session } from "../api/types";
import { AppHeader } from "../components/AppHeader";
import { Avatar } from "../components/Avatar";
import { SessionMenu } from "../components/SessionMenu";
import { displayTitle } from "../components/toolStatus";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { useSessionList, type SessionListState } from "../hooks/useSessionList";
import { ICON_STROKE, IconButton, Segmented, Spinner } from "../ui";
import "../components/team.css";

/**
 * `/history`: conversations from before the team, archived ones, and retired teammates'
 * threads. Everything stays openable and, unless retired, continuable.
 */
export function HistoryPage() {
  const list = useSessionList(useClient(), "none");
  const team = useTeam();
  const { sessions, query, archived } = list;
  const retired = (team.agents ?? []).filter((agent) => agent.status === "retired" && (!query.trim() || agent.name.toLowerCase().includes(query.trim().toLowerCase())));

  return (
    <main className="history-page">
      <AppHeader title="History" />
      <div className="history-scroll thin-scroll">
        <div className="history-column">
          <div className="history-tools">
            <label className="history-search">
              <Search size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
              <span className="sr-only">Search conversations</span>
              <input type="search" value={query} onChange={(event) => list.setQuery(event.target.value)} placeholder={archived ? "Search archived" : "Search conversations"} />
            </label>
            <Segmented
              kind="tab"
              label="Which conversations"
              options={[
                { value: "active", label: "Earlier" },
                { value: "archived", label: "Archived" },
              ]}
              value={archived ? "archived" : "active"}
              onChange={(value) => list.setArchived(value === "archived")}
            />
          </div>

          {list.error && !list.offline ? (
            <p role="alert" className="sidebar-note error">
              {list.error}
            </p>
          ) : null}

          <section className="history-group" aria-labelledby="history-earlier">
            <h2 id="history-earlier" className="session-group-heading">
              {archived ? "Archived" : query ? "Results" : "Earlier conversations"}
            </h2>
            {sessions === null ? (
              list.offline ? null : (
                <p className="sidebar-note">
                  <Spinner size={14} /> Loading conversations…
                </p>
              )
            ) : sessions.length === 0 ? (
              <p className="sidebar-note">{query ? `No conversations match “${query}”.` : archived ? "Nothing archived yet." : "Conversations from before your team appear here."}</p>
            ) : (
              <ul className="sessions history-list" aria-label={archived ? "Archived conversations" : "Earlier conversations"}>
                {sessions.map((session) => (
                  <HistoryRow key={session.id} session={session} list={list} />
                ))}
              </ul>
            )}
          </section>

          {retired.length > 0 && !archived ? (
            <section className="history-group" aria-labelledby="history-retired">
              <h2 id="history-retired" className="session-group-heading">
                Retired teammates
              </h2>
              <ul className="people" aria-label="Retired teammates">
                {retired.map((agent) => (
                  <li key={agent.id} className="person-row">
                    <Link to={`/sessions/${encodeURIComponent(agent.home_session_id)}`} className="person-link">
                      <Avatar initials={agent.initials} color={agent.color} size={40} />
                      <span className="person-text">
                        <span className="person-name">{agent.name}</span>
                        <span className="person-second">{agent.persona.role}</span>
                      </span>
                    </Link>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
        </div>
      </div>
    </main>
  );
}

function HistoryRow({ session, list }: { session: Session; list: SessionListState }) {
  const title = displayTitle(session.title);
  return (
    <li className="session-row" data-session={session.id}>
      <Link to={`/sessions/${encodeURIComponent(session.id)}`} className="session-link">
        <span className="session-text">
          <span className="session-title" title={session.title}>
            {title}
          </span>
        </span>
        {session.status === "waiting_human" ? <span className="row-badge">Needs you</span> : null}
      </Link>
      <SessionMenu
        session={session}
        label={`Options for ${session.title}`}
        onChanged={list.refresh}
        trigger={
          <IconButton label={`Options for ${session.title}`} size="small" className="row-options">
            <Ellipsis size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        }
      />
    </li>
  );
}
