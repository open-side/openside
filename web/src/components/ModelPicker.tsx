import { Check, ChevronDown } from "lucide-react";
import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import type { ModelCatalog } from "../api/types";
import { useClient } from "../context";
import { providerLabel } from "../models/providers";
import "../models/models.css";
import { manageLinkState } from "../pages/manageRoutes";
import { Button, Dialog, ICON_STROKE } from "../ui";

interface Choice {
  value: string;
  label: string;
  available: boolean;
  reason: string;
}

interface Group {
  provider: string;
  choices: Choice[];
}

interface Loaded {
  defaultModel: string | null;
  groups: Group[];
}

function fromCatalog(catalog: ModelCatalog): Loaded {
  return {
    defaultModel: catalog.default_model ?? null,
    groups: catalog.providers.map((provider) => ({
      provider: provider.provider,
      choices: provider.models.map((model) => ({ value: model.id, label: model.label, available: model.available, reason: model.reason })),
    })),
  };
}

/**
 * Lists every model in the daemon's catalog by provider, disabling the ones that cannot run right
 * now with the reason; anything else is typed freely. An empty value means "use the daemon default".
 * If the catalog cannot be read, the configured default and fallbacks are offered instead.
 */
export function ModelPicker({ value, onChange }: { value: string; onChange: (model: string) => void }) {
  const client = useClient();
  const navigate = useNavigate();
  const location = useLocation();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let live = true;
    client
      .modelCatalog()
      .then((catalog) => {
        if (!Array.isArray(catalog.providers)) throw new Error("catalog missing");
        if (live) setLoaded(fromCatalog(catalog));
      })
      .catch(() =>
        client
          .settings()
          .then((settings) => {
            const ids = [settings.default_model, ...settings.fallback_models].filter(
              (model, index, all): model is string => !!model && all.indexOf(model) === index,
            );
            if (live) {
              setLoaded({
                defaultModel: settings.default_model ?? null,
                groups: ids.length ? [{ provider: "", choices: ids.map((id) => ({ value: id, label: id, available: true, reason: "" })) }] : [],
              });
            }
          })
          .catch(() => live && setLoaded({ defaultModel: null, groups: [] })),
      );
    return () => {
      live = false;
    };
  }, [client]);

  const groups = loaded?.groups ?? [];
  const all = groups.flatMap((group) => group.choices);
  const defaultChoice = all.find((choice) => choice.value === loaded?.defaultModel);
  const defaultLabel = `Default${loaded?.defaultModel ? ` (${loaded.defaultModel})` : ""}`;
  const custom = value !== "" && !all.some((choice) => choice.value === value);
  const noneReady = loaded !== null && !all.some((choice) => choice.available);

  function pick(model: string) {
    onChange(model);
    setOpen(false);
  }

  function option(choice: { value: string; label: string; sub?: string; disabled?: boolean }) {
    const selected = value === choice.value;
    return (
      <button
        key={choice.value || "default"}
        type="button"
        role="radio"
        aria-checked={selected}
        disabled={choice.disabled}
        className="model-option"
        onClick={() => pick(choice.value)}
      >
        <span className="model-option-copy">
          <span>{choice.label}</span>
          {choice.sub ? <small>{choice.sub}</small> : null}
        </span>
        {selected ? <Check size={18} strokeWidth={ICON_STROKE} aria-hidden="true" /> : null}
      </button>
    );
  }

  const note = (
    <p className="model-picker-note" data-top={noneReady || undefined}>
      <span>{noneReady ? "No model is ready yet." : "Providers and keys are set up in Settings."}</span>
      <Button
        size="compact"
        variant={noneReady ? "primary" : "ghost"}
        onClick={() => {
          setOpen(false);
          navigate("/models", { state: manageLinkState(location) });
        }}
      >
        Models &amp; accounts
      </Button>
    </p>
  );

  return (
    <div className="model-picker">
      <button type="button" className="tool-pill" aria-haspopup="dialog" onClick={() => setOpen(true)}>
        <span className="sr-only">Model: </span>
        <span className="tool-pill-label">{value || defaultLabel}</span>
        <ChevronDown size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
      <Dialog open={open} onClose={() => setOpen(false)} title="Models" description="Used for the conversation this message starts.">
        {noneReady ? note : null}
        <div className="model-options" role="radiogroup" aria-label="Model">
          {option({ value: "", label: defaultLabel, sub: defaultChoice && !defaultChoice.available ? defaultChoice.reason || "Not available right now" : undefined })}
          {groups.map((group) => {
            // One shared reason is said once under the provider instead of under every model.
            const reasons = new Set(group.choices.filter((choice) => !choice.available).map((choice) => choice.reason));
            const sharedReason = group.choices.every((choice) => !choice.available) && reasons.size === 1 ? [...reasons][0] : "";
            return (
              <div key={group.provider} className="model-group" role="group" aria-label={group.provider ? providerLabel(group.provider) : "Configured models"}>
                <h3 className="model-group-title">{group.provider ? providerLabel(group.provider) : "Configured"}</h3>
                {sharedReason ? <p className="model-group-reason">{sharedReason}</p> : null}
                {group.choices.map((choice) =>
                  option({
                    value: choice.value,
                    label: choice.label,
                    sub: choice.available ? (choice.label === choice.value ? undefined : choice.value) : sharedReason ? undefined : choice.reason || "Not available right now",
                    disabled: !choice.available,
                  }),
                )}
              </div>
            );
          })}
        </div>
        <label className="field">
          <span>Custom model</span>
          <input
            value={custom ? value : ""}
            onChange={(event) => onChange(event.target.value)}
            placeholder="provider:model"
            autoCapitalize="off"
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        {noneReady ? null : note}
      </Dialog>
    </div>
  );
}
