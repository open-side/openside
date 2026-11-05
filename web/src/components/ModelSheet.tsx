import { Check, ChevronDown } from "lucide-react";
import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router";
import type { AgentSummary, ModelCatalog, ModelRole } from "../api/types";
import { mayUse } from "../auth";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { providerLabel } from "../models/providers";
import "../models/models.css";
import { ErrorNote } from "../pages/blocks";
import { manageLinkState } from "../pages/manageRoutes";
import { Chip, Dialog, ICON_STROKE } from "../ui";

const ROLE_LABEL: Record<ModelRole, string> = { fast: "Fast", reasoning: "Reasoning" };

/** The catalog's label for a model id; ids the catalog does not know stay as typed. */
function modelName(catalog: ModelCatalog | null, id: string): string {
  for (const provider of catalog?.providers ?? []) {
    const found = provider.models.find((model) => model.id === id);
    if (found) return found.label;
  }
  return id;
}

/**
 * Quick model choice for one person (aside-relay `model-picker`): a radio list of the daemon's
 * catalog by provider, with `Default` returning to the person's default. Picking saves at once.
 * Providers, keys and the fast/reasoning roles stay in Models & accounts.
 */
export function ModelSheet({ agent, open, onClose }: { agent: AgentSummary; open: boolean; onClose: () => void }) {
  const client = useClient();
  const team = useTeam();
  const location = useLocation();
  const [catalog, setCatalog] = useState<ModelCatalog | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saving, setSaving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const current = agent.model ?? null;

  // Availability changes as keys are added, so the catalog is read each time the sheet opens.
  useEffect(() => {
    if (!open) return;
    let live = true;
    setError(null);
    client.modelCatalog().then(
      (next) => {
        if (!live) return;
        setCatalog(next);
        setLoadError(null);
      },
      (cause: unknown) => live && setLoadError(`Could not load the model list: ${cause instanceof Error ? cause.message : "request failed"}.`),
    );
    return () => {
      live = false;
    };
  }, [client, open]);

  async function pick(model: string | null) {
    if (model === current) {
      onClose();
      return;
    }
    setSaving(model ?? "");
    setError(null);
    try {
      await client.updateAgent(agent.id, { model });
      team.refresh();
      onClose();
    } catch {
      setError(`Could not change ${agent.name}'s model. Try again.`);
    } finally {
      setSaving(null);
    }
  }

  function row({ value, label, detail, meta, disabled }: { value: string | null; label: string; detail?: string; meta?: string; disabled?: boolean }) {
    const selected = value === current;
    return (
      <button
        key={value ?? "default"}
        type="button"
        role="radio"
        aria-checked={selected}
        disabled={disabled || saving !== null}
        className="model-option model-sheet-option"
        onClick={() => void pick(value)}
      >
        <span className="model-option-copy">
          <span>{label}</span>
          {detail ? <small>{detail}</small> : null}
        </span>
        {meta ? <span className="model-sheet-meta">{meta}</span> : null}
        {selected ? <Check size={18} strokeWidth={ICON_STROKE} aria-hidden="true" /> : null}
      </button>
    );
  }

  const defaultModel = catalog?.default_model ?? null;
  return (
    <Dialog open={open} onClose={onClose} title="Model" description={`Used for ${agent.name}'s next messages.`} className="model-sheet">
      <div className="model-options" role="radiogroup" aria-label="Model">
        {row({ value: null, label: "Default", detail: defaultModel ? modelName(catalog, defaultModel) : undefined })}
        {catalog?.providers.map((provider) => (
          <div key={provider.provider} className="model-group" role="group" aria-label={providerLabel(provider.provider)}>
            <h3 className="section-heading">{providerLabel(provider.provider)}</h3>
            {provider.models.map((model) =>
              row({
                value: model.id,
                label: model.label,
                detail: model.available ? undefined : model.reason || "Not available right now",
                meta: model.recommended_for ? ROLE_LABEL[model.recommended_for] : undefined,
                disabled: !model.available,
              }),
            )}
          </div>
        ))}
      </div>
      {loadError ? <ErrorNote>{loadError}</ErrorNote> : null}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <Link className="model-sheet-manage" to="/models" state={manageLinkState(location)} onClick={onClose}>
        Manage models &amp; accounts
      </Link>
    </Dialog>
  );
}

/**
 * Chip naming the person's model; it opens `ModelSheet`. A device token without `admin` cannot
 * change a person, so it gets a plain label instead.
 */
export function ModelButton({ agent }: { agent: AgentSummary }) {
  const client = useClient();
  const [catalog, setCatalog] = useState<ModelCatalog | null>(null);
  const [open, setOpen] = useState(false);
  const readOnly = mayUse("admin") === false;

  useEffect(() => {
    if (!agent.model) return;
    let live = true;
    client.modelCatalog().then(
      (next) => live && setCatalog(next),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [client, agent.model]);

  const name = agent.model ? modelName(catalog, agent.model) : "Default model";
  if (readOnly) return <Chip aria-label={`Model: ${name}`}>{name}</Chip>;
  return (
    <>
      <Chip aria-label={`Model: ${name}`} onClick={() => setOpen(true)} icon={<ChevronDown className="model-chip-chevron" size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />}>
        {name}
      </Chip>
      <ModelSheet agent={agent} open={open} onClose={() => setOpen(false)} />
    </>
  );
}
