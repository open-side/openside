import { CircleAlert, FlaskConical, Lock, Pencil, Trash2 } from "lucide-react";
import { useCallback, useState } from "react";
import type { AgentSummary, ChannelAccount, ChannelAccountTest } from "../api/types";
import { mayUse } from "../auth";
import { Avatar } from "../components/Avatar";
import { useClient } from "../context";
import { CHIEF_ID, useTeam } from "../hooks/useAgents";
import { errorMessage, useResource } from "../hooks/useResource";
import { AddButton, ErrorNote, ResourceCard, ResourceView, RowMenu, SectionList, useConfirm, useDialogState } from "../pages/blocks";
import { RestartNotice } from "../pages/RestartNotice";
import { Button, ICON_STROKE, MenuItem, Spinner, Switch, useToast } from "../ui";
import { AccountDialog } from "./AccountDialog";
import { isLegacy, SERVICES, serviceOf, statusView, testSummary, type ChannelKind } from "./services";
import { WebhookUrl } from "./WebhookUrl";
import "./channels.css";

/** Who answers on this account: the live owner, or who will once it connects. */
function Routing({ account, agents }: { account: ChannelAccount; agents: readonly AgentSummary[] | null }) {
  const find = (id: string) => agents?.find((agent) => agent.id === id);
  const ownerId = account.effective_agent_id ?? account.agent_id ?? CHIEF_ID;
  const owner = find(ownerId);
  const name = owner?.name ?? (ownerId === CHIEF_ID ? "Your assistant" : ownerId);
  return (
    <p className="channel-route">
      {owner ? <Avatar chief={owner.kind === "chief"} initials={owner.initials} color={owner.color} size={24} /> : null}
      <span>
        {account.effective_agent_id ? "Answers as " : "Will answer as "}
        <strong>{name}</strong>
        {owner?.status === "retired" ? " (retired)" : ""}
        {account.effective_agent_id ? "" : " once it connects"}
      </span>
    </p>
  );
}

function AccountRow({
  account,
  agents,
  readOnly,
  busy,
  result,
  onToggle,
  onEdit,
  onMove,
  onTest,
  onRemove,
  onRemote,
}: {
  account: ChannelAccount;
  agents: readonly AgentSummary[] | null;
  readOnly: boolean;
  busy: boolean;
  result: { ok: boolean; text: string } | undefined;
  onToggle: (enabled: boolean) => void;
  onEdit: () => void;
  onMove: () => void;
  onTest: () => void;
  onRemove: () => void;
  onRemote: () => void;
}) {
  const service = serviceOf(account.kind);
  const status = statusView(account.status);
  const legacy = isLegacy(account);
  return (
    <ResourceCard
      icon={service.Icon}
      data-account={account.id}
      data-kind={account.kind}
      title={account.label}
      badge={status.text}
      badgeTone={status.tone}
      detail={
        <>
          <code>{account.id}</code>
          {legacy ? " · from config.toml" : ""}
        </>
      }
      trailing={
        readOnly ? undefined : (
          <span className="row-actions">
            <Switch checked={account.enabled} disabled={busy} label={`Turn on ${account.label}`} onChange={onToggle} />
            <Button size="compact" variant="ghost" disabled={busy} aria-label={`${account.status === "expired" ? "Reconnect" : "Edit"} ${account.label}`} onClick={onEdit}>
              {account.status === "expired" ? "Reconnect" : "Edit"}
            </Button>
            <RowMenu name={account.label}>
              <MenuItem icon={<FlaskConical size={18} strokeWidth={ICON_STROKE} />} disabled={busy || !account.secret_present} onClick={onTest}>
                Test connection
              </MenuItem>
              <MenuItem icon={<Pencil size={18} strokeWidth={ICON_STROKE} />} onClick={onEdit}>
                Edit account
              </MenuItem>
              <MenuItem danger icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />} onClick={onRemove}>
                Remove
              </MenuItem>
            </RowMenu>
          </span>
        )
      }
    >
      <Routing account={account} agents={agents} />
      <WebhookUrl account={account} onRemote={onRemote} />
      {legacy ? (
        <p className="channel-note" data-legacy>
          This account still reads its credentials from config.toml.
          {readOnly ? (
            " Move them into the vault from an admin device to manage it here."
          ) : (
            <>
              {" "}
              Move them into the vault to manage and rotate them here; its conversations are kept.{" "}
              <Button variant="ghost" size="compact" disabled={busy} onClick={onMove}>
                Move to vault
              </Button>
            </>
          )}
        </p>
      ) : null}
      {busy ? (
        <p className="channel-result" role="status">
          <Spinner size={14} /> Working…
        </p>
      ) : result ? (
        <p className={result.ok ? "channel-result" : "channel-result channel-result--bad"} role="status">
          {result.text}
        </p>
      ) : null}
    </ResourceCard>
  );
}

/**
 * Every Telegram, Slack and Discord account, each with its own switch, teammate and status.
 * Credentials are only ever typed into the account dialog; nothing here can read them back.
 */
export function ChannelsPanel({ onRemote }: { onRemote: () => void }) {
  const client = useClient();
  const toast = useToast();
  const team = useTeam();
  const accounts = useResource(useCallback(() => client.channelAccounts(), [client]));
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [results, setResults] = useState<Record<string, { ok: boolean; text: string }>>({});
  const dialog = useDialogState<{ kind: ChannelKind; account: ChannelAccount | null; replaceCredentials: boolean }>();
  const [confirmElement, ask] = useConfirm();
  // Mutations need Admin on a person's device; a pasted token's scopes are unknown, so the daemon decides.
  const readOnly = mayUse("admin") === false;
  const reload = accounts.reload;
  const keyOf = (account: ChannelAccount) => `${account.kind}:${account.id}`;

  async function run(key: string, action: () => Promise<void>) {
    setBusy(key);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  const toggle = (account: ChannelAccount, enabled: boolean) =>
    run(keyOf(account), async () => {
      await client.updateChannelAccount(account.kind, account.id, { enabled });
      await reload();
    });

  const test = (account: ChannelAccount) =>
    run(keyOf(account), async () => {
      const outcome: ChannelAccountTest = await client.testChannelAccount(account.kind, account.id);
      setResults((current) => ({ ...current, [keyOf(account)]: { ok: outcome.ok, text: testSummary(outcome) } }));
      await reload();
    });

  const remove = (account: ChannelAccount) =>
    ask({
      title: `Remove ${account.label}?`,
      description: `Only this ${serviceOf(account.kind).name} account stops, and its encrypted credentials are deleted from the vault. Your other accounts stay connected. Conversations and history stay, but no new messages arrive through it. This does not revoke the bot with ${serviceOf(account.kind).name}.`,
      confirmLabel: "Remove",
      onConfirm: () =>
        void run(keyOf(account), async () => {
          await client.deleteChannelAccount(account.kind, account.id);
          toast.notify(`${account.label} removed.`);
          await reload();
        }),
    });

  const saved = async (message: string) => {
    toast.notify(message);
    await reload();
  };

  return (
    <div className="manage-body" data-section="channels">
      <ResourceView resource={accounts} what="channels">
        {(data) => (
          <>
            {readOnly ? (
              <div className="models-callout models-callout--info" role="note">
                <Lock size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                <p>This device can see the accounts but cannot change them. Manage channels on the computer running openside or from an admin device.</p>
              </div>
            ) : null}
            {data.accounts.some((account) => account.status === "vault_locked") ? (
              <div className="models-callout" role="status">
                <CircleAlert size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                <p>The vault is locked, so these accounts are paused. Unlock it under Workspace → Vault and they reconnect on their own.</p>
              </div>
            ) : null}
            {data.accounts.some((account) => account.restart_required) ? (
              <RestartNotice onRestarted={() => void reload()}>Some accounts need openside to restart before they can connect.</RestartNotice>
            ) : null}
            {error ? <ErrorNote>{error}</ErrorNote> : null}
            {SERVICES.map((service) => {
              const rows = data.accounts.filter((account) => account.kind === service.kind);
              return (
                <SectionList
                  key={service.kind}
                  title={service.name}
                  description={service.blurb}
                  actions={
                    readOnly ? undefined : (
                      <AddButton variant="secondary" onClick={() => dialog.show({ kind: service.kind, account: null, replaceCredentials: true })}>
                        Add account
                      </AddButton>
                    )
                  }
                >
                  {rows.length ? (
                    <ul className="resource-list" aria-label={`${service.name} accounts`}>
                      {rows.map((account) => (
                        <li key={account.id}>
                          <AccountRow
                            account={account}
                            agents={team.agents}
                            readOnly={readOnly}
                            busy={busy === keyOf(account)}
                            result={results[keyOf(account)]}
                            onToggle={(enabled) => void toggle(account, enabled)}
                            onEdit={() => dialog.show({ kind: account.kind as ChannelKind, account, replaceCredentials: false })}
                            onMove={() => dialog.show({ kind: account.kind as ChannelKind, account, replaceCredentials: true })}
                            onTest={() => void test(account)}
                            onRemove={() => remove(account)}
                            onRemote={onRemote}
                          />
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="channel-empty">No {service.name} accounts yet.</p>
                  )}
                </SectionList>
              );
            })}
          </>
        )}
      </ResourceView>
      {dialog.value ? (
        <AccountDialog
          open={dialog.open}
          kind={dialog.value.kind}
          account={dialog.value.account}
          replaceCredentials={dialog.value.replaceCredentials}
          agents={team.agents}
          onClose={dialog.close}
          onSaved={saved}
          onTested={reload}
          onRemote={onRemote}
        />
      ) : null}
      {confirmElement}
    </div>
  );
}
