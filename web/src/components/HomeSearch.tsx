import { Search } from "lucide-react";
import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router";
import { useClient } from "../context";
import { useSessionList } from "../hooks/useSessionList";
import { Button, ICON_STROKE, Spinner } from "../ui";
import { displayTitle } from "./toolStatus";

const MAX_RESULTS = 8;

const sessionPath = (id: string) => `/sessions/${encodeURIComponent(id)}`;

/**
 * Home's Search mode: the daemon's conversation search, as a pill with results under it.
 * Enter opens the top match. Nothing is fetched until there is something to search for.
 */
export function HomeSearch() {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [first, setFirst] = useState<string | null>(null);
  const term = query.trim();

  return (
    <div className="home-search">
      <form
        role="search"
        className="home-omnibox"
        onSubmit={(event) => {
          event.preventDefault();
          if (term && first) navigate(sessionPath(first));
        }}
      >
        <Search className="home-omnibox-icon" size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <label className="home-omnibox-field">
          <span className="sr-only">Search conversations</span>
          <input
            type="search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search your conversations"
            enterKeyHint="search"
            autoComplete="off"
            // Picking Search is asking to type; the field is the only thing in this mode.
            autoFocus
          />
        </label>
      </form>
      {term ? <SearchResults query={term} onFirst={setFirst} /> : null}
    </div>
  );
}

function SearchResults({ query, onFirst }: { query: string; onFirst: (id: string | null) => void }) {
  const list = useSessionList(useClient());
  const { setQuery, sessions, answered, error, offline, refresh } = list;
  useEffect(() => setQuery(query), [query, setQuery]);
  // Until the debounced request for a new term lands, the previous term's matches stay up, but
  // Enter waits for this term's answer.
  const current = answered === query;
  const shown = sessions?.slice(0, MAX_RESULTS) ?? null;
  const top = current ? (shown?.[0]?.id ?? null) : null;
  useEffect(() => onFirst(top), [top, onFirst]);

  return (
    <section className="home-results" aria-label="Search results" aria-live="polite" aria-busy={!current && !error}>
      {error ? (
        <div role="alert" className="home-results-note error">
          <span>{offline ? "Can’t reach openside, so search is unavailable." : error}</span>
          <Button variant="secondary" size="compact" onClick={refresh}>
            Retry
          </Button>
        </div>
      ) : shown === null ? (
        <p className="home-results-note">
          <Spinner size={14} /> Searching…
        </p>
      ) : shown.length === 0 ? (
        current ? (
          <p className="home-results-note">No conversations match “{query}”.</p>
        ) : (
          <p className="home-results-note">
            <Spinner size={14} /> Searching…
          </p>
        )
      ) : (
        <ul className="home-results-list">
          {shown.map((session) => (
            <li key={session.id}>
              <Link to={sessionPath(session.id)} className="home-result" title={session.title}>
                <span className="home-result-title">{displayTitle(session.title)}</span>
                {session.status === "waiting_human" ? <span className="row-badge">Needs you</span> : null}
              </Link>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
