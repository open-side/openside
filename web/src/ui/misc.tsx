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

export function Segmented<T extends string>({ label, options, value, onChange, kind = "radio" }: SegmentedProps<T>) {
  return (
    <div className="ui-segmented" role={kind === "tab" ? "tablist" : "radiogroup"} aria-label={label}>
      {options.map((option) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role={kind}
            aria-selected={kind === "tab" ? selected : undefined}
            aria-checked={kind === "radio" ? selected : undefined}
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
