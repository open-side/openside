import type { CatalogModel, ModelCatalog } from "../api/types";

export interface ModelChoice {
  id: string;
  name: string;
  /** Provider namespace the model is listed under, for its glyph. */
  provider: string;
}

/** What a person reads for a catalog model: the curated product name, else the model-name label. */
export function catalogModelName(model: Pick<CatalogModel, "display_name" | "label" | "id">): string {
  return model.display_name.trim() || model.label.trim() || model.id;
}

/**
 * The models of one kind that can be used right now, by display name, in catalog order. Models
 * without a usable account are left out rather than listed with a reason: the person fixes
 * accounts in Settings, not in a picker.
 */
export function modelChoices(catalog: ModelCatalog | null, kind: CatalogModel["kind"]): ModelChoice[] {
  const seen = new Set<string>();
  const choices: ModelChoice[] = [];
  for (const provider of catalog?.providers ?? []) {
    for (const model of provider.models) {
      if (model.kind !== kind || !model.available || seen.has(model.id)) continue;
      seen.add(model.id);
      choices.push({ id: model.id, name: catalogModelName(model), provider: provider.provider });
    }
  }
  return choices;
}

/** The chat models a chat can switch to right now; image models never serve a conversation. */
export function chatModelChoices(catalog: ModelCatalog | null): ModelChoice[] {
  return modelChoices(catalog, "chat");
}

/**
 * The display name of a model id. An id the catalog does not know (a CLI-configured one) is shown
 * by its model part only, so a `provider:` prefix never reaches the screen.
 */
export function modelDisplayName(catalog: ModelCatalog | null, id: string): string {
  for (const provider of catalog?.providers ?? []) {
    const found = provider.models.find((model) => model.id === id);
    if (found) return catalogModelName(found);
  }
  const colon = id.indexOf(":");
  return colon >= 0 ? id.slice(colon + 1) : id;
}
