import { Archive, ArchiveRestore, Ellipsis, Trash2 } from "lucide-react";
import { useEffect, useMemo } from "react";
import { Link } from "react-router";
import type { Session } from "../../api/types";
import { useChatActions } from "../../components/ChatActions";
import { relativeTime } from "../../components/SearchPalette";
import { displaySessionTitle } from "../../components/toolStatus";
import { useClient } from "../../context";
import { useTeam } from "../../hooks/useAgents";
import { useSessionList } from "../../hooks/useSessionList";
import { t } from "../../i18n";
import { EmptyState, ICON_STROKE, IconButton, MenuItem, PopoverMenu, SettingsCard, SettingsRow, Spinner } from "../../ui";
import "./archived.css";

const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** One archived chat: its title opens the thread; ⋯ unarchives it or, unless it is an agent's home chat, deletes it. */
function ArchivedRow({ session, deletable, onChanged }: { session: Session; deletable: boolean; onChanged: () => void }) {
  const actions = useChatActions(session, { onChanged });
  const title = displaySessionTitle(session.title);
  const label = t("shell.chat.menu", { title });
  return (
    <>
      <SettingsRow
        title={
          <Link to={`/sessions/${encodeURIComponent(session.id)}`} className="settings-row-link" title={title}>
            {title}
          </Link>
        }
        description={relativeTime(session.last_activity_at) ?? undefined}
        control={
          <PopoverMenu
            label={label}
            trigger={
              <IconButton label={label} size="small">
                <Ellipsis {...icon} />
              </IconButton>
            }
          >
            <MenuItem icon={<ArchiveRestore {...icon} />} onClick={() => void actions.setArchived(false)}>
              {t("shell.chat.unarchive")}
            </MenuItem>
            {deletable ? (
              <MenuItem icon={<Trash2 {...icon} />} danger onClick={actions.confirmDelete}>
                {t("shell.chat.delete")}
              </MenuItem>
            ) : null}
          </PopoverMenu>
        }
      />
      {actions.dialogs}
    </>
  );
}

/**
 * Settings › Archived chats, in the daemon's order. The page title's ⓘ (SettingsLayout) says
 * what archiving does.
 */
export function ArchivedChats() {
  const client = useClient();
  const list = useSessionList(client);
  const team = useTeam();
  const { sessions, setArchived } = list;
  useEffect(() => setArchived(true), [setArchived]);
  // Until the roster arrives, an agent's home chat cannot be told apart, so nothing is deletable.
  const homes = useMemo(() => (team.agents === null ? null : new Set(team.agents.map((agent) => agent.home_session_id))), [team.agents]);

  if (!list.archived || sessions === null) return list.error ? <p className="settings-text">{list.error}</p> : <Spinner size={16} />;
  if (sessions.length === 0) {
    return (
      <div className="archived-empty">
        <EmptyState icon={<Archive {...icon} />} title={t("shell.chats.archivedEmptyTitle")} description={t("shell.chats.archivedEmptyDetail")} />
      </div>
    );
  }
  return (
    <SettingsCard>
      {sessions.map((session) => (
        <ArchivedRow key={session.id} session={session} deletable={homes !== null && !homes.has(session.id)} onChanged={list.refresh} />
      ))}
    </SettingsCard>
  );
}
