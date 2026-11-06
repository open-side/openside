import { ArrowUpRight, ChevronDown } from "lucide-react";
import { useNavigate } from "react-router";
import type { ModelCatalog } from "../api/types";
import { t } from "../i18n";
import { chatModelChoices, modelDisplayName } from "../models/modelNames";
import { BrandMark, ICON_STROKE, MenuItem, MenuSeparator, PopoverMenu } from "../ui";
import { settingsPath } from "../settings/nav";

export interface ModelPickerProps {
  /** Read by the composer, which needs it for more than this chip. */
  catalog: ModelCatalog | null;
  /** The chosen model id; `""` or `null` is the daemon default. */
  value: string | null;
  /** Shown on the chip while `value` is the default. */
  fallback?: string | null;
  onChange: (model: string) => void;
  disabled?: boolean;
  /** Tooltip of a disabled chip. */
  title?: string;
  side?: "top" | "bottom";
  align?: "start" | "end";
}

/**
 * Aside's model picker (`L 15`, `L 67`): a 194px popover with a search row, the chat models that
 * can be used right now by display name, a ✓ on the current one and a link to Settings › Models.
 * Pools, accounts, raw ids and unavailable models stay in Settings.
 */
export function ModelPicker({ catalog, value, fallback, onChange, disabled, title, side = "top", align = "end" }: ModelPickerProps) {
  const navigate = useNavigate();

  const choices = chatModelChoices(catalog);
  const current = value || null;
  const shown = current ?? fallback ?? catalog?.default_model ?? null;
  const name = shown ? modelDisplayName(catalog, shown) : t("thread.options.default");

  return (
    <PopoverMenu
      label={t("thread.model.label")}
      side={side}
      align={align}
      sideOffset={5}
      popupClassName="composer-model-menu"
      search={{ placeholder: t("thread.options.searchModels"), empty: t("thread.options.noModelMatches") }}
      trigger={
        <button type="button" className="composer-chip composer-model-chip" disabled={disabled} title={disabled ? title : undefined} aria-label={t("thread.model.chipLabel", { name })}>
          <BrandMark size={14} />
          <span className="composer-chip-label">{name}</span>
          <ChevronDown className="composer-chip-caret" strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      }
    >
      {choices.length === 0 ? <MenuItem disabled>{t("thread.modelPicker.noneReady")}</MenuItem> : null}
      {choices.map((choice) => (
        <MenuItem key={choice.id} checked={choice.id === shown} textValue={choice.name} onClick={() => onChange(choice.id)}>
          {choice.name}
        </MenuItem>
      ))}
      <MenuSeparator />
      <MenuItem trailing={<ArrowUpRight strokeWidth={ICON_STROKE} aria-hidden="true" />} onClick={() => navigate(settingsPath("models"))}>
        {t("thread.modelPicker.settings")}
      </MenuItem>
    </PopoverMenu>
  );
}

