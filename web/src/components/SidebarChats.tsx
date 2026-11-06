import { Archive, SquarePen } from "lucide-react";
import { Link, useMatch } from "react-router";
import { ApiError } from "../api/client";
import type { Session } from "../api/types";
import { t } from "../i18n";
import { ContextMenu, cx, ICON_STROKE, Spinner } from "../ui";
import { ChatMenuItems, useChatActions } from "./ChatActions";
import { displaySessionTitle } from "./toolStatus";

const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** What a failed request says to the person; the extension's absence gets its own sentence. */
export function browserFailure(cause: unknown): string {
  if (cause instanceof ApiError && cause.code === "person_browser_not_connected") return t("shell.browser.notConnected");
  return cause instanceof ApiError ? cause.message : t("shell.requestFailed");
}

export interface SidebarChatsProps {
  /** The shell's chat list in the daemon's order (pinned first, then most recent); `null` while loading. */
  sessions: readonly Session[] | null;
  count: number;
  /** Agent home conversations, which cannot be deleted from here; `null` until the roster has loaded. */
  homes: ReadonlySet<string> | null;
}

/** Chats: New Chat, the most recent chats, and Show all (`L 20`, `L 64`). */
export function SidebarChats({ sessions, count, homes }: SidebarChatsProps) {
  const openId = useMatch("/sessions/:id")?.params.id ?? null;
  const recent = (sessions ?? []).filter((session) => !session.archived).slice(0, count);
  const onChats = useMatch("/chats") !== null;
  return (
    <section className="sidebar-section" aria-labelledby="sidebar-chats-heading">
      <h2 id="sidebar-chats-heading" className="sidebar-heading">
        {t("shell.sidebar.chats")}
      </h2>
      <ul className="sidebar-rows sidebar-chats">
        <li>
          <Link to="/" className="sidebar-row sidebar-row--secondary">
            <SquarePen {...icon} />
            <span>{t("shell.sidebar.newChat")}</span>
          </Link>
        </li>
        {recent.map((session) => (
          <ChatRow key={session.id} session={session} current={session.id === openId} deletable={homes !== null && !homes.has(session.id)} />
        ))}
        {/* Always shown, also with no chats yet: it is the way to the Chats tab and its archived chats. */}
        <li>
          <Link to="/chats" className="sidebar-row sidebar-row--secondary" aria-current={onChats ? "page" : undefined}>
            <span>{t("shell.sidebar.showAll")}</span>
          </Link>
        </li>
      </ul>
    </section>
  );
}

function ChatRow({ session, current, deletable }: { session: Session; current: boolean; deletable: boolean }) {
  const actions = useChatActions(session, { current });
  const path = `/sessions/${encodeURIComponent(session.id)}`;
  const working = session.status === "running";
  const needsYou = session.status === "waiting_human";
  const state = needsYou ? t("shell.person.needsYou") : working ? t("shell.person.working") : session.unread ? t("shell.person.unread") : null;
  const title = displaySessionTitle(session.title);

  return (
    <>
      <ContextMenu
        label={t("shell.chat.menu", { title })}
        trigger={
          <li className={cx("chat-row", current && "current", session.unread && "unread")} data-session={session.id}>
            <Link
              to={path}
              className="sidebar-row"
              aria-current={current ? "page" : undefined}
              aria-label={state ? t("shell.person.label", { name: title, state }) : undefined}
              title={title}
            >
              <span className="sidebar-row-label">{title}</span>
            </Link>
            {/* Siblings of the link, not inside it: a button cannot nest in an anchor. The state mark
                stays while Archive shows, moved aside (`.chat-status`). */}
            {working ? (
              <span className="sidebar-row-trailing chat-status chat-spinner">
                <Spinner size={14} />
              </span>
            ) : needsYou ? (
              <span className="sidebar-row-trailing chat-status row-badge" aria-hidden="true">
                {t("shell.person.needsYou")}
              </span>
            ) : session.unread ? (
              <span className="sidebar-row-trailing unread-dot" aria-hidden="true" />
            ) : null}
            <button type="button" className="sidebar-row-trailing sidebar-row-action" aria-label={t("shell.chat.archiveNamed", { title })} title={t("shell.chat.archive")} onClick={() => void actions.setArchived(true)}>
              <Archive size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </button>
          </li>
        }
      >
        <ChatMenuItems session={session} actions={actions} deletable={deletable} />
      </ContextMenu>
      {actions.dialogs}
    </>
  );
}
