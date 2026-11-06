import { useCallback, useState } from "react";
import type { ModelCatalog, ModelsUpdate } from "../../api/types";
import { useClient } from "../../context";
import { errorMessage, useResource } from "../../hooks/useResource";
import { t, type MessageKey } from "../../i18n";
import { modelChoices } from "../../models/modelNames";
import { ProviderGlyph } from "../../models/ProviderGlyph";
import { ProvidersCard } from "../../models/ProvidersCard";
import { ResourceView } from "../../pages/blocks";
import { MenuItem, SettingsCard, SettingsRow, SettingsSection, SplitSelect, useToast, type SelectOption } from "../../ui";
import "../../models/models.css";

type RoleKey = "default_model" | "fast_model" | "standard_model" | "deep_model" | "visual_model" | "image_model";
type Roles = Record<RoleKey, string | null>;

const ROLE_KEYS: RoleKey[] = ["default_model", "fast_model", "standard_model", "deep_model", "visual_model", "image_model"];
/** The stored `image_model` that turns image generation off; null means automatic. */
const IMAGE_OFF = "off";
/** Chat roles the daemon keeps when the request leaves them out. */
const OPTIONAL_ROLES = ["standard_model", "deep_model", "visual_model"] as const;

/** The task rows of the second card, in Aside's order (`L 48`). */
const TASKS: { key: Exclude<RoleKey, "default_model" | "image_model">; title: MessageKey; description: MessageKey }[] = [
  { key: "fast_model", title: "models.task.fast", description: "models.task.fast.description" },
  { key: "standard_model", title: "models.task.standard", description: "models.task.standard.description" },
  { key: "deep_model", title: "models.task.deep", description: "models.task.deep.description" },
  { key: "visual_model", title: "models.task.visual", description: "models.task.visual.description" },
];

const rolesOf = (catalog: ModelCatalog): Roles => Object.fromEntries(ROLE_KEYS.map((key) => [key, catalog[key] ?? null])) as Roles;

/**
 * The whole selection with `patch` applied, or null while there is no usable default model (every
 * save carries one). The daemon rejects the whole save if any named model is unusable or of the
 * wrong kind, so a stored optional role that went stale is left out (the daemon keeps it), while a
 * stale fast model or fallback, which must be sent, is dropped. A new default never stays a
 * fallback too; the daemon refuses the pair. Image generation is sent only when the patch names it,
 * so no other save can turn it on, off or back to automatic.
 */
export function modelsRequest(catalog: ModelCatalog, patch: Partial<Roles>): ModelsUpdate | null {
  const roles = { ...rolesOf(catalog), ...patch };
  const chat = new Set(modelChoices(catalog, "chat").map((choice) => choice.id));
  const defaultModel = roles.default_model;
  if (defaultModel === null || !chat.has(defaultModel)) return null;
  const fast = roles.fast_model;
  const request: ModelsUpdate = {
    default_model: defaultModel,
    fast_model: "fast_model" in patch || fast === null || chat.has(fast) ? fast : null,
    fallback_models: catalog.fallback_models.filter((id) => id !== defaultModel && chat.has(id)),
  };
  for (const key of OPTIONAL_ROLES) {
    const value = roles[key];
    if (key in patch || value === null || chat.has(value)) request[key] = value;
  }
  if ("image_model" in patch) request.image_model = roles.image_model;
  return request;
}

function TaskModels({ catalog, onSaved }: { catalog: ModelCatalog; onSaved: () => Promise<void> }) {
  const client = useClient();
  const toast = useToast();
  // The pick shows at once; the catalog reload after the save confirms it.
  const [pending, setPending] = useState<Partial<Roles> | null>(null);
  const roles = { ...rolesOf(catalog), ...pending };
  const [chat, images] = (["chat", "image"] as const).map((kind): SelectOption[] =>
    modelChoices(catalog, kind).map((choice) => ({ value: choice.id, label: choice.name, icon: <ProviderGlyph provider={choice.provider} /> })),
  ) as [SelectOption[], SelectOption[]];
  const known = (id: string | null) => (id !== null && chat.some((option) => option.value === id) ? id : "");
  // Every save carries a usable default model, so the other roles wait until there is one.
  const ready = known(roles.default_model) !== "";

  async function save(patch: Partial<Roles>) {
    const request = modelsRequest(catalog, patch);
    if (!request) return;
    setPending((previous) => ({ ...previous, ...patch }));
    try {
      await client.setModels(request);
      toast.notify(t("models.task.saved"));
      await onSaved();
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    } finally {
      setPending(null);
    }
  }

  const none = chat.length === 0 ? t("models.task.noneAvailable") : undefined;
  // Automatic (null) shows the model the daemon resolved; the pending pick shows before the reload.
  const imageAuto = roles.image_model === null;
  const imageShown = imageAuto ? catalog.effective_image_model : roles.image_model;
  const imageValue = imageShown && images.some((option) => option.value === imageShown) ? imageShown : "";
  return (
    <SettingsSection label={t("models.task.title")}>
      <SettingsCard>
        <SettingsRow
          title={t("models.task.default")}
          description={t("models.task.default.description")}
          control={(ids) => (
            <SplitSelect
              aria-labelledby={ids.labelId}
              value={known(roles.default_model)}
              options={chat}
              placeholder={none ?? t("models.task.choose")}
              disabled={chat.length === 0}
              onChange={(id) => void save({ default_model: id })}
              more={{
                label: t("models.task.more", { name: t("models.task.default") }),
                items: (
                  <MenuItem disabled={!ready} onClick={() => void save({ fast_model: null, standard_model: null, deep_model: null, visual_model: null })}>
                    {t("models.task.resetAll")}
                  </MenuItem>
                ),
              }}
            />
          )}
        />
      </SettingsCard>
      <SettingsCard>
        {TASKS.map((task) => (
          <SettingsRow
            key={task.key}
            title={t(task.title)}
            description={t(task.description)}
            control={(ids) => (
              <SplitSelect
                aria-labelledby={ids.labelId}
                value={known(roles[task.key])}
                options={chat}
                placeholder={none ?? t("models.task.sessionModel")}
                disabled={!ready}
                onChange={(id) => void save({ [task.key]: id } as Partial<Roles>)}
                more={{
                  label: t("models.task.more", { name: t(task.title) }),
                  items: (
                    <MenuItem checked={roles[task.key] === null} checkRole="checkbox" onClick={() => void save({ [task.key]: null } as Partial<Roles>)}>
                      {t("models.task.useSession")}
                    </MenuItem>
                  ),
                }}
              />
            )}
          />
        ))}
      </SettingsCard>
      <SettingsCard>
        <SettingsRow
          title={t("models.task.image")}
          description={t("models.task.image.description")}
          control={(ids) => (
            <SplitSelect
              aria-labelledby={ids.labelId}
              value={imageValue}
              options={images.length ? [{ value: "", label: t("models.task.image.off") }, ...images] : []}
              placeholder={t("models.task.noneAvailable")}
              disabled={!images.length || !ready}
              onChange={(id) => void save({ image_model: id || IMAGE_OFF })}
              more={{
                label: t("models.task.more", { name: t("models.task.image") }),
                items: (
                  <MenuItem checked={imageAuto} checkRole="checkbox" disabled={!ready} onClick={() => void save({ image_model: null })}>
                    {t("models.task.image.auto")}
                  </MenuItem>
                ),
              }}
            />
          )}
        />
      </SettingsCard>
    </SettingsSection>
  );
}

/** Settings › Models: connected providers, then the model each kind of task uses. */
export function ModelsSettings() {
  const client = useClient();
  const catalog = useResource(useCallback(() => client.modelCatalog(), [client]));
  const { reload } = catalog;
  return (
    <div className="models-page" data-section="models">
      <SettingsSection label={t("models.providers.title")}>
        <ProvidersCard onChanged={() => void reload()} />
      </SettingsSection>
      <ResourceView resource={catalog} what={t("models.scope.models")}>
        {(data) => <TaskModels catalog={data} onSaved={reload} />}
      </ResourceView>
    </div>
  );
}
