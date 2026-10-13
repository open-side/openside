import { useState, type FormEvent } from "react";
import type { Connection, ConnectionAccess, ReloginResult, RevokeConnectionRequest } from "../api/types";
import { Avatar } from "../components/Avatar";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { errorMessage } from "../hooks/useResource";
import { Button, Dialog, DialogActions, Select } from "../ui";
import { ErrorNote } from "./blocks";
import { ACCESS_OPTIONS, accessOf, activeAgents, identityLine } from "./connectionsView";

export type RevokeMode = "forget" | "sign_out";

/** Forget keeps the browser signed in; sign out also clears the site's data, so each says what it ends. */
export function RevokeDialog({
  open,
  onClose,
  target,
  onConfirm,
}: {
  open: boolean;
  onClose: () => void;
  target: { connection: Connection; mode: RevokeMode } | null;
  onConfirm: (connection: Connection, request: RevokeConnectionRequest) => void;
}) {
  const [vaultToo, setVaultToo] = useState(false);
  if (!target) return null;
  const { connection, mode } = target;
  const signOut = mode === "sign_out";
  const separate = connection.account_space !== null;
  const description = separate
    ? `This deletes the separate browser profile for this ${connection.site} account, with its cookies and site data. Your other accounts on ${connection.site} are not touched, and agents lose access to this one.`
    : signOut
      ? `This clears the cookies and site data for ${connection.site} in the agent browser. Every account signed in to ${connection.site} there is signed out, and agents lose access.`
      : `This removes ${connection.site} from this list along with its per-agent access settings. The browser stays signed in, so agents can still open ${connection.site} like any other site. To end the sign-in, use Sign out.`;
  const title = separate ? `Remove this ${connection.site} account?` : signOut ? `Sign out of ${connection.site}?` : `Forget ${connection.site}?`;
  return (
    <Dialog open={open} onClose={onClose} title={title} description={description}>
      <form
        className="dialog-form"
        aria-label={separate ? "Remove account" : signOut ? "Sign out" : "Forget"}
        onSubmit={(event) => {
          event.preventDefault();
          onClose();
          onConfirm(connection, { sign_out: signOut, delete_vault_item: connection.vault_item_id ? vaultToo : false });
          setVaultToo(false);
        }}
      >
        {connection.vault_item_id ? (
          <label className="conn-check">
            <input type="checkbox" checked={vaultToo} onChange={(event) => setVaultToo(event.target.checked)} />
            <span>Also delete the saved login from the Vault</span>
          </label>
        ) : null}
        <DialogActions>
          <Button onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="primary" className="confirm-danger">
            {separate ? "Remove" : signOut ? "Sign out" : "Forget"}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

/** One row per agent; a change applies at once, like the policy select on a Vault item. */
export function AccessDialog({
  open,
  onClose,
  connection,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  connection: Connection | null;
  onChanged: () => void;
}) {
  const client = useClient();
  const team = useTeam();
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const agents = activeAgents(team.agents);

  async function change(connection: Connection, agentId: string, access: ConnectionAccess) {
    setPending(agentId);
    setError(null);
    try {
      await client.updateConnection(connection.id, { grants: [{ agent_id: agentId, access }], home_url: null });
      onChanged();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setPending(null);
    }
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Agent access"
      description={connection ? `Who may use the ${connection.site} sign-in (${identityLine(connection)}). “Ask me first” sends you a request before the agent uses your sign-in; allowing it once covers the page the agent is on until it opens another.` : undefined}
    >
      {connection ? (
        <div className="dialog-form">
          <ul className="resource-list conn-access-list" aria-label="Agent access">
            {agents.map((agent) => (
              <li key={agent.id} className="conn-access-row">
                <Avatar chief={agent.kind === "chief"} initials={agent.initials} color={agent.color} size={32} />
                <span className="conn-access-name">{agent.name}</span>
                <Select
                  variant="inline"
                  aria-label={`${agent.name}'s access to ${connection.site}`}
                  disabled={pending === agent.id}
                  value={accessOf(connection, agent)}
                  onChange={(value) => void change(connection, agent.id, value as ConnectionAccess)}
                  options={ACCESS_OPTIONS}
                />
              </li>
            ))}
          </ul>
          {connection.account_space === null && connection.accounts_on_site > 1 ? (
            <p className="conn-note">
              {connection.site} has more than one account in the agent browser, and an agent cannot tell them apart there. So every account must allow it: one “Blocked” blocks the site, and “Ask me first” on any of them asks you.
            </p>
          ) : null}
          {error ? <ErrorNote>{error}</ErrorNote> : null}
          <DialogActions>
            <Button variant="primary" onClick={onClose}>
              Done
            </Button>
          </DialogActions>
        </div>
      ) : null}
    </Dialog>
  );
}

const COSTS = [
  "Uses about 200–400 MB of memory while it is open, and closes after 10 minutes unused.",
  "Takes a few seconds to start after sitting idle, and about 50–150 MB on disk.",
  "You sign in once there, including any second step, and again whenever it expires.",
];

/** A separate browser profile for one more account on a site that has no account switcher. */
export function AddAccountDialog({
  open,
  onClose,
  site,
  sites,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  /** Prefilled when opened from a card; empty when opened from the section header. */
  site: string;
  sites: string[];
  onCreated: (result: ReloginResult, site: string) => void;
}) {
  const client = useClient();
  const [typed, setTyped] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const value = typed ?? site;
  const clean = value.trim().toLowerCase();

  function close() {
    setTyped(null);
    setError(null);
    onClose();
  }

  async function create(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const created = await client.createAccountSpace(clean);
      setTyped(null);
      onCreated(created.result, clean);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onClose={close}
      title="Add another account"
      description="Most sites let you switch accounts from their own menu, and agents use that. For a site that does not, openside can keep one more account in a separate browser profile."
    >
      <form className="dialog-form" onSubmit={(event) => void create(event)} aria-label="Add another account">
        <label className="field">
          <span>Site</span>
          <input
            inputMode="url"
            list="conn-known-sites"
            placeholder="example.com"
            value={value}
            onChange={(event) => setTyped(event.target.value)}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            required
          />
          <datalist id="conn-known-sites">
            {sites.map((known) => (
              <option key={known} value={known} />
            ))}
          </datalist>
        </label>
        <ul className="conn-costs" aria-label="What a separate profile costs">
          {COSTS.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button onClick={close}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy || clean.length === 0}>
            Create and sign in
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}
