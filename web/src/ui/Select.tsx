import { Select as BaseSelect } from "@base-ui/react/select";
import { Check, ChevronDown, ChevronsUpDown } from "lucide-react";
import type { ReactNode } from "react";
import { formatList } from "../i18n";
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

interface SelectBaseProps {
  options: readonly (SelectOption | SelectGroup)[];
  /** Id the visible `<label htmlFor>` points at. */
  id?: string;
  name?: string;
  /** Required when no visible label points at the control. */
  "aria-label"?: string;
  "aria-labelledby"?: string;
  "aria-describedby"?: string;
  title?: string;
  /** Shown while nothing is chosen. */
  placeholder?: string;
  /** Trigger text instead of the chosen labels, e.g. "Everything" when every kind is chosen. */
  summary?: string;
  disabled?: boolean;
  /**
   * `inline` hugs its content at a settings row's end (Aside's select); `field` fills its
   * column (forms, dialogs).
   */
  variant?: "field" | "inline";
  className?: string;
}

export interface SelectProps extends SelectBaseProps {
  value: string;
  onChange: (value: string) => void;
}

export type DropdownProps = SelectBaseProps &
  ({ multiple?: false; value: string; onChange: (value: string) => void } | { multiple: true; value: readonly string[]; onChange: (value: string[]) => void });

// Base UI treats "" as "no value", but an option such as "(not set)" legitimately uses it.
const EMPTY = "\u0000empty";
const encode = (value: string) => (value === "" ? EMPTY : value);
const decode = (value: string) => (value === EMPTY ? "" : value);

const isGroup = (item: SelectOption | SelectGroup): item is SelectGroup => "options" in item;

interface ChoiceProps extends SelectBaseProps {
  indicator: "stepper" | "chevron";
  multiple: boolean;
  selected: readonly string[];
  onSelect: (values: string[]) => void;
}

/**
 * Accessible listbox that replaces the native `<select>`: a combobox button with an overlay
 * listbox on wide screens and a bottom sheet on phones (ui.css). Base UI owns keyboard (arrows,
 * Home/End, type-ahead, Enter, Escape), focus return and scrolling the selection into view.
 */
function Choice({
  options,
  id,
  name,
  "aria-label": ariaLabel,
  "aria-labelledby": labelledBy,
  "aria-describedby": describedBy,
  title,
  placeholder,
  summary,
  disabled,
  variant = "field",
  className,
  indicator,
  multiple,
  selected,
  onSelect,
}: ChoiceProps) {
  const flat = options.flatMap((item) => (isGroup(item) ? item.options : [item]));
  const chosen = flat.filter((option) => selected.includes(option.value));
  const lead = chosen.length === 1 ? chosen[0] : undefined;
  // Aside reads a multi-choice as a plain comma list ("Chats, Bookmarks"), not "A and B".
  const text = summary ?? (chosen.length > 0 ? formatList(chosen.map((option) => option.label), { type: "unit", style: "short" }) : placeholder);

  const renderOption = (option: SelectOption) => (
    <BaseSelect.Item key={option.value} value={encode(option.value)} label={option.label} disabled={option.disabled} className="ui-menu-item ui-select-item">
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
        <Check strokeWidth={ICON_STROKE} />
      </BaseSelect.ItemIndicator>
    </BaseSelect.Item>
  );

  const Indicator = indicator === "stepper" ? ChevronsUpDown : ChevronDown;
  const encoded = chosen.map((option) => encode(option.value));
  return (
    <BaseSelect.Root
      id={id}
      name={name}
      disabled={disabled}
      multiple={multiple}
      value={multiple ? encoded : (encoded[0] ?? null)}
      onValueChange={(next: string | string[] | null) => {
        if (next === null) return;
        onSelect((Array.isArray(next) ? next : [next]).map(decode));
      }}
    >
      <BaseSelect.Trigger
        className={cx("ui-select", `ui-select--${variant}`, className)}
        aria-label={ariaLabel}
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        title={title}
      >
        <span className="ui-select-value" data-placeholder={chosen.length > 0 ? undefined : ""}>
          {lead?.icon && (
            <span className="ui-select-icon" aria-hidden="true">
              {lead.icon}
            </span>
          )}
          <BaseSelect.Value className="ui-select-label">{() => text}</BaseSelect.Value>
        </span>
        <BaseSelect.Icon className="ui-select-chevron">
          <Indicator strokeWidth={ICON_STROKE} aria-hidden="true" />
        </BaseSelect.Icon>
      </BaseSelect.Trigger>
      <BaseSelect.Portal>
        <BaseSelect.Backdrop className="ui-select-backdrop" />
        <BaseSelect.Positioner
          className="ui-select-positioner"
          side="bottom"
          align="start"
          sideOffset={6}
          collisionPadding={8}
          // On a mouse Aside's listbox overlaps the trigger (macOS style); the phone bottom sheet
          // in ui.css pins the positioner and cannot share its inline geometry with that mode.
          alignItemWithTrigger={!multiple && window.matchMedia?.("(min-width: 821px)").matches === true}
        >
          <BaseSelect.Popup className="ui-select-popup surface-overlay">
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

/** Aside's select: the value and a ⌃⌄ stepper (Theme, Zoom level, Default search engine). */
export function Select({ value, onChange, ...props }: SelectProps) {
  return <Choice {...props} indicator="stepper" multiple={false} selected={[value]} onSelect={(values) => values[0] !== undefined && onChange(values[0])} />;
}

/**
 * The same box with a ⌄ chevron (Customize sidebar, Task notifications). With `multiple` the
 * list keeps its popup open and the trigger reads the chosen labels as a list ("Chats, Bookmarks").
 */
export function Dropdown(props: DropdownProps) {
  if (props.multiple) {
    const { value, onChange, ...rest } = props;
    return <Choice {...rest} indicator="chevron" multiple selected={value} onSelect={onChange} />;
  }
  const { value, onChange, ...rest } = props;
  return <Choice {...rest} indicator="chevron" multiple={false} selected={[value]} onSelect={(values) => values[0] !== undefined && onChange(values[0])} />;
}
