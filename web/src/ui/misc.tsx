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
}

const STEP: Record<string, number> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };

/**
 * One choice out of a few, as a radio group or a tab list. Tab enters at the chosen segment only;
 * arrows (wrapping), Home and End move the choice and the focus together, as both ARIA patterns ask.
 */
export function Segmented<T extends string>({ label, options, value, onChange, kind = "radio" }: SegmentedProps<T>) {
  const chosen = Math.max(
    0,
    options.findIndex((option) => option.value === value),
  );
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = STEP[event.key];
    const next = step !== undefined ? (chosen + step + options.length) % options.length : event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : null;
    const target = next === null ? undefined : options[next];
    if (next === null || !target) return;
    event.preventDefault();
    onChange(target.value);
    event.currentTarget.querySelectorAll<HTMLButtonElement>(".ui-segment")[next]?.focus();
  };
  return (
    <div className="ui-segmented" role={kind === "tab" ? "tablist" : "radiogroup"} aria-label={label} onKeyDown={onKeyDown}>
      {options.map((option, index) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role={kind}
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
  "aria-describedby"?: string;
}

/** On/off toggle: a native `role="switch"` checkbox over a drawn track, with a 44px hit box. */
export function Switch({ checked, onChange, label, id, disabled, "aria-describedby": describedBy }: SwitchProps) {
  return (
    <span className="ui-switch">
      <input
        type="checkbox"
        role="switch"
        id={id}
        aria-label={label}
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
