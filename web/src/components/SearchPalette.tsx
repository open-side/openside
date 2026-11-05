import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { Globe, MessageSquare, Search } from "lucide-react";
import { useEffect, useId, useMemo, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useNavigate } from "react-router";
import type { AgentSummary, Session } from "../api/types";
import { useClient } from "../context";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { recency, useTeam } from "../hooks/useAgents";
import { originOf, useFavicon } from "../hooks/useFavicon";
import { keyLabel, searchKeys } from "../shortcuts";
import { cx, ICON_STROKE } from "../ui";
import { Avatar } from "./Avatar";
import { tabTarget } from "./SidebarTabs";
import { displayTitle } from "./toolStatus";
import "../shortcuts.css";
import "./searchPalette.css";

const SECTION_LIMIT = 6;
const SEARCH_DEBOUNCE_MS = 150;

const DATE = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const DATE_YEAR = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });
// Local calendar date as a UTC day number: whole days with no DST hour to round away.
const dayNumber = (at: Date) => Date.UTC(at.getFullYear(), at.getMonth(), at.getDate()) / 86_400_000;

/**
 * "Just now", "5 min ago", "3 h ago" within a day, then calendar days: "Yesterday", "4 days ago",
 * "Oct 3". A time ahead of this device's clock (skew) reads "Just now"; a missing or unreadable
 * time is `null`.
 */
export function relativeTime(at: string | null | undefined, now = new Date()): string | null {
  if (!at) return null;
  const date = new Date(at);
  const ms = now.getTime() - date.getTime();
  if (Number.isNaN(ms)) return null;
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return "Just now";
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)} h ago`;
  const days = dayNumber(now) - dayNumber(date);
  if (days <= 1) return "Yesterday";
  if (days < 7) return `${days} days ago`;
  return (date.getFullYear() === now.getFullYear() ? DATE : DATE_YEAR).format(date);
}

type Item =
  | { kind: "tab"; key: string; to: string; title: string; detail: string; tab: OwnedTab }
  | { kind: "chat"; key: string; to: string; title: string; detail: string; agent: AgentSummary; time: string | null }
  | { kind: "conversation"; key: string; to: string; title: string; detail: string };

function matches(query: string, ...fields: string[]): boolean {
  const haystack = fields.join(" ").toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

function tabItems(tabs: readonly OwnedTab[], agents: readonly AgentSummary[], query: string): Item[] {
  return tabs
    .filter((tab) => matches(query, tab.title, tab.url))
    .slice(0, SECTION_LIMIT)
    .map((tab) => {
      const origin = originOf(tab.url);
      const host = origin ? new URL(origin).host : "";
      return { kind: "tab", key: `tab:${tab.tab_id}`, to: tabTarget(tab, agents), title: tab.title.trim() || host || tab.url, detail: host, tab };
    });
}

function chatItems(agents: readonly AgentSummary[], query: string, now: Date): Item[] {
  return agents
    .filter((agent) => matches(query, agent.name, agent.persona.role))
    .sort((a, b) => recency(b) - recency(a) || a.name.localeCompare(b.name))
    .slice(0, SECTION_LIMIT)
    .map((agent) => ({
      kind: "chat",
      key: `chat:${agent.id}`,
      // Retired teammates keep only their thread, the way History opens it.
      to: agent.status === "retired" ? `/sessions/${encodeURIComponent(agent.home_session_id)}` : `/agents/${encodeURIComponent(agent.id)}`,
      title: agent.name,
      detail: agent.status === "retired" ? "Retired" : agent.persona.role,
      agent,
      time: relativeTime(agent.last_message_at, now),
    }));
}

function conversationItems(sessions: readonly Session[]): Item[] {
  return sessions.slice(0, SECTION_LIMIT).map((session) => ({
    kind: "conversation",
    key: `conversation:${session.id}`,
    to: `/sessions/${encodeURIComponent(session.id)}`,
    title: displayTitle(session.title),
    detail: session.status === "waiting_human" ? "Needs you" : "",
  }));
}

/**
 * The daemon's own conversation search (the History list: conversations outside the team),
 * debounced while typing; `null` until the first answer for this query arrives. A request the
 * person has typed past, or that is still out when the palette closes, is aborted.
 */
function useConversationSearch(query: string): Session[] | null {
  const client = useClient();
  const [result, setResult] = useState<{ query: string; sessions: Session[] } | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    const timer = window.setTimeout(
      () => {
        client.listSessions({ q: query, agent: "none" }, controller.signal).then(
          (list) => {
            if (!controller.signal.aborted) setResult({ query, sessions: list.sessions });
          },
          () => {
            // An unreachable daemon is announced by the shell's banner; the section just stays empty.
            if (!controller.signal.aborted) setResult({ query, sessions: [] });
          },
        );
      },
      query ? SEARCH_DEBOUNCE_MS : 0,
    );
    return () => {
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [client, query]);
  return result && result.query === query ? result.sessions : null;
}

const SECTIONS = [
  { kind: "tab", heading: "Open tabs" },
  { kind: "chat", heading: "Chats" },
  { kind: "conversation", heading: "Conversations" },
] as const;

function Results({ tabs, onDone }: { tabs: readonly OwnedTab[]; onDone: () => void }) {
  const navigate = useNavigate();
  const team = useTeam();
  const agents = useMemo(() => team.agents ?? [], [team.agents]);
  const [query, setQuery] = useState("");
  const trimmed = query.trim();
  const sessions = useConversationSearch(trimmed);
  const items = useMemo(() => {
    const now = new Date();
    return [...tabItems(tabs, agents, trimmed), ...chatItems(agents, trimmed, now), ...conversationItems(sessions ?? [])];
  }, [tabs, agents, trimmed, sessions]);
  const [active, setActive] = useState(0);
  const listId = useId();
  const current = Math.min(active, items.length - 1);
  const optionId = (index: number) => `${listId}-option-${index}`;

  useEffect(() => {
    if (current >= 0) document.getElementById(`${listId}-option-${current}`)?.scrollIntoView?.({ block: "nearest" });
  }, [listId, current]);

  const activate = (item: Item | undefined) => {
    if (!item) return;
    onDone();
    navigate(item.to);
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (items.length === 0) return;
      const step = event.key === "ArrowDown" ? 1 : -1;
      setActive((Math.max(current, 0) + step + items.length) % items.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      activate(items[current]);
    }
  };

  const loading = sessions === null;
  let index = 0;
  return (
    <>
      <div className="search-palette-field">
        <Search className="search-palette-icon" size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <input
          className="search-palette-input"
          type="text"
          role="combobox"
          aria-label="Search tabs and chats"
          aria-expanded="true"
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={current >= 0 ? optionId(current) : undefined}
          placeholder="Search tabs, chats and conversations"
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            // A new query starts from the first result again.
            setActive(0);
          }}
          onKeyDown={onKeyDown}
        />
        <kbd className="key-chip search-palette-key">{keyLabel(searchKeys())}</kbd>
      </div>
      <div className="search-palette-results thin-scroll" id={listId} role="listbox" aria-label="Results">
        {SECTIONS.map(({ kind, heading }) => {
          const group = items.filter((item) => item.kind === kind);
          if (group.length === 0) return null;
          const headingId = `${listId}-${kind}`;
          return (
            <div key={kind} role="group" aria-labelledby={headingId} className="search-palette-group">
              <div id={headingId} className="search-palette-heading" role="presentation">
                {heading}
              </div>
              {group.map((item) => {
                const position = index++;
                return (
                  <ResultRow
                    key={item.key}
                    id={optionId(position)}
                    item={item}
                    selected={position === current}
                    onHover={() => setActive(position)}
                    onChoose={() => activate(item)}
                  />
                );
              })}
            </div>
          );
        })}
        {items.length === 0 ? (
          <p className="search-palette-empty" role="status">
            {loading ? "Searching…" : trimmed ? `Nothing matches “${trimmed}”.` : "No open tabs or chats yet."}
          </p>
        ) : null}
      </div>
    </>
  );
}

function TabIcon({ tab }: { tab: OwnedTab }) {
  const icon = useFavicon(originOf(tab.url));
  return icon ? <img className="search-palette-favicon" src={icon} alt="" width={16} height={16} /> : <Globe size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
}

function ResultRow({ id, item, selected, onHover, onChoose }: { id: string; item: Item; selected: boolean; onHover: () => void; onChoose: () => void }) {
  const trailing = item.kind === "tab" ? "Switch" : item.kind === "chat" ? item.time : null;
  return (
    <div
      id={id}
      role="option"
      aria-selected={selected}
      className={cx("search-palette-row", selected && "active")}
      data-kind={item.kind}
      // Keep focus in the input so typing and arrow keys keep working after a pointer move.
      onMouseDown={(event) => event.preventDefault()}
      onMouseMove={selected ? undefined : onHover}
      onClick={onChoose}
    >
      <span className="search-palette-lead">
        {item.kind === "tab" ? (
          <TabIcon tab={item.tab} />
        ) : item.kind === "chat" ? (
          <Avatar chief={item.agent.kind === "chief"} initials={item.agent.initials} color={item.agent.color} size={24} />
        ) : (
          <MessageSquare size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        )}
      </span>
      <span className="search-palette-text">
        <span className="search-palette-title">{item.title}</span>
        {item.detail ? <span className="search-palette-detail">{item.detail}</span> : null}
      </span>
      {trailing ? <span className="search-palette-trailing">{trailing}</span> : null}
    </div>
  );
}

/**
 * Mod+Shift+A (or the sidebar's Search button): one field over the agents' open tabs, the
 * team's chats and the daemon's conversation search. A centered panel on wide screens, a bottom
 * sheet on phones; Base UI owns focus trapping, Escape, outside press and scroll lock.
 */
export function SearchPalette({ open, onClose, tabs }: { open: boolean; onClose: () => void; tabs: readonly OwnedTab[] }) {
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="ui-backdrop" />
        <BaseDialog.Viewport className="search-palette-viewport">
          <BaseDialog.Popup className="search-palette surface-overlay" aria-label="Search">
            {/* Mounted only while open, so every opening starts from an empty field. */}
            {open ? <Results tabs={tabs} onDone={onClose} /> : null}
          </BaseDialog.Popup>
        </BaseDialog.Viewport>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
