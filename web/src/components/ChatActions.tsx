import { Archive, ArchiveRestore, Bookmark, Copy, ExternalLink, Eye, EyeOff, Pencil, Pin, PinOff, Trash2 } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../api/client";
import type { Session } from "../api/types";
import { useClient } from "../context";
import { usePersonBrowser } from "../hooks/usePersonBrowser";
import { t } from "../i18n";
import { useShell } from "../shell";
import { Button, copyText, Dialog, DialogActions, ICON_STROKE, MenuItem, MenuSeparator, useToast } from "../ui";
import { RenameDialog } from "./SessionMenu";
import { browserFailure } from "./SidebarChats";
import { displaySessionTitle } from "./toolStatus";

const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

export interface ChatActions {
  update: (patch: { title?: string; pinned?: boolean; archived?: boolean; read?: boolean }, done?: string) => Promise<boolean>;
  setArchived: (archived: boolean) => Promise<void>;
  bookmark: () => Promise<void>;
  copyId: () => void;
  rename: () => void;
  confirmDelete: () => void;
  /** The rename and delete dialogs; render them next to the row. */
  dialogs: ReactNode;
}

/**
 * One chat's actions, shared by the sidebar's chat rows and the Chats page: every change refreshes
 * the shell's chat list (and `onChanged`), and leaving the open chat by archiving or deleting it
 * goes Home.
 */
export function useChatActions(session: Session, { current = false, onChanged }: { current?: boolean; onChanged?: () => void } = {}): ChatActions {
  const client = useClient();
  const toast = useToast();
  const shell = useShell();
  const navigate = useNavigate();
  const browser = usePersonBrowser();
  const [renaming, setRenaming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const path = `/sessions/${encodeURIComponent(session.id)}`;

  const changed = () => {
    shell.refreshSessions();
    onChanged?.();
  };

  async function update(patch: { title?: string; pinned?: boolean; archived?: boolean; read?: boolean }, done?: string) {
    try {
      await client.updateSession(session.id, patch);
      if (done) toast.notify(done);
      changed();
      return true;
    } catch (cause) {
      toast.notify(cause instanceof ApiError ? cause.message : t("shell.requestFailed"), "error");
      return false;
    }
  }

  async function setArchived(archived: boolean) {
    if (!(await update({ archived }, t(archived ? "shell.session.archived" : "shell.session.unarchived")))) return;
    if (archived && current) navigate("/");
  }

  async function remove() {
    setDeleteBusy(true);
    setDeleteError(null);
    try {
      await client.deleteSession(session.id);
      setDeleting(false);
      toast.notify(t("shell.session.deleted"));
      changed();
      if (current) navigate("/");
    } catch (cause) {
      setDeleteError(cause instanceof ApiError ? cause.message : t("shell.requestFailed"));
    } finally {
      setDeleteBusy(false);
    }
  }

  async function bookmark() {
    try {
      await client.createPersonBookmark({ parent_id: browser.bookmarkBar, title: displaySessionTitle(session.title), url: `${window.location.origin}${path}` });
      toast.notify(t("shell.chat.bookmarked"));
      browser.refreshBookmarks();
    } catch (cause) {
      toast.notify(browserFailure(cause), "error");
    }
  }

  const copyId = () => void copyText(session.id).then((copied) => toast.notify(t(copied ? "shell.session.idCopied" : "shell.session.idCopyFailed"), copied ? undefined : "error"));

  const dialogs = (
    <>
      <RenameDialog
        open={renaming}
        title={session.title}
        onClose={() => setRenaming(false)}
        onSave={async (title) => {
          if (await update({ title }, t("shell.session.renamed"))) setRenaming(false);
        }}
      />
      <Dialog open={deleting} onClose={() => { if (!deleteBusy) setDeleting(false); }} title={t("shell.session.deleteTitle")} description={t("shell.session.deleteDescription", { title: displaySessionTitle(session.title) })}>
        {deleteError ? <p role="alert">{deleteError}</p> : null}
        <DialogActions>
          <Button disabled={deleteBusy} onClick={() => setDeleting(false)}>
            {t("common.cancel")}
          </Button>
          <Button variant="danger" disabled={deleteBusy} onClick={() => void remove()}>
            {deleteBusy ? t("shell.session.deleting") : t("shell.chat.delete")}
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );

  return {
    update,
    setArchived,
    bookmark,
    copyId,
    rename: () => setRenaming(true),
    confirmDelete: () => {
      setDeleteError(null);
      setDeleting(true);
    },
    dialogs,
  };
}

/** Pin, Rename, Archive, Delete, Bookmark, Mark read, Copy session ID, Open in new tab (`L 33`). */
export function ChatMenuItems({ session, actions, deletable }: { session: Session; actions: ChatActions; deletable: boolean }) {
  const path = `/sessions/${encodeURIComponent(session.id)}`;
  return (
    <>
      <MenuItem icon={session.pinned ? <PinOff {...icon} /> : <Pin {...icon} />} onClick={() => void actions.update({ pinned: !session.pinned }, t(session.pinned ? "shell.session.unpinned" : "shell.session.pinned"))}>
        {t(session.pinned ? "shell.chat.unpin" : "shell.chat.pin")}
      </MenuItem>
      <MenuSeparator />
      <MenuItem icon={<Pencil {...icon} />} onClick={actions.rename}>
        {t("shell.chat.rename")}
      </MenuItem>
      <MenuItem icon={session.archived ? <ArchiveRestore {...icon} /> : <Archive {...icon} />} onClick={() => void actions.setArchived(!session.archived)}>
        {t(session.archived ? "shell.chat.unarchive" : "shell.chat.archive")}
      </MenuItem>
      {deletable ? (
        <MenuItem icon={<Trash2 {...icon} />} danger onClick={actions.confirmDelete}>
          {t("shell.chat.delete")}
        </MenuItem>
      ) : null}
      <MenuItem icon={<Bookmark {...icon} />} onClick={() => void actions.bookmark()}>
        {t("shell.chat.bookmark")}
      </MenuItem>
      <MenuItem icon={session.unread ? <Eye {...icon} /> : <EyeOff {...icon} />} onClick={() => void actions.update({ read: session.unread })}>
        {t(session.unread ? "shell.chat.markRead" : "shell.chat.markUnread")}
      </MenuItem>
      <MenuSeparator />
      <MenuItem icon={<Copy {...icon} />} onClick={actions.copyId}>
        {t("shell.session.copyId")}
      </MenuItem>
      <MenuSeparator />
      <MenuItem icon={<ExternalLink {...icon} />} onClick={() => window.open(path, "_blank", "noopener")}>
        {t("shell.chat.openNewTab")}
      </MenuItem>
    </>
  );
}
