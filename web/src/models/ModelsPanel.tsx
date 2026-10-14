import { CircleAlert, Lock, RefreshCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { ModelAccountRef, ModelCatalog, ModelProvider, ModelRole, ModelTier, SubscriptionAccount } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { ErrorNote, ResourceCard, ResourceView, SectionList, useConfirm, useDialogState } from "../pages/blocks";
import { Button, IconButton, ICON_STROKE, Select, useToast, type SelectGroup } from "../ui";
import { accountsOf, refKey, type AccountRow } from "./accounts";
import { AddKeyDialog, AddSubscriptionDialog, ManageAccountDialog, SignInDialog, type ManageChanges } from "./AccountDialogs";
import { ProviderCard } from "./ProviderCard";
import { anyModelReady, providerLabel } from "./providers";
import { useModelSettings } from "./useModelSettings";
import "./models.css";

const MAX_FALLBACKS = 16;

type CatalogModel = ModelProvider["models"][number];

/** The pick for each role plus the shared fallback chain; `fast` null means "same as the reasoning model". */
interface Roles {
  reasoning: string;
  fast: string | null;
  fallbacks: string[];
}

const SAME_AS_REASONING = "";
const TIER_LABEL: Record<ModelTier, string> = { fast: "Fast", reasoning: "Reasoning", frontier: "Frontier" };

function ModelRoles({ catalog, readOnly, busy, onSave }: { catalog: ModelCatalog; readOnly: boolean; busy: boolean; onSave: (next: Roles) => void }) {
  const reasoning = catalog.default_model ?? null;
  const fast = catalog.fast_model ?? null;
  const find = (id: string | null) => catalog.providers.flatMap((provider) => provider.models).find((model) => model.id === id);
  const known = (id: string | null) => find(id) !== undefined;
  /** The picker's name for a model, so a chip reads the same as the list it was chosen from. */
  const nameOf = (id: string) => find(id)?.label ?? id;
  const taken = new Set([reasoning, ...catalog.fallback_models]);
  const save = (patch: Partial<Roles>) => onSave({ reasoning: reasoning ?? "", fast, fallbacks: catalog.fallback_models, ...patch });

  const option = (model: CatalogModel, provider?: string) => ({
    value: model.id,
    label: model.available ? model.label : `${model.label} (unavailable)`,
    tag: TIER_LABEL[model.tier],
    detail: [provider, model.in_catalog ? model.strengths : "Not in current catalog"].filter(Boolean).join(" · ") || undefined,
    disabled: !model.available,
  });

  /** Models the daemon recommends for `role` come first, labelled with their provider; the rest follow by provider. */
  const groups = (role: ModelRole | null, skip: (id: string) => boolean): SelectGroup[] => {
    const recommended = (model: CatalogModel) => role !== null && model.recommended_for === role;
    const top = catalog.providers.flatMap((provider) =>
      provider.models.filter((model) => recommended(model) && !skip(model.id)).map((model) => option(model, providerLabel(provider.provider))),
    );
    const rest = catalog.providers.flatMap((provider) => {
      const models = provider.models.filter((model) => !recommended(model) && !skip(model.id));
      return models.length ? [{ label: providerLabel(provider.provider), options: models.map((model) => option(model)) }] : [];
    });
    return [...(top.length ? [{ label: "Recommended", options: top }] : []), ...rest];
  };

  const missing = (id: string | null) => (id !== null && !known(id) ? [{ value: id, label: `${id} (not in the catalog)` }] : []);

  return (
    <SectionList title="Models" description="Chosen once here and used for every conversation.">
      <div className="default-model" data-section="default-model">
        <div className="model-role">
          <div className="model-role-copy">
            <label htmlFor="fast-model">Fast model</label>
            <p>Answers quickly and hands bigger work to the reasoning model.</p>
          </div>
          {readOnly ? (
            <span className="default-model-value">{fast ? nameOf(fast) : "Same as reasoning model"}</span>
          ) : (
            <Select
              id="fast-model"
              value={fast ?? SAME_AS_REASONING}
              disabled={busy || reasoning === null}
              options={[{ value: SAME_AS_REASONING, label: "Same as reasoning model" }, ...missing(fast), ...groups("fast", () => false)]}
              onChange={(value) => save({ fast: value === SAME_AS_REASONING ? null : value })}
            />
          )}
        </div>
        <div className="model-role">
          <div className="model-role-copy">
            <label htmlFor="reasoning-model">Reasoning model</label>
            <p>Does the thinking for tasks and teammates.</p>
          </div>
          {readOnly ? (
            <span className="default-model-value">{reasoning ? nameOf(reasoning) : "Not set"}</span>
          ) : (
            <Select
              id="reasoning-model"
              value={reasoning ?? ""}
              disabled={busy}
              placeholder="Choose a model…"
              options={[...missing(reasoning), ...groups("reasoning", () => false)]}
              onChange={(value) => save({ reasoning: value, fast: fast === value ? null : fast, fallbacks: catalog.fallback_models.filter((id) => id !== value) })}
            />
          )}
        </div>
        {catalog.fallback_models.length || !readOnly ? (
          <div className="default-model-row" aria-label="Fallback models" role="group">
            <span>Fallbacks, tried in order</span>
            {catalog.fallback_models.length ? (
              <ul className="fallback-list">
                {catalog.fallback_models.map((id) => (
                  <li key={id} className="fallback-chip">
                    <span title={id}>{nameOf(id)}</span>
                    {readOnly ? null : (
                      <IconButton label={`Remove fallback ${nameOf(id)}`} size="small" disabled={busy} onClick={() => save({ fallbacks: catalog.fallback_models.filter((other) => other !== id) })}>
                        <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                      </IconButton>
                    )}
                  </li>
                ))}
              </ul>
            ) : (
              <span className="default-model-value">None</span>
            )}
          </div>
        ) : null}
        {readOnly || reasoning === null || catalog.fallback_models.length >= MAX_FALLBACKS ? null : (
          <div className="default-model-row">
            <label htmlFor="add-fallback">Add a fallback</label>
            <Select
              id="add-fallback"
              value=""
              disabled={busy}
              placeholder="Choose a model…"
              options={groups(null, (id) => taken.has(id))}
              onChange={(value) => value && save({ fallbacks: [...catalog.fallback_models, value] })}
            />
          </div>
        )}
      </div>
    </SectionList>
  );
}

/**
 * Providers, how each one is connected and which models that gives, plus the fast and reasoning models.
 * Shared by the Models & accounts dialog and the first-run step (`compact`).
 */
export function ModelsPanel({ compact = false, onCatalog }: { compact?: boolean; onCatalog?: (catalog: ModelCatalog) => void }) {
  const client = useClient();
  const toast = useToast();
  const settings = useModelSettings();
  const { catalog, keys, subscriptions, pools, accounts, readOnly } = settings;
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const keyDialog = useDialogState<{ provider: string; first: boolean }>();
  const subscriptionDialog = useDialogState<{ source: string; first: boolean }>();
  const signInDialog = useDialogState<SubscriptionAccount>();
  const manageDialog = useDialogState<AccountRow>();
  useEffect(() => {
    if (catalog.data) onCatalog?.(catalog.data);
  }, [catalog.data, onCatalog]);
  const [confirmElement, ask] = useConfirm();

  const poolOf = (provider: string): ModelAccountRef[] => pools.data?.pools.find((pool) => pool.provider === provider)?.accounts ?? [];
  const rowsOf = (provider: string) =>
    accountsOf(
      catalog.data!.providers.find((candidate) => candidate.provider === provider)!,
      poolOf(provider),
      keys.data?.keys ?? [],
      subscriptions.data?.sources.flatMap((source) => source.accounts) ?? [],
    );

  async function run(id: string, action: () => Promise<void>) {
    setBusy(id);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  const saveModels = (next: Roles) =>
    run("models", async () => {
      await client.setModels({ default_model: next.reasoning, fast_model: next.fast, fallback_models: next.fallbacks });
      toast.notify("Models saved.");
      await catalog.reload();
    });

  // A new account only serves requests once it is in the provider's rotation, so adding one
  // offers to join it (and always joins when it is the provider's first).
  async function joinRotation(provider: string, ref: ModelAccountRef) {
    await client.setPool(provider, [...poolOf(provider), ref]);
  }

  async function addKey(provider: string, label: string, key: string, join: boolean) {
    const created = await client.addApiKey(provider, { label, key });
    if (join) await joinRotation(provider, { source: "vault", account_id: created.key_id });
    toast.notify(`${providerLabel(provider)} API key saved.`);
    await settings.reload();
  }

  async function addSubscription(source: string, label: string, join: boolean): Promise<SubscriptionAccount> {
    const created = await client.addSubscriptionAccount(source, label);
    if (join) await joinRotation(created.provider, { source: created.source, account_id: created.account_id });
    await settings.reload();
    return created;
  }

  async function saveAccount(row: AccountRow, changes: ManageChanges) {
    const patch = { ...(changes.label !== undefined ? { label: changes.label } : {}), ...(changes.reserve_percent !== undefined ? { reserve_percent: changes.reserve_percent } : {}) };
    if (row.key) {
      if (changes.key) await client.replaceApiKey(row.provider, row.key.key_id, { key: changes.key, ...(changes.label !== undefined ? { label: changes.label } : {}) });
      const rest = changes.key ? { ...(changes.reserve_percent !== undefined ? { reserve_percent: changes.reserve_percent } : {}) } : patch;
      if (Object.keys(rest).length) await client.updateApiKey(row.provider, row.key.key_id, rest);
    } else if (row.subscription) {
      await client.updateSubscriptionAccount(row.subscription.source, row.subscription.account_id, patch);
    }
    toast.notify("Saved.");
    await settings.reload();
  }

  const toggle = (row: AccountRow, enabled: boolean) =>
    run(refKey(row.ref), async () => {
      if (row.key) await client.updateApiKey(row.provider, row.key.key_id, { enabled });
      else if (row.subscription) await client.updateSubscriptionAccount(row.subscription.source, row.subscription.account_id, { enabled });
      await settings.reload();
    });

  const setPool = (provider: string, next: ModelAccountRef[]) =>
    run(`pool:${provider}`, async () => {
      await client.setPool(provider, next);
      await Promise.all([pools.reload(), catalog.reload()]);
    });

  const removeAccount = (row: AccountRow) =>
    ask({
      title: row.subscription && !row.subscription.managed ? `Stop using ${row.label}?` : `Remove ${row.label}?`,
      description: row.key
        ? "The key is deleted from the vault on this computer. Requests made from now on stop using it; one already running may finish. This does not revoke the key with the provider."
        : row.subscription && !row.subscription.managed
          ? "openside stops listing and using this sign-in. Your terminal's own sign-in is not touched."
          : "The sign-in openside stored for this account is wiped. Requests made from now on stop using it. Your own terminal sign-in is not touched.",
      confirmLabel: row.subscription && !row.subscription.managed ? "Stop using" : "Remove",
      onConfirm: () =>
        void run(refKey(row.ref), async () => {
          if (row.key) await client.deleteApiKey(row.provider, row.key.key_id);
          else if (row.subscription) await client.deleteSubscriptionAccount(row.subscription.source, row.subscription.account_id);
          toast.notify("Account removed.");
          await settings.reload();
        }),
    });

  return (
    <div className="manage-body" data-section={compact ? "models-step" : "models"}>
      <ResourceView resource={catalog} what="models">
        {(data) => (
          <>
            {anyModelReady(data) ? null : (
              <div className="models-callout" role="status">
                <CircleAlert size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                <p>No model is ready yet. Add an API key or a subscription account below, then start a conversation.</p>
              </div>
            )}
            {readOnly ? (
              <div className="models-callout models-callout--info" role="note">
                <Lock size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                <p>
                  This device can see which models are ready but cannot change them. Accounts, rotation and the fast and reasoning models are
                  managed on the computer running openside or from an admin device.
                </p>
              </div>
            ) : null}
            {!readOnly && keys.error ? <ErrorNote>{`Saved keys cannot be read right now. ${keys.error}`}</ErrorNote> : null}
            {error ? <ErrorNote>{error}</ErrorNote> : null}
            {compact ? null : <ModelRoles catalog={data} readOnly={readOnly} busy={busy === "models"} onSave={(next) => void saveModels(next)} />}
            <SectionList
              title={compact ? undefined : "Providers"}
              summary={compact ? "Connect at least one provider to use a model." : undefined}
              label="Providers"
              actions={
                <Button size="compact" onClick={() => void settings.reload()}>
                  <RefreshCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  Refresh
                </Button>
              }
            >
              {data.providers.map((provider) => {
                const rows = rowsOf(provider.provider);
                return (
                  <ProviderCard
                    key={provider.provider}
                    provider={provider}
                    rows={rows}
                    pool={poolOf(provider.provider)}
                    readOnly={readOnly}
                    compact={compact}
                    busy={busy}
                    onAddKey={(id) => keyDialog.show({ provider: id, first: rows.length === 0 })}
                    onAddSubscription={(source) => subscriptionDialog.show({ source, first: rows.length === 0 })}
                    onSignIn={(row) => row.subscription && signInDialog.show(row.subscription)}
                    onManage={(row) => manageDialog.show(row)}
                    onToggle={(row, enabled) => void toggle(row, enabled)}
                    onPool={(id, next) => void setPool(id, next)}
                  />
                );
              })}
            </SectionList>
            {compact || !accounts.data?.sources.length ? null : (
              <SectionList title="Credential sources" description="Where openside looks for model credentials, in priority order.">
                <ul className="resource-list" aria-label="Credential sources">
                  {accounts.data.sources.map((source) => (
                    <li key={source.name} data-source={source.name}>
                      <ResourceCard
                        title={source.name}
                        detail={source.reason || source.kind}
                        badge={source.available ? "Available" : "Unavailable"}
                        badgeTone={source.available ? "success" : "neutral"}
                      />
                    </li>
                  ))}
                </ul>
                {accounts.data.truncated ? <p className="settings-text">Some accounts are not shown.</p> : null}
              </SectionList>
            )}
          </>
        )}
      </ResourceView>
      {keyDialog.value ? <AddKeyDialog open={keyDialog.open} provider={keyDialog.value.provider} first={keyDialog.value.first} onClose={keyDialog.close} onSave={addKey} /> : null}
      {subscriptionDialog.value ? (
        <AddSubscriptionDialog
          open={subscriptionDialog.open}
          source={subscriptionDialog.value.source}
          first={subscriptionDialog.value.first}
          onClose={subscriptionDialog.close}
          onCreate={addSubscription}
          onSignedIn={() => void settings.reload()}
        />
      ) : null}
      {signInDialog.value ? <SignInDialog open={signInDialog.open} account={signInDialog.value} onClose={signInDialog.close} onSignedIn={() => void settings.reload()} /> : null}
      {manageDialog.value ? <ManageAccountDialog open={manageDialog.open} row={manageDialog.value} onClose={manageDialog.close} onSave={saveAccount} onRemove={removeAccount} /> : null}
      {confirmElement}
    </div>
  );
}
