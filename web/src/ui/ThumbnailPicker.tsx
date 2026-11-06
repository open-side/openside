import type { CSSProperties, ReactNode } from "react";
import { cx } from "./Button";
import { rovingChoice } from "./misc";

export interface ThumbnailOption<T extends string> {
  value: T;
  label: string;
  /** The 70×43 picture, drawn by the caller (an `<img>` or inline SVG). */
  thumbnail: ReactNode;
  /** Colour of the selection ring; Aside tints it with the thumbnail's own accent. */
  accent?: string;
}

export interface ThumbnailPickerProps<T extends string> {
  /** Accessible name of the group (the row's title). */
  label?: string;
  "aria-labelledby"?: string;
  options: readonly ThumbnailOption<T>[];
  value: T;
  onChange: (value: T) => void;
}

/**
 * Aside's picture choice (Appearance › Tab style, `L 42`): 70×43 thumbnails with a caption; the
 * chosen one has a 3px ring behind a 1px gap and a bold caption. A radio group with roving focus.
 */
export function ThumbnailPicker<T extends string>({ label, "aria-labelledby": labelledBy, options, value, onChange }: ThumbnailPickerProps<T>) {
  const chosen = Math.max(
    0,
    options.findIndex((option) => option.value === value),
  );
  return (
    <div
      className="ui-thumbs"
      role="radiogroup"
      aria-label={label}
      aria-labelledby={labelledBy}
      onKeyDown={(event) => rovingChoice(event, options, chosen, ".ui-thumb", (option) => onChange(option.value))}
    >
      {options.map((option, index) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={index === chosen ? 0 : -1}
            className={cx("ui-thumb", selected && "ui-thumb--selected")}
            style={option.accent ? ({ "--thumb-accent": option.accent } as CSSProperties) : undefined}
            onClick={() => onChange(option.value)}
          >
            <span className="ui-thumb-image" aria-hidden="true">
              {option.thumbnail}
            </span>
            <span className="ui-thumb-label">{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}
