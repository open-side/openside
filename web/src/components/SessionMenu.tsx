import { Archive, ArchiveRestore, Copy, Link as LinkIcon, Pencil, Pin, PinOff } from "lucide-react";
import { useRef, useState, type FormEvent, type ReactElement } from "react";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { Button, copyText, Dialog, DialogActions, ICON_STROKE, MenuItem, MenuSeparator, PopoverMenu, useToast } from "../ui";

export interface SessionSummary {
  id: string;
  title: string;
  pinned: boolean;
  archived: boolean;
}

export interface SessionMenuProps {
  session: SessionSummary;
  trigger: ReactElement;
  /** Accessible name of the menu popup. */
  label: string;
  /** Called after a successful rename, pin or archive so lists can reload. */
  onChanged?: () => void;
}

const icon = { size: 18, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** The conversation actions shared by the header and every sidebar row. */
export function SessionMenu({ session, trigger, label, onChanged }: SessionMenuProps) {
  const client = useClient();
  const toast = useToast();
  const [renaming, setRenaming] = useState(false);

  async function update(patch: { title?: string; pinned?: boolean; archived?: boolean }, done: string) {
    try {
      await client.updateSession(session.id, patch);
      toast.notify(done);
      onChanged?.();
      return true;
    } catch (cause) {
      toast.notify(cause instanceof ApiError ? cause.message : "Request failed", "error");
      return false;
    }
  }

  async function copy(text: string, what: string) {
    if (await copyText(text)) toast.notify(`${what} copied`);
    else toast.notify(`Could not copy the ${what.toLowerCase()}`, "error");
  }

  return (
    <>
      <PopoverMenu trigger={trigger} label={label} title={session.title}>
        <MenuItem
          icon={session.pinned ? <PinOff {...icon} /> : <Pin {...icon} />}
          onClick={() => void update({ pinned: !session.pinned }, session.pinned ? "Conversation unpinned" : "Conversation pinned")}
        >
          {session.pinned ? "Unpin conversation" : "Pin conversation"}
        </MenuItem>
        <MenuItem icon={<Pencil {...icon} />} onClick={() => setRenaming(true)}>
          Rename
        </MenuItem>
        <MenuItem
          icon={session.archived ? <ArchiveRestore {...icon} /> : <Archive {...icon} />}
          onClick={() => void update({ archived: !session.archived }, session.archived ? "Conversation restored" : "Conversation archived")}
        >
          {session.archived ? "Unarchive" : "Archive"}
        </MenuItem>
        <MenuSeparator />
        <MenuItem icon={<LinkIcon {...icon} />} onClick={() => void copy(`${window.location.origin}/sessions/${encodeURIComponent(session.id)}`, "Link")}>
          Copy link
        </MenuItem>
        <MenuItem icon={<Copy {...icon} />} onClick={() => void copy(session.id, "Session ID")}>
          Copy session ID
        </MenuItem>
      </PopoverMenu>
      <RenameDialog
        open={renaming}
        title={session.title}
        onClose={() => setRenaming(false)}
        onSave={async (title) => {
          if (await update({ title }, "Conversation renamed")) setRenaming(false);
        }}
      />
    </>
  );
}

function RenameDialog({ open, title, onClose, onSave }: { open: boolean; title: string; onClose: () => void; onSave: (title: string) => Promise<void> }) {
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
    <Dialog open={open} onClose={onClose} title="Rename conversation" initialFocus={input}>
      <form className="dialog-form" onSubmit={submit}>
        <label className="field">
          <span className="sr-only">Conversation name</span>
          <input ref={input} defaultValue={title} maxLength={200} autoComplete="off" />
        </label>
        <DialogActions>
          <Button onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy}>
            Save
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}
