import { useState, type FormEvent } from "react";
import { ApiError } from "../api/client";
import type { ExternalAgent } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { Button, Dialog, DialogActions } from "../ui";
import { ErrorNote } from "./blocks";

const NAME = /^[a-z0-9-]+$/;

/**
 * Tells openside where another coding agent keeps its skills, so the browser skill can be
 * installed for it. With `agent` the name is fixed and only the folder changes.
 */
export function AgentFolderDialog({
  open,
  agent,
  onClose,
  onSaved,
}: {
  open: boolean;
  agent?: ExternalAgent;
  onClose: () => void;
  onSaved: (id: string) => void;
}) {
  const client = useClient();
  const [name, setName] = useState(agent?.id ?? "");
  const [folder, setFolder] = useState(agent?.skills_dir ?? "");
  const [error, setError] = useState<string | null>(null);
  const [localOnly, setLocalOnly] = useState(false);
  const [busy, setBusy] = useState(false);
  const id = agent?.id ?? name.trim();

  async function save(event?: FormEvent) {
    event?.preventDefault();
    setError(null);
    setLocalOnly(false);
    if (!NAME.test(id)) {
      setError("Name the agent with lowercase letters, digits and hyphens, like my-agent.");
      return;
    }
    setBusy(true);
    try {
      await client.setExternalAgent(id, folder.trim());
      onSaved(id);
    } catch (cause) {
      setLocalOnly(cause instanceof ApiError && cause.code === "local_access_required");
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title={agent ? `Skills folder of ${agent.label}` : "Add an agent"} description="openside installs its browser skill into this folder when you ask it to.">
      <form className="dialog-form" onSubmit={(event) => void save(event)}>
        <label className="field">
          <span>Agent name</span>
          <input
            value={agent?.id ?? name}
            readOnly={agent !== undefined}
            maxLength={40}
            placeholder="my-agent"
            autoCapitalize="none"
            spellCheck={false}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label className="field">
          <span>Skills folder</span>
          <input value={folder} placeholder="~/.my-agent/skills" spellCheck={false} onChange={(event) => setFolder(event.target.value)} />
        </label>
        <p className="settings-text">
          Use the full path, or start with <code>~/</code>. The folder must be inside your home folder.
        </p>
        {localOnly ? <LocalOnlyNotice what="change an agent" onChanged={() => void save()} /> : error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy || !id || !folder.trim()}>
            {busy ? "Saving…" : "Save"}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}
