import { Check, ChevronDown } from "lucide-react";
import { useEffect, useState } from "react";
import type { Settings } from "../api/types";
import { useClient } from "../context";
import { Dialog, ICON_STROKE } from "../ui";

/**
 * Offers the daemon's configured default and fallback models; anything else is
 * typed freely. An empty value means "use the daemon default".
 */
export function ModelPicker({ value, onChange }: { value: string; onChange: (model: string) => void }) {
  const client = useClient();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let live = true;
    client
      .settings()
      .then((loaded) => live && setSettings(loaded))
      .catch(() => live && setSettings({ fallback_models: [], policy_mode: "guard" }));
    return () => {
      live = false;
    };
  }, [client]);

  const configured = [settings?.default_model, ...(settings?.fallback_models ?? [])].filter(
    (model, index, all): model is string => !!model && all.indexOf(model) === index,
  );
  const defaultLabel = `Default${settings?.default_model ? ` (${settings.default_model})` : ""}`;
  const options = [{ value: "", label: defaultLabel }, ...configured.map((model) => ({ value: model, label: model }))];
  const custom = value !== "" && !configured.includes(value);

  return (
    <div className="model-picker">
      <button type="button" className="tool-pill" aria-haspopup="dialog" onClick={() => setOpen(true)}>
        <span className="sr-only">Model: </span>
        <span className="tool-pill-label">{value || defaultLabel}</span>
        <ChevronDown size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
      <Dialog open={open} onClose={() => setOpen(false)} title="Models" description="Used for the conversation this message starts.">
        <div className="model-options" role="radiogroup" aria-label="Model">
          {options.map((option) => (
            <button
              key={option.value || "default"}
              type="button"
              role="radio"
              aria-checked={value === option.value}
              className="model-option"
              onClick={() => {
                onChange(option.value);
                setOpen(false);
              }}
            >
              <span>{option.label}</span>
              {value === option.value ? <Check size={18} strokeWidth={ICON_STROKE} aria-hidden="true" /> : null}
            </button>
          ))}
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
      </Dialog>
    </div>
  );
}
