import { ContextMenu as BaseContextMenu } from "@base-ui/react/context-menu";
import { Menu as BaseMenu } from "@base-ui/react/menu";
import { Tooltip } from "@base-ui/react/tooltip";
import { Check, ChevronRight, Search } from "lucide-react";
import { useId, useRef, type ReactElement, type ReactNode } from "react";
import { cx, ICON_STROKE } from "./Button";

export interface PopoverMenuProps {
  /** The trigger element, usually an `IconButton`; Base UI merges its props and ref. */
  trigger: ReactElement;
  /** Accessible name of the menu popup. */
  label: string;
  /** Optional heading shown above the items (the session title in session menus). */
  title?: string;
  align?: "start" | "center" | "end";
  side?: "top" | "bottom";
  /** Gap to the trigger; Aside opens popovers 5–6px away, the account menu 1px above its button. */
  sideOffset?: number;
  /**
   * A search row on top that filters the items by their text (model picker, `L 15`). Items move
   * into a list under it; `empty` shows when nothing matches.
   */
  search?: { placeholder: string; empty: string };
  /** Controlled open state, for callers that open the menu from elsewhere. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Extra class on the popup, for a menu with a fixed width (model picker 194px). */
  popupClassName?: string;
  children: ReactNode;
}

/** Aside popover: Base UI owns arrows, Home/End, type-ahead, Enter, Escape and focus return. */
export function PopoverMenu({ trigger, label, title, align = "end", side = "bottom", sideOffset = 6, search, open, onOpenChange, popupClassName, children }: PopoverMenuProps) {
  const root = (
    <BaseMenu.Root open={open} onOpenChange={onOpenChange}>
      <BaseMenu.Trigger render={trigger} />
      <BaseMenu.Portal>
        <BaseMenu.Positioner className="ui-menu-positioner" side={side} align={align} sideOffset={sideOffset} collisionPadding={8}>
          <BaseMenu.Popup className={cx("ui-menu surface-overlay", popupClassName)} aria-label={label}>
            {title && (
              <div className="ui-menu-title" aria-hidden="true">
                {title}
              </div>
            )}
            {search ? (
              <>
                <label className="ui-menu-search">
                  <Search strokeWidth={ICON_STROKE} aria-hidden="true" />
                  <BaseMenu.Input placeholder={search.placeholder} aria-label={search.placeholder} />
                </label>
                <BaseMenu.List className="ui-menu-list">{children}</BaseMenu.List>
                <BaseMenu.Empty className="ui-menu-empty">{search.empty}</BaseMenu.Empty>
              </>
            ) : (
              children
            )}
          </BaseMenu.Popup>
        </BaseMenu.Positioner>
      </BaseMenu.Portal>
    </BaseMenu.Root>
  );
  return search ? <BaseMenu.FilterProvider autoHighlight>{root}</BaseMenu.FilterProvider> : root;
}

export interface MenuItemProps {
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  /**
   * Picker rows: `true` marks the current choice with a ✓; any boolean makes the row a
   * `menuitemradio` with `aria-checked`. Leave undefined for plain commands.
   */
  checked?: boolean;
  /** `"checkbox"` makes a `checked` row a `menuitemcheckbox` (an on/off state, not a choice among rows). */
  checkRole?: "radio" | "checkbox";
  /** Text at the row's end: a value ("Guard"), a shortcut, or a ›/↗ glyph. */
  trailing?: ReactNode;
  /** Text the search row matches when `children` is not plain text. */
  textValue?: string;
  /** Leaves the menu open after the click (submenu-like rows, toggles). */
  keepOpen?: boolean;
  /** Omitted only on disabled placeholder rows ("Loading…", "Nothing here yet"). */
  onClick?: () => void;
  /**
   * Why the row is the way it is (usually why it is disabled): a tooltip on hover and keyboard
   * focus, and the row's accessible description.
   */
  description?: string;
  children: ReactNode;
}

export function MenuItem({ icon, danger, disabled, checked, checkRole = "radio", trailing, textValue, keepOpen, onClick, description, children }: MenuItemProps) {
  const descriptionId = useId();
  const item = (
    <BaseMenu.Item
      className={cx("ui-menu-item", danger && "ui-menu-item--danger")}
      disabled={disabled}
      label={textValue ?? (typeof children === "string" ? children : undefined)}
      {...(checked === undefined ? {} : { role: checkRole === "checkbox" ? "menuitemcheckbox" : "menuitemradio", "aria-checked": checked })}
      {...(description ? { "aria-describedby": descriptionId } : {})}
      closeOnClick={!keepOpen}
      onClick={onClick}
    >
      {icon && (
        <span className="ui-menu-icon" aria-hidden="true">
          {icon}
        </span>
      )}
      <span className="ui-menu-label">{children}</span>
      {(trailing || checked) && (
        <span className="ui-menu-trailing">
          {trailing}
          {checked && <Check className="ui-menu-check" strokeWidth={ICON_STROKE} aria-hidden="true" />}
        </span>
      )}
    </BaseMenu.Item>
  );
  if (!description) return item;
  // The description sits beside the row, not inside it, so it never becomes part of the row's name.
  return (
    <>
      <Tooltip.Root>
        <Tooltip.Trigger render={item} delay={300} />
        <Tooltip.Portal>
          <Tooltip.Positioner side="right" sideOffset={8} collisionPadding={8}>
            <Tooltip.Popup className="ui-tooltip" aria-hidden="true">
              {description}
            </Tooltip.Popup>
          </Tooltip.Positioner>
        </Tooltip.Portal>
      </Tooltip.Root>
      <span id={descriptionId} hidden>
        {description}
      </span>
    </>
  );
}

/** A non-interactive group header ("Projects", "Profiles") over the rows that follow. */
export function MenuGroupLabel({ children }: { children: ReactNode }) {
  return (
    <div className="ui-menu-title" role="presentation">
      {children}
    </div>
  );
}

export function MenuSeparator() {
  return <BaseMenu.Separator className="ui-menu-separator" />;
}

export interface ContextMenuProps {
  /** The element that answers right-click, long-press and the context-menu key; Base UI merges its props. */
  trigger: ReactElement;
  label: string;
  /** Controlled open state, for opening the menu from the keyboard (Shift+F10). */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  popupClassName?: string;
  children: ReactNode;
}

/**
 * The same popover as `PopoverMenu`, opened at the pointer (sidebar chat and tab rows, `L 33`/`L 34`).
 * Closing returns focus to the control that had it when the menu opened (the row's button), since
 * the trigger is often a non-focusable list item.
 */
export function ContextMenu({ trigger, label, open, onOpenChange, popupClassName, children }: ContextMenuProps) {
  const invoker = useRef<HTMLElement | null>(null);
  return (
    <BaseContextMenu.Root
      open={open}
      onOpenChange={(next) => {
        if (next) invoker.current = document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? document.activeElement : null;
        onOpenChange?.(next);
      }}
    >
      <BaseContextMenu.Trigger render={trigger} />
      <BaseContextMenu.Portal>
        <BaseContextMenu.Positioner className="ui-menu-positioner" collisionPadding={8}>
          <BaseContextMenu.Popup className={cx("ui-menu surface-overlay", popupClassName)} aria-label={label} finalFocus={() => (invoker.current?.isConnected ? invoker.current : true)}>
            {children}
          </BaseContextMenu.Popup>
        </BaseContextMenu.Positioner>
      </BaseContextMenu.Portal>
    </BaseContextMenu.Root>
  );
}

export interface SubmenuItemProps {
  icon?: ReactNode;
  label: string;
  disabled?: boolean;
  /** The current choice shown before the › ("Guard" on the composer's Permission row, `L 14`). */
  value?: ReactNode;
  /** Replaces the trailing › (a profile row's …). */
  indicator?: ReactNode;
  /** Called when the submenu opens, so its rows can load then (recent history, downloads). */
  onOpen?: () => void;
  popupClassName?: string;
  children: ReactNode;
}

/** A row with a trailing › that opens its own popover beside the menu (account menu `L 40`). */
export function SubmenuItem({ icon, label, disabled, value, indicator, onOpen, popupClassName, children }: SubmenuItemProps) {
  return (
    <BaseMenu.SubmenuRoot onOpenChange={(open) => open && onOpen?.()}>
      <BaseMenu.SubmenuTrigger className="ui-menu-item" disabled={disabled} label={label}>
        {icon && (
          <span className="ui-menu-icon" aria-hidden="true">
            {icon}
          </span>
        )}
        <span className="ui-menu-label">{label}</span>
        <span className="ui-menu-trailing">
          {value}
          {indicator ?? <ChevronRight strokeWidth={ICON_STROKE} aria-hidden="true" />}
        </span>
      </BaseMenu.SubmenuTrigger>
      <BaseMenu.Portal>
        <BaseMenu.Positioner className="ui-menu-positioner" side="right" align="start" sideOffset={2} alignOffset={-4} collisionPadding={8}>
          <BaseMenu.Popup className={cx("ui-menu surface-overlay", popupClassName)} aria-label={label}>
            {children}
          </BaseMenu.Popup>
        </BaseMenu.Positioner>
      </BaseMenu.Portal>
    </BaseMenu.SubmenuRoot>
  );
}
