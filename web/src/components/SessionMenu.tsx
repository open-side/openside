import { Archive, ArchiveRestore, Copy, MessageSquareWarning, Pencil, Trash2 } from "lucide-react";
import { useRef, useState, type FormEvent, type ReactElement } from "react";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { openInBrowser } from "../hooks/usePersonBrowser";
import { t } from "../i18n";
import { SETTINGS_FOOTER } from "../settings/nav";
import { Button, copyText, Dialog, DialogActions, ICON_STROKE, MenuItem, MenuSeparator, PopoverMenu, useToast } from "../ui";

export interface SessionSummary {
  id: string;
  title: string;
  archived: boolean;
}

export interface SessionMenuProps {
  session: SessionSummary;
  trigger: ReactElement;
  /** Accessible name of the menu popup. */
  label: string;
  /** Refreshes lists after a successful mutation. */
  onChanged?: () => void;
  onDeleted?: () => void;
}

const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;
const FEEDBACK_URL = SETTINGS_FOOTER.find((link) => link.key === "feedback")?.url ?? "";

/** A chat's ⋯ menu in its header: Rename, Send feedback, Copy session ID, Archive, Delete. */
export function SessionMenu({ session, trigger, label, onChanged, onDeleted }: SessionMenuProps) {
  const client = useClient();
  const toast = useToast();
  const [renaming, setRenaming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const team = useTeam();
  // Until the roster arrives, a home conversation cannot safely be identified.
  const canDelete = team.agents !== null && !team.agents.some((agent) => agent.home_session_id === session.id);

  async function update(patch: { title?: string; archived?: boolean }, done: string) {
    try {
      await client.updateSession(session.id, patch);
      toast.notify(done);
      onChanged?.();
      return true;
    } catch (cause) {
      toast.notify(cause instanceof ApiError ? cause.message : t("shell.requestFailed"), "error");
      return false;
    }
  }

  async function remove() {
    setDeleteBusy(true);
    setDeleteError(null);
    try {
      await client.deleteSession(session.id);
      setDeleting(false);
      toast.notify(t("shell.session.deleted"));
      onChanged?.();
      onDeleted?.();
    } catch (cause) {
      setDeleteError(cause instanceof ApiError ? cause.message : t("shell.requestFailed"));
    } finally {
      setDeleteBusy(false);
    }
  }

  async function copyId() {
    if (await copyText(session.id)) toast.notify(t("shell.session.idCopied"));
    else toast.notify(t("shell.session.idCopyFailed"), "error");
  }

  return (
    <>
      <PopoverMenu trigger={trigger} label={label} align="start" popupClassName="session-menu">
        <MenuItem icon={<Pencil {...icon} />} onClick={() => setRenaming(true)}>
          {t("common.rename")}
        </MenuItem>
        <MenuItem icon={<MessageSquareWarning {...icon} />} onClick={() => void openInBrowser(client, FEEDBACK_URL)}>
          {t("shell.session.feedback")}
        </MenuItem>
        <MenuItem icon={<Copy {...icon} />} onClick={() => void copyId()}>
          {t("shell.session.copyId")}
        </MenuItem>
        <MenuSeparator />
        <MenuItem
          icon={session.archived ? <ArchiveRestore {...icon} /> : <Archive {...icon} />}
          onClick={() => void update({ archived: !session.archived }, t(session.archived ? "shell.session.unarchived" : "shell.session.archived"))}
        >
          {t(session.archived ? "shell.session.unarchive" : "shell.session.archive")}
        </MenuItem>
        {canDelete ? (
          <MenuItem icon={<Trash2 {...icon} />} danger onClick={() => { setDeleteError(null); setDeleting(true); }}>
            {t("shell.session.delete")}
          </MenuItem>
        ) : null}
      </PopoverMenu>
      <RenameDialog
        open={renaming}
        title={session.title}
        onClose={() => setRenaming(false)}
        onSave={async (title) => {
          if (await update({ title }, t("shell.session.renamed"))) setRenaming(false);
        }}
      />
      <Dialog
        open={deleting}
        onClose={() => { if (!deleteBusy) setDeleting(false); }}
        title={t("shell.session.deleteTitle")}
        description={t("shell.session.deleteDescription", { title: session.title })}
      >
        {deleteError ? <p role="alert">{deleteError}</p> : null}
        <DialogActions>
          <Button disabled={deleteBusy} onClick={() => setDeleting(false)}>{t("common.cancel")}</Button>
          <Button variant="danger" disabled={deleteBusy} onClick={() => void remove()}>
            {deleteBusy ? t("shell.session.deleting") : t("shell.session.delete")}
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}

export function RenameDialog({ open, title, onClose, onSave }: { open: boolean; title: string; onClose: () => void; onSave: (title: string) => Promise<void> }) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const next = input.current?.value.trim() ?? "";
    if (!next || next === title) return onClose();
    setBusy(true);
    await onSave(next);
    setBusy(false);
  }

  return (
    <Dialog open={open} onClose={onClose} title={t("shell.session.renameTitle")} initialFocus={input}>
      <form className="dialog-form" onSubmit={submit}>
        <label className="field">
          <span className="sr-only">{t("shell.session.nameLabel")}</span>
          <input ref={input} defaultValue={title} maxLength={200} autoComplete="off" />
        </label>
        <DialogActions>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button type="submit" variant="primary" disabled={busy}>
            {t("common.save")}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}
