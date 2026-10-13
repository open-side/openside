import { Select as BaseSelect } from "@base-ui/react/select";
import { Check, ChevronsUpDown } from "lucide-react";
import type { ReactNode } from "react";
import { cx, ICON_STROKE } from "./Button";

export interface SelectOption {
  value: string;
  label: string;
  /** Leading mark: an icon or a colour swatch. */
  icon?: ReactNode;
  /** Muted second line under the label. */
  detail?: string;
  /** Small pill after the label, for a short category such as a model tier. */
  tag?: string;
  disabled?: boolean;
}

export interface SelectGroup {
  label: string;
  options: readonly SelectOption[];
}

export interface SelectProps {
  value: string;
  onChange: (value: string) => void;
  options: readonly (SelectOption | SelectGroup)[];
  /** Id the visible `<label htmlFor>` points at. */
  id?: string;
  name?: string;
  /** Required when no visible label points at the control. */
  "aria-label"?: string;
  "aria-describedby"?: string;
  title?: string;
  /** Shown while `value` matches no option. */
  placeholder?: string;
  disabled?: boolean;
  /**
   * `field` is a bordered input-sized control; `inline` is a borderless, right-aligned value
   * for settings rows and dense lists.
   */
  variant?: "field" | "inline";
  className?: string;
}

// Base UI treats "" as "no value", but an option such as "(not set)" legitimately uses it.
const EMPTY = "\u0000empty";

const isGroup = (item: SelectOption | SelectGroup): item is SelectGroup => "options" in item;

/**
 * Accessible listbox that replaces the native `<select>`: a combobox button with a glass popup
 * on wide screens and a bottom sheet on phones (ui.css). Base UI owns keyboard (arrows,
 * Home/End, type-ahead, Enter, Escape), focus return and scrolling the selection into view.
 */
export function Select({
  value,
  onChange,
  options,
  id,
  name,
  "aria-label": ariaLabel,
  "aria-describedby": describedBy,
  title,
  placeholder,
  disabled,
  variant = "field",
  className,
}: SelectProps) {
  const flat = options.flatMap((item) => (isGroup(item) ? item.options : [item]));
  const selected = flat.find((option) => option.value === value);

  const renderOption = (option: SelectOption) => (
    <BaseSelect.Item key={option.value} value={option.value === "" ? EMPTY : option.value} label={option.label} disabled={option.disabled} className="ui-menu-item ui-select-item">
      {option.icon && (
        <span className="ui-menu-icon ui-select-icon" aria-hidden="true">
          {option.icon}
        </span>
      )}
      <span className="ui-select-text">
        <span className="ui-select-title">
          <BaseSelect.ItemText>{option.label}</BaseSelect.ItemText>
          {option.tag && <span className="ui-select-tag">{option.tag}</span>}
        </span>
        {option.detail && <span className="ui-select-detail">{option.detail}</span>}
      </span>
      <BaseSelect.ItemIndicator className="ui-select-check" aria-hidden="true">
        <Check size={16} strokeWidth={ICON_STROKE} />
      </BaseSelect.ItemIndicator>
    </BaseSelect.Item>
  );

  return (
    <BaseSelect.Root
      id={id}
      name={name}
      disabled={disabled}
      value={selected ? (selected.value === "" ? EMPTY : selected.value) : null}
      onValueChange={(next) => {
        if (next !== null) onChange(next === EMPTY ? "" : next);
      }}
    >
      <BaseSelect.Trigger className={cx("ui-select", `ui-select--${variant}`, className)} aria-label={ariaLabel} aria-describedby={describedBy} title={title}>
        <span className="ui-select-value" data-placeholder={selected ? undefined : ""}>
          {selected?.icon && (
            <span className="ui-select-icon" aria-hidden="true">
              {selected.icon}
            </span>
          )}
          <span className="ui-select-label">{selected ? selected.label : placeholder}</span>
        </span>
        <BaseSelect.Icon className="ui-select-chevron">
          <ChevronsUpDown size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </BaseSelect.Icon>
      </BaseSelect.Trigger>
      <BaseSelect.Portal>
        <BaseSelect.Backdrop className="ui-select-backdrop" />
        <BaseSelect.Positioner className="ui-select-positioner" side="bottom" align="start" sideOffset={6} collisionPadding={8} alignItemWithTrigger={false}>
          <BaseSelect.Popup className="ui-select-popup glass-float">
            <BaseSelect.List className="ui-select-list thin-scroll">
              {options.map((item) =>
                isGroup(item) ? (
                  <BaseSelect.Group key={item.label} className="ui-select-group">
                    <BaseSelect.GroupLabel className="ui-menu-title">{item.label}</BaseSelect.GroupLabel>
                    {item.options.map(renderOption)}
                  </BaseSelect.Group>
                ) : (
                  renderOption(item)
                ),
              )}
            </BaseSelect.List>
          </BaseSelect.Popup>
        </BaseSelect.Positioner>
      </BaseSelect.Portal>
    </BaseSelect.Root>
  );
}
