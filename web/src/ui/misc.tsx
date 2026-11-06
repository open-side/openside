import type { KeyboardEvent, ReactNode } from "react";
import { cx } from "./Button";

/** Accent spinner; decorative unless a label is given. */
export function Spinner({ label, size = 16 }: { label?: string; size?: number }) {
  return (
    <span
      className="ui-spinner"
      style={{ width: size, height: size }}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    />
  );
}

export interface SegmentOption<T extends string> {
  value: T;
  label: string;
}

export interface SegmentedProps<T extends string> {
  /** Accessible name of the group. */
  label: string;
  options: SegmentOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** `tab` filters a list in place; `radio` picks a setting. */
  kind?: "tab" | "radio";
  /**
   * Tabs only: the id of the one panel showing the chosen tab's content. Each tab gets the id
   * `${panel}-${value}` (for the panel's `aria-labelledby`), and the chosen one `aria-controls`.
   */
  panel?: string;
}

const STEP: Record<string, number> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };

/**
 * Roving choice for radio groups and tab lists: arrows wrap, Home/End jump. Moves the choice and
 * the focus (the `itemSelector` child at the new index) together, as both ARIA patterns ask.
 */
export function rovingChoice<T>(event: KeyboardEvent<HTMLElement>, items: readonly T[], current: number, itemSelector: string, choose: (item: T) => void): void {
  const step = STEP[event.key];
  const next = step !== undefined ? (current + step + items.length) % items.length : event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : null;
  const target = next === null ? undefined : items[next];
  if (next === null || target === undefined) return;
  event.preventDefault();
  choose(target);
  event.currentTarget.querySelectorAll<HTMLElement>(itemSelector)[next]?.focus();
}

/**
 * One choice out of a few, as a radio group or a tab list. Tab enters at the chosen segment only;
 * arrows (wrapping), Home and End move the choice and the focus together, as both ARIA patterns ask.
 */
export function Segmented<T extends string>({ label, options, value, onChange, kind = "radio", panel }: SegmentedProps<T>) {
  const chosen = Math.max(
    0,
    options.findIndex((option) => option.value === value),
  );
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => rovingChoice(event, options, chosen, ".ui-segment", (option) => onChange(option.value));
  return (
    <div className="ui-segmented" role={kind === "tab" ? "tablist" : "radiogroup"} aria-label={label} onKeyDown={onKeyDown}>
      {options.map((option, index) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role={kind}
            id={kind === "tab" && panel ? `${panel}-${option.value}` : undefined}
            aria-controls={kind === "tab" && panel && selected ? panel : undefined}
            aria-selected={kind === "tab" ? selected : undefined}
            aria-checked={kind === "radio" ? selected : undefined}
            tabIndex={index === chosen ? 0 : -1}
            className={cx("ui-segment", selected && "ui-segment--selected")}
            onClick={() => onChange(option.value)}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

export interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** Accessible name when no `<label htmlFor>` names it. */
  label?: string;
  id?: string;
  disabled?: boolean;
  "aria-labelledby"?: string;
  "aria-describedby"?: string;
}

/** Aside's toggle: a native `role="switch"` checkbox over a drawn 40×20 track, with a 44px hit box. */
export function Switch({ checked, onChange, label, id, disabled, "aria-labelledby": labelledBy, "aria-describedby": describedBy }: SwitchProps) {
  return (
    <span className="ui-switch">
      <input
        type="checkbox"
        role="switch"
        id={id}
        aria-label={label}
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="ui-switch-track" aria-hidden="true" />
    </span>
  );
}

export interface ChipProps {
  children: ReactNode;
  icon?: ReactNode;
  /** Without a handler the chip is a plain label (`<span>`), not a control. */
  onClick?: () => void;
  /** Toggle state for filter chips; rendered as `aria-pressed`. */
  pressed?: boolean;
  title?: string;
  "aria-label"?: string;
}

/** Quiet 32px pill: model picker, filters, context tags. */
export function Chip({ children, icon, onClick, pressed, title, "aria-label": ariaLabel }: ChipProps) {
  const content = (
    <>
      {icon}
      <span className="ui-chip-label">{children}</span>
    </>
  );
  if (!onClick) {
    return (
      <span className="ui-chip" title={title} aria-label={ariaLabel}>
        {content}
      </span>
    );
  }
  return (
    <button type="button" className="ui-chip" title={title} aria-label={ariaLabel} aria-pressed={pressed} onClick={onClick}>
      {content}
    </button>
  );
}

/**
 * Aside's shortcut key cap (Appearance › Keyboard shortcuts): 26px, the glyphs of one shortcut
 * in one cap. `unset` is the unfilled cap of a shortcut that is not assigned or not active.
 */
export function KbdChip({ keys, unset, label }: { keys: readonly string[]; unset?: boolean; label?: string }) {
  const cap = (
    <kbd className={cx("ui-kbd", unset && "ui-kbd--unset")} aria-hidden={label ? true : undefined}>
      {keys.map((key, index) => (
        <kbd key={index}>{key}</kbd>
      ))}
    </kbd>
  );
  if (!label) return cap;
  // aria-label is not announced on <kbd> (a generic role), so the spoken name is hidden text.
  return (
    <>
      {cap}
      <span className="sr-only">{label}</span>
    </>
  );
}

/** Small pill after a label: `neutral` ("This device", "Default"), `new` (sky "New"). */
export function Badge({ tone = "neutral", children }: { tone?: "neutral" | "new"; children: ReactNode }) {
  return <span className={cx("ui-badge", tone === "new" && "ui-badge--new")}>{children}</span>;
}
