import { ArrowDown, Ellipsis, Filter, LayoutGrid, List, Pin, Search, X } from "lucide-react";
import { useEffect, useMemo, useState, type KeyboardEvent, type MouseEvent } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
import { ApiError } from "../api/client";
import type { Project, Session } from "../api/types";
import { AppHeader } from "../components/AppHeader";
import { ChatMenuItems, useChatActions } from "../components/ChatActions";
import { Composer } from "../components/Composer";
import { relativeTime } from "../components/SearchPalette";
import { displayTitle } from "../components/toolStatus";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { useSessionList } from "../hooks/useSessionList";
import { t } from "../i18n";
import { useShell } from "../shell";
import { Button, ContextMenu, cx, Dialog, DialogActions, ICON_STROKE, IconButton, MenuGroupLabel, MenuItem, MenuSeparator, PopoverMenu, Spinner, useToast } from "../ui";
import { rovingChoice } from "../ui/misc";
import "./chats.css";

type View = "list" | "card";
type Show = "all" | "unread" | "pinned" | "archived";
type Sort = "recent" | "oldest" | "title";

export const CHATS_VIEW_KEY = "openside.chats.view";

const SHOWS: readonly Show[] = ["all", "unread", "pinned", "archived"];
const SHOW_LABEL = { all: "shell.chats.showAll", unread: "shell.chats.showUnread", pinned: "shell.chats.showPinned", archived: "shell.chats.showArchived" } as const;
const SORT_LABEL = { recent: "shell.chats.sortRecent", oldest: "shell.chats.sortOldest", title: "shell.chats.sortTitle" } as const;
const icon = { size: 14, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;
// The page's own tools draw 12px glyphs on a 28px pitch (`L 71`).
const toolIcon = { ...icon, size: 12 } as const;

const activity = (session: Session) => (session.last_activity_at ? Date.parse(session.last_activity_at) : 0);

/** Pinned chats stay on top in every order, as the daemon lists them. */
function ordered(sessions: readonly Session[], show: Show, sort: Sort): Session[] {
  const shown = sessions.filter((session) => (show === "unread" ? session.unread : show === "pinned" ? session.pinned : true));
  if (sort === "recent") return shown;
  const by = sort === "oldest" ? (a: Session, b: Session) => activity(a) - activity(b) : (a: Session, b: Session) => displayTitle(a.title).localeCompare(displayTitle(b.title));
  return [...shown].sort((a, b) => Number(b.pinned) - Number(a.pinned) || by(a, b));
}

function loadView(): View {
  return localStorage.getItem(CHATS_VIEW_KEY) === "card" ? "card" : "list";
}

/**
 * The Chats tab (Show all, `L 71`): a composer for a new chat, then every chat as a list or as
 * cards, with search, a filter (all, unread, pinned, archived, and a project), an order, and a selection for
 * marking read, archiving or deleting several at once. `?show=archived` opens the archived ones; `?project=<id>` one project's.
 */
export function ChatsPage() {
  const client = useClient();
  const shell = useShell();
  const toast = useToast();
  const navigate = useNavigate();
  const team = useTeam();
  const list = useSessionList(client);
  const [params, setParams] = useSearchParams();
  const showParam = params.get("show");
  const show: Show = SHOWS.find((value) => value === showParam) ?? "all";
  const project = params.get("project");
  const [projects, setProjects] = useState<Project[]>([]);
  const [view, setViewState] = useState<View>(loadView);
  const [sort, setSort] = useState<Sort>("recent");
  const [searching, setSearching] = useState(false);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const { setArchived, setQuery, refresh } = list;
  const homes = useMemo(() => (team.agents === null ? null : new Set(team.agents.map((agent) => agent.home_session_id))), [team.agents]);

  useEffect(() => setArchived(show === "archived"), [setArchived, show]);
  useEffect(() => {
    let live = true;
    client.listProjects().then(
      (found) => live && setProjects(found.projects),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [client]);

  const visible = useMemo(() => ordered((list.sessions ?? []).filter((session) => !project || session.project_id === project), show, sort), [list.sessions, project, show, sort]);
  // A chat that left the list (archived elsewhere, deleted, filtered out) leaves the selection too.
  const chosen = visible.filter((session) => selected.has(session.id));
  const deletable = homes === null ? [] : chosen.filter((session) => !homes.has(session.id));

  const setView = (next: View) => {
    setViewState(next);
    localStorage.setItem(CHATS_VIEW_KEY, next);
  };
  const setFilter = (nextShow: Show, nextProject: string | null) => {
    setSelected(new Set());
    setParams({ ...(nextShow === "all" ? {} : { show: nextShow }), ...(nextProject ? { project: nextProject } : {}) }, { replace: true });
  };
  const closeSearch = () => {
    setSearching(false);
    setQuery("");
  };

  const toggle = (session: Session, range: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      const from = anchor === null ? -1 : visible.findIndex((entry) => entry.id === anchor);
      const to = visible.findIndex((entry) => entry.id === session.id);
      if (range && from >= 0 && to >= 0) {
        // Shift extends from the last chat clicked to this one, all taking that chat's new state.
        const on = !current.has(session.id);
        for (const entry of visible.slice(Math.min(from, to), Math.max(from, to) + 1)) {
          if (on) next.add(entry.id);
          else next.delete(entry.id);
        }
      } else if (!next.delete(session.id)) next.add(session.id);
      return next;
    });
    setAnchor(session.id);
  };

  async function bulk(targets: readonly Session[], change: (session: Session) => Promise<unknown>, done: (count: number) => string) {
    setBulkBusy(true);
    const results = await Promise.allSettled(targets.map(change));
    setBulkBusy(false);
    const failed = results.filter((result) => result.status === "rejected");
    const succeeded = results.length - failed.length;
    if (succeeded > 0) toast.notify(done(succeeded));
    if (failed.length > 0) {
      const reason = failed[0]?.status === "rejected" && failed[0].reason instanceof ApiError ? failed[0].reason.message : t("shell.requestFailed");
      toast.notify(t("shell.chats.bulkFailed", { count: failed.length, reason }), "error");
    }
    setSelected(new Set());
    refresh();
    shell.refreshSessions();
  }

  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape" && selected.size > 0 && !event.defaultPrevented) {
      event.preventDefault();
      setSelected(new Set());
    }
  };

  const archivedView = show === "archived";
  // After switching to or from Archived, the previous answer stays until the new one lands.
  const loading = list.sessions === null || list.sessions.some((session) => session.archived !== archivedView);

  return (
    <main className="chats-page" onKeyDown={onKeyDown}>
      <AppHeader title={t("shell.sidebar.chats")} newTask={false} />
      <div className="chats-scroll thin-scroll">
        <div className="chats-column">
          <div className="chats-composer">
            <Composer variant="home" onSent={(created) => navigate(created ? `/sessions/${encodeURIComponent(created)}` : "/")} />
          </div>

          <div className="chats-toolbar">
            {chosen.length > 0 ? (
              <div className="chats-bulk" role="toolbar" aria-label={t("shell.chats.bulkLabel")}>
                <IconButton label={t("shell.chats.clearSelection")} size="small" onClick={() => setSelected(new Set())}>
                  <X {...icon} />
                </IconButton>
                <span className="chats-bulk-count" role="status">
                  {t("shell.chats.selectedCount", { count: chosen.length })}
                </span>
                <Button size="small" variant="ghost" disabled={bulkBusy || chosen.length === visible.length} onClick={() => setSelected(new Set(visible.map((session) => session.id)))}>
                  {t("shell.chats.selectAll")}
                </Button>
                <span className="chats-bulk-actions">
                  {archivedView ? null : (
                    <Button size="small" variant="ghost" disabled={bulkBusy || !chosen.some((session) => session.unread)} onClick={() => void bulk(chosen.filter((session) => session.unread), (session) => client.updateSession(session.id, { read: true }), (count) => t("shell.chats.bulkRead", { count }))}>
                      {t("shell.chat.markRead")}
                    </Button>
                  )}
                  <Button
                    size="small"
                    variant="ghost"
                    disabled={bulkBusy}
                    onClick={() => void bulk(chosen, (session) => client.updateSession(session.id, { archived: !archivedView }), (count) => t(archivedView ? "shell.chats.bulkUnarchived" : "shell.chats.bulkArchived", { count }))}
                  >
                    {t(archivedView ? "shell.session.unarchive" : "shell.session.archive")}
                  </Button>
                  <Button size="small" variant="ghost" disabled={bulkBusy || deletable.length === 0} onClick={() => setConfirmDelete(true)}>
                    {t("shell.chats.delete")}
                  </Button>
                </span>
              </div>
            ) : (
              <>
                <ViewSwitch value={view} onChange={setView} />
                {searching ? (
                  <label className="chats-search">
                    <Search {...icon} />
                    <span className="sr-only">{t("shell.chats.search")}</span>
                    <input
                      type="search"
                      autoFocus
                      value={list.query}
                      placeholder={t("shell.chats.search")}
                      onChange={(event) => setQuery(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === "Escape") {
                          event.preventDefault();
                          closeSearch();
                        }
                      }}
                    />
                    <IconButton label={t("shell.chats.closeSearch")} size="small" onClick={closeSearch}>
                      <X {...icon} />
                    </IconButton>
                  </label>
                ) : null}
                <div className="chats-tools">
                  {searching ? null : (
                    <IconButton label={t("shell.chats.search")} size="small" onClick={() => setSearching(true)}>
                      <Search {...toolIcon} />
                    </IconButton>
                  )}
                  <PopoverMenu
                    label={t("shell.chats.filter")}
                    trigger={
                      <IconButton label={t("shell.chats.filter")} size="small" className={cx((show !== "all" || project) && "is-on")}>
                        <Filter {...toolIcon} />
                      </IconButton>
                    }
                  >
                    {SHOWS.map((value) => (
                      <MenuItem key={value} checked={show === value} onClick={() => setFilter(value, project)}>
                        {t(SHOW_LABEL[value])}
                      </MenuItem>
                    ))}
                    {projects.length ? (
                      <>
                        <MenuSeparator />
                        <MenuGroupLabel>{t("shell.chats.projects")}</MenuGroupLabel>
                        {projects.map((entry) => (
                          <MenuItem key={entry.id} checked={project === entry.id} onClick={() => setFilter(show, project === entry.id ? null : entry.id)}>
                            {entry.name}
                          </MenuItem>
                        ))}
                      </>
                    ) : null}
                  </PopoverMenu>
                  <PopoverMenu
                    label={t("shell.chats.sort")}
                    trigger={
                      <IconButton label={t("shell.chats.sort")} size="small" className={cx(sort !== "recent" && "is-on")}>
                        <ArrowDown {...toolIcon} />
                      </IconButton>
                    }
                  >
                    {(["recent", "oldest", "title"] as const).map((value) => (
                      <MenuItem key={value} checked={sort === value} onClick={() => setSort(value)}>
                        {t(SORT_LABEL[value])}
                      </MenuItem>
                    ))}
                  </PopoverMenu>
                </div>
              </>
            )}
          </div>

          {list.error && !list.offline ? (
            <p role="alert" className="chats-note error">
              {list.error}
            </p>
          ) : null}

          {loading ? (
            list.offline ? null : (
              <p className="chats-note" role="status">
                <Spinner size={14} label={t("shell.chats.loading")} />
              </p>
            )
          ) : visible.length === 0 ? (
            <p className="chats-empty" role="status">
              {t("shell.chats.empty")}
            </p>
          ) : (
            <ul className={view === "card" ? "chats-cards" : "chats-list"} aria-label={t(archivedView ? "shell.chats.archivedList" : "shell.chats.allList")} data-selecting={chosen.length > 0 || undefined}>
              {visible.map((session) => (
                <ChatEntry key={session.id} session={session} view={view} selected={selected.has(session.id)} deletable={homes !== null && !homes.has(session.id)} onToggle={(range) => toggle(session, range)} onChanged={refresh} />
              ))}
            </ul>
          )}
        </div>
      </div>
      <Dialog
        open={confirmDelete}
        onClose={() => {
          if (!bulkBusy) setConfirmDelete(false);
        }}
        title={t("shell.chats.deleteTitle", { count: deletable.length })}
        description={t("shell.chats.deleteDescription", { count: deletable.length })}
      >
        <DialogActions>
          <Button disabled={bulkBusy} onClick={() => setConfirmDelete(false)}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="danger"
            disabled={bulkBusy}
            onClick={() =>
              void bulk(deletable, (session) => client.deleteSession(session.id), (count) => t("shell.chats.bulkDeleted", { count })).then(() => setConfirmDelete(false))
            }
          >
            {bulkBusy ? t("shell.session.deleting") : t("shell.chats.delete")}
          </Button>
        </DialogActions>
      </Dialog>
    </main>
  );
}

/** List | Card (`L 71`): a radio pair with glyphs; arrows, Home and End move the choice. */
function ViewSwitch({ value, onChange }: { value: View; onChange: (view: View) => void }) {
  const options = [
    { value: "list" as const, label: t("shell.chats.list"), glyph: <List {...icon} /> },
    { value: "card" as const, label: t("shell.chats.card"), glyph: <LayoutGrid {...icon} /> },
  ];
  const chosen = options.findIndex((option) => option.value === value);
  return (
    <div className="chats-view" role="radiogroup" aria-label={t("shell.chats.viewLabel")} onKeyDown={(event) => rovingChoice(event, options, chosen, ".chats-view-option", (option) => onChange(option.value))}>
      {options.map((option, index) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          tabIndex={index === chosen ? 0 : -1}
          className={cx("chats-view-option", option.value === value && "is-selected")}
          onClick={() => onChange(option.value)}
        >
          {option.glyph}
          {option.label}
        </button>
      ))}
    </div>
  );
}

function ChatEntry({ session, view, selected, deletable, onToggle, onChanged }: { session: Session; view: View; selected: boolean; deletable: boolean; onToggle: (range: boolean) => void; onChanged: () => void }) {
  const actions = useChatActions(session, { onChanged });
  const title = displayTitle(session.title);
  const working = session.status === "running";
  const needsYou = session.status === "waiting_human";
  const time = relativeTime(session.last_activity_at);
  const path = `/sessions/${encodeURIComponent(session.id)}`;

  const status = working ? (
    <span className="chats-status">
      <Spinner size={12} label={t("shell.person.working")} />
    </span>
  ) : needsYou ? (
    <span className="chats-status row-badge">{t("shell.person.needsYou")}</span>
  ) : session.unread ? (
    <span className="chats-status unread-dot" role="img" aria-label={t("shell.person.unread")} />
  ) : null;

  const check = (
    <input
      type="checkbox"
      className="chats-check"
      checked={selected}
      aria-label={t("shell.chats.select", { title })}
      onChange={() => undefined}
      onClick={(event: MouseEvent<HTMLInputElement>) => onToggle(event.shiftKey)}
    />
  );
  const menu = (
    <PopoverMenu
      label={t("shell.chat.menu", { title })}
      trigger={
        <IconButton label={t("shell.chat.menu", { title })} size="small" className="chats-more">
          <Ellipsis size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      }
    >
      <ChatMenuItems session={session} actions={actions} deletable={deletable} />
    </PopoverMenu>
  );

  return (
    <>
      <ContextMenu
        label={t("shell.chat.menu", { title })}
        trigger={
          <li className={cx(view === "card" ? "chats-card" : "chats-row", selected && "is-selected", session.unread && "is-unread")} data-session={session.id}>
            {check}
            <Link to={path} className="chats-link" title={session.title}>
              <span className="chats-title">
                {session.pinned ? <Pin className="chats-pin" size={12} strokeWidth={ICON_STROKE} role="img" aria-label={t("shell.chats.pinned")} /> : null}
                <span className="chats-title-text">{title}</span>
              </span>
              {view === "card" && session.activity ? <span className="chats-activity">{session.activity}</span> : null}
              <span className="chats-meta">
                {status}
                {time ? <span className="chats-time">{time}</span> : null}
              </span>
            </Link>
            {menu}
          </li>
        }
      >
        <ChatMenuItems session={session} actions={actions} deletable={deletable} />
      </ContextMenu>
      {actions.dialogs}
    </>
  );
}
