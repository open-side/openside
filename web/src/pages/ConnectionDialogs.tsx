import { useState, type FormEvent } from "react";
import type { Connection, ConnectionAccess, ReloginResult, RevokeConnectionRequest } from "../api/types";
import { Avatar } from "../components/Avatar";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { errorMessage } from "../hooks/useResource";
import { t, type MessageKey } from "../i18n";
import { Button, Dialog, DialogActions, Select } from "../ui";
import { ErrorNote } from "./blocks";
import { accessOf, accessOptions, activeAgents, identityLine } from "./connectionsView";

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
  const site = { site: connection.site };
  const description = t(
    separate ? "connections.revoke.separateDescription" : signOut ? "connections.revoke.signOutDescription" : "connections.revoke.forgetDescription",
    site,
  );
  const title = t(separate ? "connections.revoke.separateTitle" : signOut ? "connections.revoke.signOutTitle" : "connections.revoke.forgetTitle", site);
  return (
    <Dialog open={open} onClose={onClose} title={title} description={description}>
      <form
        className="dialog-form"
        aria-label={t(separate ? "connections.revoke.removeAccount" : signOut ? "connections.revoke.signOut" : "connections.revoke.forget")}
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
            <span>{t("connections.revoke.deleteVaultToo")}</span>
          </label>
        ) : null}
        <DialogActions>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button type="submit" variant="primary" className="confirm-danger">
            {t(separate ? "common.remove" : signOut ? "connections.revoke.signOut" : "connections.revoke.forget")}
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
      title={t("connections.accessDialog.title")}
      description={connection ? t("connections.accessDialog.description", { site: connection.site, identity: identityLine(connection) }) : undefined}
    >
      {connection ? (
        <div className="dialog-form">
          <ul className="resource-list conn-access-list" aria-label={t("connections.accessDialog.title")}>
            {agents.map((agent) => (
              <li key={agent.id} className="conn-access-row">
                <Avatar chief={agent.kind === "chief"} initials={agent.initials} color={agent.color} size={32} />
                <span className="conn-access-name">{agent.name}</span>
                <Select
                  variant="inline"
                  aria-label={t("connections.accessDialog.selectLabel", { agent: agent.name, site: connection.site })}
                  disabled={pending === agent.id}
                  value={accessOf(connection, agent)}
                  onChange={(value) => void change(connection, agent.id, value as ConnectionAccess)}
                  options={accessOptions()}
                />
              </li>
            ))}
          </ul>
          {connection.account_space === null && connection.accounts_on_site > 1 ? (
            <p className="conn-note">
              {t("connections.accessDialog.sharedNote", { site: connection.site })}
            </p>
          ) : null}
          {error ? <ErrorNote>{error}</ErrorNote> : null}
          <DialogActions>
            <Button variant="primary" onClick={onClose}>
              {t("common.done")}
            </Button>
          </DialogActions>
        </div>
      ) : null}
    </Dialog>
  );
}

const COSTS: MessageKey[] = ["connections.addAccount.cost.memory", "connections.addAccount.cost.start", "connections.addAccount.cost.signIn"];

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
      title={t("connections.addAccount.title")}
      description={t("connections.addAccount.description")}
    >
      <form className="dialog-form" onSubmit={(event) => void create(event)} aria-label={t("connections.addAccount.title")}>
        <label className="field">
          <span>{t("connections.addAccount.site")}</span>
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
        <ul className="conn-costs" aria-label={t("connections.addAccount.costsLabel")}>
          {COSTS.map((line) => (
            <li key={line}>{t(line)}</li>
          ))}
        </ul>
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button onClick={close}>{t("common.cancel")}</Button>
          <Button type="submit" variant="primary" disabled={busy || clean.length === 0}>
            {t("connections.addAccount.create")}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}
