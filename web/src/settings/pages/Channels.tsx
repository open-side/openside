import { ChevronDown, FlaskConical, Pencil, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useId, useState } from "react";
import type { AgentSummary, ChannelAccount, ChannelAccountTest } from "../../api/types";
import { mayUse } from "../../auth";
import { Avatar } from "../../components/Avatar";
import { AccountDialog } from "../../channels/AccountDialog";
import { isLegacy, SERVICES, serviceOf, statusView, testSummary, type ChannelKind, type Service } from "../../channels/services";
import { WebhookUrl } from "../../channels/WebhookUrl";
import { useClient } from "../../context";
import { CHIEF_ID, useTeam } from "../../hooks/useAgents";
import { errorMessage, useResource } from "../../hooks/useResource";
import { formatList, t } from "../../i18n";
import { tNodes } from "../../i18n/react";
import { Badge, ErrorNote, ResourceView, RowMenu, useConfirm, useDialogState } from "../../pages/blocks";
import { RestartNotice } from "../../pages/RestartNotice";
import { Button, HeroCard, IconButton, ICON_STROKE, InfoBanner, MenuItem, PopoverMenu, SettingsCard, SettingsRow, SettingsSection, Spinner, Switch, useToast } from "../../ui";
import "./personal.css";
import "./channels.css";

/** openside's own picture: a question from a phone chat and openside's reply, on a dusk gradient. */
function ChatBubbles() {
  const id = useId();
  const dusk = `${id}-dusk`;
  const glow = `${id}-glow`;
  return (
    <svg viewBox="0 0 288 165" role="presentation" preserveAspectRatio="xMidYMid slice">
      <defs>
        <linearGradient id={dusk} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#6366f1" />
          <stop offset="0.55" stopColor="#a855f7" />
          <stop offset="1" stopColor="#f472b6" />
        </linearGradient>
        <radialGradient id={glow} cx="0.75" cy="0.2" r="0.7">
          <stop offset="0" stopColor="#ffffff" stopOpacity="0.45" />
          <stop offset="1" stopColor="#ffffff" stopOpacity="0" />
        </radialGradient>
      </defs>
      <rect width="288" height="165" fill={`url(#${dusk})`} />
      <rect width="288" height="165" fill={`url(#${glow})`} />
      <g>
        <circle cx="40" cy="44" r="11" fill="#fde68a" />
        <rect x="58" y="24" width="150" height="46" rx="14" fill="#ffffff" />
        <path d="M58 52 l-8 10 l14 -4 z" fill="#ffffff" />
        <rect x="70" y="35" width="44" height="5" rx="2.5" fill="#6366f1" />
        <rect x="70" y="46" width="124" height="4" rx="2" fill="#cbd5e1" />
        <rect x="70" y="55" width="96" height="4" rx="2" fill="#cbd5e1" />
      </g>
      <g>
        <rect x="80" y="86" width="150" height="54" rx="14" fill="#1e1b4b" fillOpacity="0.9" />
        <path d="M230 120 l8 10 l-14 -4 z" fill="#1e1b4b" fillOpacity="0.9" />
        <circle cx="250" cy="128" r="11" fill="#ffffff" />
        <path d="M245 128 h10 M250 123 v10" stroke="#a855f7" strokeWidth="2" strokeLinecap="round" />
        <rect x="92" y="98" width="38" height="5" rx="2.5" fill="#c4b5fd" />
        <rect x="92" y="109" width="124" height="4" rx="2" fill="#ffffff" fillOpacity="0.55" />
        <rect x="92" y="118" width="110" height="4" rx="2" fill="#ffffff" fillOpacity="0.55" />
        <g fill="#ffffff" fillOpacity="0.8">
          <circle cx="96" cy="130" r="2" />
          <circle cx="103" cy="130" r="2" />
          <circle cx="110" cy="130" r="2" />
        </g>
      </g>
    </svg>
  );
}

/** "Answers as Mina", or who will once the account connects. */
function routing(account: ChannelAccount, agents: readonly AgentSummary[] | null) {
  const ownerId = account.effective_agent_id ?? account.agent_id ?? CHIEF_ID;
  const owner = agents?.find((agent) => agent.id === ownerId);
  const name = owner?.name ?? (ownerId === CHIEF_ID ? t("models.channels.yourAssistant") : ownerId);
  const retired = owner?.status === "retired";
  const key = account.effective_agent_id
    ? retired
      ? "models.channels.answersAsRetired"
      : "models.channels.answersAs"
    : retired
      ? "models.channels.willAnswerAsRetired"
      : "models.channels.willAnswerAs";
  return { owner, text: tNodes(key, { name: <strong>{name}</strong> }) };
}

interface AccountRowProps {
  account: ChannelAccount;
  agents: readonly AgentSummary[] | null;
  readOnly: boolean;
  busy: boolean;
  /** Remote Control is off, so Slack and Discord cannot post to this computer. */
  remoteOff: boolean;
  result: { ok: boolean; text: string } | undefined;
  onToggle: (enabled: boolean) => void;
  onEdit: () => void;
  onMove: () => void;
  onTest: () => void;
  onRemove: () => void;
}

/**
 * One account under its platform: who answers, its switch, Edit (Reconnect once its credentials
 * expired) and ⋯ (Test, Edit, Remove); the callback address, legacy hint and last test result
 * sit under the row.
 */
function AccountRow({ account, agents, readOnly, busy, remoteOff, result, onToggle, onEdit, onMove, onTest, onRemove }: AccountRowProps) {
  const status = statusView(account.status);
  const legacy = isLegacy(account);
  const route = routing(account, agents);
  const expired = account.status === "expired";
  const id = legacy ? tNodes("models.channels.fromConfig", { id: <code>{account.id}</code> }) : <code>{account.id}</code>;
  return (
    <div className="channel-account" data-account={account.id} data-kind={account.kind}>
      <SettingsRow
        leading={route.owner ? <Avatar chief={route.owner.kind === "chief"} initials={route.owner.initials} color={route.owner.color} size={24} /> : undefined}
        title={account.label}
        badge={<Badge tone={status.tone}>{status.text}</Badge>}
        description={
          <>
            <span className="channel-account-route">{route.text}</span>
            <span className="channel-account-id">{id}</span>
          </>
        }
        control={
          readOnly ? undefined : (
            <>
              <Switch checked={account.enabled} disabled={busy} label={t("models.channels.turnOnNamed", { name: account.label })} onChange={onToggle} />
              <Button
                variant="ghost"
                size="small"
                disabled={busy}
                aria-label={expired ? t("models.channels.reconnectNamed", { name: account.label }) : t("models.channels.editNamed", { name: account.label })}
                onClick={onEdit}
              >
                {expired ? t("models.channels.reconnect") : t("common.edit")}
              </Button>
              <RowMenu name={account.label}>
                <MenuItem icon={<FlaskConical size={16} strokeWidth={ICON_STROKE} />} disabled={busy || !account.secret_present} onClick={onTest}>
                  {t("models.channels.test")}
                </MenuItem>
                <MenuItem icon={<Pencil size={16} strokeWidth={ICON_STROKE} />} onClick={onEdit}>
                  {t("models.channels.editAccount")}
                </MenuItem>
                <MenuItem danger icon={<Trash2 size={16} strokeWidth={ICON_STROKE} />} onClick={onRemove}>
                  {t("common.remove")}
                </MenuItem>
              </RowMenu>
            </>
          )
        }
      />
      <AccountDetail account={account} legacy={legacy} readOnly={readOnly} busy={busy} remoteOff={remoteOff} result={result} onMove={onMove} />
    </div>
  );
}

function AccountDetail({ account, legacy, readOnly, busy, remoteOff, result, onMove }: { account: ChannelAccount; legacy: boolean; readOnly: boolean; busy: boolean; remoteOff: boolean; result: { ok: boolean; text: string } | undefined; onMove: () => void }) {
  const callback = Boolean((account.webhook_url || (remoteOff && !legacy)) && serviceOf(account.kind).callback);
  if (!callback && !legacy && !busy && !result) return null;
  return (
    <div className="channel-account-detail">
      <WebhookUrl account={account} remoteOff={remoteOff && !legacy} />
      {legacy ? (
        <p className="channel-note" data-legacy>
          {t("models.channels.legacy")}
          {readOnly ? (
            ` ${t("models.channels.legacyReadOnly")}`
          ) : (
            <>
              {" "}
              {t("models.channels.legacyMove")}{" "}
              <Button variant="ghost" disabled={busy} onClick={onMove}>
                {t("models.channels.moveToVault")}
              </Button>
            </>
          )}
        </p>
      ) : null}
      {busy ? (
        <p className="channel-result" role="status">
          <Spinner size={14} /> {t("models.channels.working")}
        </p>
      ) : result ? (
        <p className={result.ok ? "channel-result" : "channel-result channel-result--bad"} role="status">
          {result.text}
        </p>
      ) : null}
    </div>
  );
}

/** The platform row's second line: each account with its state, or what the platform is for. */
function platformLine(service: Service, accounts: readonly ChannelAccount[]): string {
  if (accounts.length === 0) return t(service.tagline);
  return formatList(accounts.map((account) => `${account.label} · ${statusView(account.status).text}`));
}

/**
 * Settings › Channels (Aside's layout): the hero, then one "Personal Assistants" card with a row
 * per platform. Each platform row's + opens the add dialog for that service, and its accounts
 * follow it as their own rows with a switch, Edit and ⋯. Credentials are only ever typed into
 * the account dialog; nothing here can read them back.
 */
export function ChannelsSettings() {
  const client = useClient();
  const toast = useToast();
  const team = useTeam();
  const accounts = useResource(useCallback(() => client.channelAccounts(), [client]));
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [results, setResults] = useState<Record<string, { ok: boolean; text: string }>>({});
  const [remoteOff, setRemoteOff] = useState(false);
  useEffect(() => {
    let alive = true;
    client.remoteStatus().then(
      (status) => alive && setRemoteOff(!status.enabled),
      // Unknown counts as on: the daemon's own webhook address is then the only hint.
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, [client]);
  const dialog = useDialogState<{ kind: ChannelKind; account: ChannelAccount | null; replaceCredentials: boolean }>();
  const [confirmElement, ask] = useConfirm();
  // Mutations need Admin on a person's device; a pasted token's scopes are unknown, so the daemon decides.
  const readOnly = mayUse("admin") === false;
  const reload = accounts.reload;
  const keyOf = (account: ChannelAccount) => `${account.kind}:${account.id}`;
  const add = (kind: ChannelKind) => dialog.show({ kind, account: null, replaceCredentials: true });

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
      title: t("models.channels.removeTitle", { name: account.label }),
      description: t("models.channels.removeDescription", { service: serviceOf(account.kind).name }),
      confirmLabel: t("common.remove"),
      onConfirm: () =>
        void run(keyOf(account), async () => {
          await client.deleteChannelAccount(account.kind, account.id);
          toast.notify(t("models.channels.removed", { name: account.label }));
          await reload();
        }),
    });

  const saved = async (message: string) => {
    toast.notify(message);
    await reload();
  };

  return (
    <div className="personal-page channels-page" data-section="channels">
      <HeroCard
        title={t("models.channels.hero.title")}
        description={t("models.channels.hero.description")}
        action={
          readOnly ? undefined : (
            <PopoverMenu
              label={t("models.channels.addAccount")}
              align="start"
              trigger={
                <Button variant="primary">
                  {t("models.channels.addAccount")}
                  <ChevronDown size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                </Button>
              }
            >
              {SERVICES.map((service) => (
                <MenuItem key={service.kind} icon={<service.Icon size={16} strokeWidth={ICON_STROKE} />} onClick={() => add(service.kind)}>
                  {service.name}
                </MenuItem>
              ))}
            </PopoverMenu>
          )
        }
        illustration={<ChatBubbles />}
      />
      <SettingsSection label={t("models.channels.section")}>
        <ResourceView resource={accounts} what={t("models.scope.channels")}>
          {(data) => (
            <>
              {data.accounts.some((account) => account.restart_required) ? (
                <RestartNotice onRestarted={() => void reload()}>{t("models.channels.restartNeeded")}</RestartNotice>
              ) : null}
              {error ? <ErrorNote>{error}</ErrorNote> : null}
              <SettingsCard className="channels-card">
                {readOnly ? <InfoBanner>{t("models.channels.readOnly")}</InfoBanner> : null}
                {data.accounts.some((account) => account.status === "vault_locked") ? <InfoBanner>{t("models.channels.vaultLocked")}</InfoBanner> : null}
                {SERVICES.map((service) => {
                  const rows = data.accounts.filter((account) => account.kind === service.kind);
                  return (
                    <section key={service.kind} className="channel-platform" aria-label={t("models.channels.accounts", { service: service.name })}>
                      <SettingsRow
                        leading={
                          <span className="channel-logo" data-kind={service.kind}>
                            <service.Icon size={16} strokeWidth={2} />
                          </span>
                        }
                        title={service.name}
                        description={platformLine(service, rows)}
                        control={
                          readOnly ? undefined : (
                            <IconButton label={t("models.channels.form.addTitle", { service: service.name })} className="channel-add" onClick={() => add(service.kind)}>
                              <Plus size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                            </IconButton>
                          )
                        }
                      />
                      {rows.map((account) => (
                        <AccountRow
                          key={account.id}
                          account={account}
                          agents={team.agents}
                          readOnly={readOnly}
                          busy={busy === keyOf(account)}
                          remoteOff={remoteOff}
                          result={results[keyOf(account)]}
                          onToggle={(enabled) => void toggle(account, enabled)}
                          onEdit={() => dialog.show({ kind: account.kind as ChannelKind, account, replaceCredentials: false })}
                          onMove={() => dialog.show({ kind: account.kind as ChannelKind, account, replaceCredentials: true })}
                          onTest={() => void test(account)}
                          onRemove={() => remove(account)}
                        />
                      ))}
                    </section>
                  );
                })}
              </SettingsCard>
            </>
          )}
        </ResourceView>
      </SettingsSection>
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
        />
      ) : null}
      {confirmElement}
    </div>
  );
}
