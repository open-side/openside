import { Ellipsis } from "lucide-react";
import type { ReactNode } from "react";
import { ICON_STROKE } from "./Button";
import { PopoverMenu } from "./Menu";
import { Dropdown, type SelectGroup, type SelectOption } from "./Select";

export interface SplitSelectProps {
  value: string;
  onChange: (value: string) => void;
  options: readonly (SelectOption | SelectGroup)[];
  /** Accessible name of the choice ("Default model"). */
  "aria-label"?: string;
  "aria-labelledby"?: string;
  placeholder?: string;
  disabled?: boolean;
  /**
   * Rows of the "…" segment (`MenuItem`s). Without them the box is the single variant and has no
   * second segment.
   */
  more?: { label: string; items: ReactNode };
}

/**
 * Aside's model select (`L 48`): one 176px box holding the choice (icon, label, light ⌄) and,
 * after a hairline, a 32px "…" segment with the row's other actions.
 */
export function SplitSelect({ value, onChange, options, "aria-label": ariaLabel, "aria-labelledby": labelledBy, placeholder, disabled, more }: SplitSelectProps) {
  return (
    <span className="ui-split-select" data-disabled={disabled ? "" : undefined}>
      <Dropdown
        value={value}
        onChange={onChange}
        options={options}
        aria-label={ariaLabel}
        aria-labelledby={labelledBy}
        placeholder={placeholder}
        disabled={disabled}
        variant="inline"
        className="ui-split-select-choice"
      />
      {more && (
        <PopoverMenu
          label={more.label}
          align="end"
          trigger={
            <button type="button" className="ui-split-select-more" aria-label={more.label} title={more.label} disabled={disabled}>
              <Ellipsis strokeWidth={ICON_STROKE} aria-hidden="true" />
            </button>
          }
        >
          {more.items}
        </PopoverMenu>
      )}
    </span>
  );
}
