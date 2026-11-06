import { Ellipsis, Plus } from "lucide-react";
import { Fragment, useCallback, useState } from "react";
import type { ModelAccountRef, ProviderConnection } from "../api/types";
import type { Client } from "../api/client";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { t } from "../i18n";
import { ResourceView, useConfirm } from "../pages/blocks";
import { ICON_STROKE, IconButton, MenuItem, MenuSeparator, PopoverMenu, SettingsCard, SettingsRow, useToast } from "../ui";
import { OAUTH_SOURCE, SUBSCRIPTION_SOURCES } from "./accounts";
import { ConnectDialog, ReplaceKeyDialog, SIGN_IN_PROVIDERS, type ConnectStart } from "./ConnectDialog";
import { ReloginDialog } from "./OAuthDialogs";
import { ProviderGlyph } from "./ProviderGlyph";
import { KEY_PROVIDERS, providerLabel } from "./providers";
import "./models.css";

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** What Reconnect does for a row, from the accounts behind it. */
type Reconnect =
  | { kind: "relogin"; provider: string; accountId: string; name: string }
  | { kind: "replace-key"; provider: string; keyId: string; name: string }
  | { kind: "connect"; start: ConnectStart }
  | null;

function reconnectOf(row: ProviderConnection): Reconnect {
  if (row.kind === "api_key") {
    const key = row.accounts.find((ref) => ref.source === "vault");
    if (key) return { kind: "replace-key", provider: row.provider, keyId: key.account_id, name: row.display_name };
    // A key read from the environment or a command lives outside openside; a saved key takes over.
    return (KEY_PROVIDERS as readonly string[]).includes(row.provider) ? { kind: "connect", start: { kind: "api_key", provider: row.provider } } : null;
  }
  const signedIn = row.accounts.find((ref) => ref.source === OAUTH_SOURCE);
  if (signedIn) return { kind: "relogin", provider: row.provider, accountId: signedIn.account_id, name: row.display_name };
  // A terminal sign-in (Claude Code, Codex CLI) is replaced by openside's own sign-in to the same provider.
  return SIGN_IN_PROVIDERS.has(row.provider) ? { kind: "connect", start: { kind: "subscription", provider: row.provider, name: row.display_name } } : null;
}

const SUBSCRIPTION = new Set<string>(SUBSCRIPTION_SOURCES);

/** Deletes one account; false for accounts configured outside the app (environment, commands, config.toml). */
async function removeAccount(client: Client, provider: string, ref: ModelAccountRef): Promise<boolean> {
  if (ref.source === OAUTH_SOURCE) await client.deleteOAuthAccount(provider, ref.account_id);
  else if (ref.source === "vault") await client.deleteApiKey(provider, ref.account_id);
  else if (SUBSCRIPTION.has(ref.source)) await client.deleteSubscriptionAccount(ref.source, ref.account_id);
  else return false;
  return true;
}

/** Sign-ins that belong to one company share its row: a ChatGPT sign-in sits under OpenAI. */
const COMPANY: Record<string, string> = {
  "openai-codex": "openai",
  "google-gla": "google",
  "google-vertex": "google",
  "google-gemini-cli": "google",
  "google-antigravity": "google",
};

interface ProviderGroup {
  key: string;
  name: string;
  connections: ProviderConnection[];
}

/** One row per company, in the daemon's order of first appearance (Aside lists one row per provider). */
function groupProviders(connections: ProviderConnection[]): ProviderGroup[] {
  const groups = new Map<string, ProviderGroup>();
  for (const connection of connections) {
    const key = COMPANY[connection.provider] ?? connection.provider;
    const group = groups.get(key) ?? { key, name: providerLabel(key), connections: [] };
    group.connections.push(connection);
    groups.set(key, group);
  }
  return [...groups.values()];
}

/**
 * Settings › Models › Providers (Aside `L 48`): one row per provider with its connections as the
 * subtitle and a "…" menu that reconnects or removes each of them, then "+ Connect", which opens the
 * picker.
 */
export function ProvidersCard({ onChanged }: { onChanged?: () => void }) {
  const client = useClient();
  const toast = useToast();
  const providers = useResource(useCallback(() => client.modelProviders(), [client]));
  const [connect, setConnect] = useState<{ start?: ConnectStart } | null>(null);
  const [relogin, setRelogin] = useState<Extract<Reconnect, { kind: "relogin" }> | null>(null);
  const [replace, setReplace] = useState<Extract<Reconnect, { kind: "replace-key" }> | null>(null);
  const [confirm, ask] = useConfirm();
  const { reload } = providers;

  const refresh = useCallback(async () => {
    await reload();
    onChanged?.();
  }, [reload, onChanged]);

  function reconnect(connection: ProviderConnection) {
    const action = reconnectOf(connection);
    if (action?.kind === "relogin") setRelogin(action);
    else if (action?.kind === "replace-key") setReplace(action);
    else if (action?.kind === "connect") setConnect({ start: action.start });
  }

  async function disconnect(connection: ProviderConnection, name: string) {
    let kept = 0;
    try {
      for (const ref of connection.accounts) if (!(await removeAccount(client, connection.provider, ref))) kept++;
      const done = connection.kind === "api_key" ? t("models.providers.keyRemoved", { name }) : t("models.providers.disconnected", { name });
      toast.notify(kept ? t("models.providers.disconnectedPartly", { name }) : done);
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    }
    await refresh();
  }

  function connectionMenu(group: ProviderGroup, connection: ProviderConnection) {
    const removable = connection.accounts.some((ref) => ref.source === OAUTH_SOURCE || ref.source === "vault" || SUBSCRIPTION.has(ref.source));
    const key = connection.kind === "api_key";
    return (
      <>
        <MenuItem disabled={reconnectOf(connection) === null} onClick={() => reconnect(connection)}>
          {key ? t("models.providers.replaceKey") : t("models.providers.reconnectNamed", { name: connection.display_name })}
        </MenuItem>
        <MenuItem
          danger
          disabled={!removable}
          onClick={() =>
            ask(
              key
                ? {
                    title: t("models.providers.removeKeyTitle", { name: group.name }),
                    description: t("models.providers.disconnectKey"),
                    confirmLabel: t("common.remove"),
                    onConfirm: () => void disconnect(connection, group.name),
                  }
                : {
                    title: t("models.providers.disconnectTitle", { name: connection.display_name }),
                    description: t("models.providers.disconnectSubscription"),
                    confirmLabel: t("models.providers.disconnect"),
                    onConfirm: () => void disconnect(connection, connection.display_name),
                  },
            )
          }
        >
          {key ? t("models.providers.removeKey") : t("models.providers.disconnectNamed", { name: connection.display_name })}
        </MenuItem>
      </>
    );
  }

  return (
    <>
      <ResourceView resource={providers} what={t("models.scope.models")}>
        {(data) => {
          const groups = groupProviders(data.providers);
          return (
            <SettingsCard className="models-providers">
              {groups.length ? (
                <ul className="models-provider-list" aria-label={t("models.providers.title")}>
                  {groups.map((group) => {
                    const label = t("models.providers.options", { name: group.name });
                    const connected = group.connections.some((connection) => connection.status === "connected");
                    const status = connected ? t("models.providers.connected") : t("models.providers.needsAttention");
                    const parts = group.connections.map((connection) => {
                      const name = connection.kind === "api_key" ? t("models.providers.apiKey") : connection.display_name;
                      return connection.status === "connected" ? name : t("models.providers.partNeedsAttention", { name });
                    });
                    return (
                      <li key={group.key} data-provider={group.key}>
                        <SettingsRow
                          leading={<ProviderGlyph provider={group.key} size={18} />}
                          title={group.name}
                          description={parts.join(", ")}
                          control={
                            <span className="row-actions">
                              <span className="models-status-mark" data-status={connected ? "connected" : "needs_attention"} role="img" aria-label={status} title={status} />
                              <PopoverMenu
                                label={label}
                                trigger={
                                  <IconButton label={label} size="small" className="models-row-more">
                                    <Ellipsis {...ICON} />
                                  </IconButton>
                                }
                              >
                                {group.connections.map((connection, index) => (
                                  <Fragment key={connection.id}>
                                    {index > 0 ? <MenuSeparator /> : null}
                                    {connectionMenu(group, connection)}
                                  </Fragment>
                                ))}
                              </PopoverMenu>
                            </span>
                          }
                        />
                      </li>
                    );
                  })}
                </ul>
              ) : null}
              <button type="button" className="ui-row ui-row--single models-connect-row" onClick={() => setConnect({})}>
                <Plus {...ICON} />
                <span>{t("models.providers.connect")}</span>
              </button>
            </SettingsCard>
          );
        }}
      </ResourceView>
      {connect ? <ConnectDialog open start={connect.start} onClose={() => setConnect(null)} onConnected={() => void refresh()} /> : null}
      {relogin ? (
        <ReloginDialog open provider={relogin.provider} accountId={relogin.accountId} name={relogin.name} onClose={() => setRelogin(null)} onReady={() => void refresh()} />
      ) : null}
      {replace ? <ReplaceKeyDialog open provider={replace.provider} keyId={replace.keyId} name={replace.name} onClose={() => setReplace(null)} onSaved={() => void refresh()} /> : null}
      {confirm}
    </>
  );
}
