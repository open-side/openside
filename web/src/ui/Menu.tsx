import { Menu as BaseMenu } from "@base-ui/react/menu";
import type { ReactElement, ReactNode } from "react";
import { cx } from "./Button";

export interface PopoverMenuProps {
  /** The trigger element, usually an `IconButton`; Base UI merges its props and ref. */
  trigger: ReactElement;
  /** Accessible name of the menu popup. */
  label: string;
  /** Optional heading shown above the items (the session title in session menus). */
  title?: string;
  align?: "start" | "center" | "end";
  side?: "top" | "bottom";
  children: ReactNode;
}

export function PopoverMenu({ trigger, label, title, align = "end", side = "bottom", children }: PopoverMenuProps) {
  return (
    <BaseMenu.Root>
      <BaseMenu.Trigger render={trigger} />
      <BaseMenu.Portal>
        <BaseMenu.Positioner className="ui-menu-positioner" side={side} align={align} sideOffset={8} collisionPadding={8}>
          <BaseMenu.Popup className="ui-menu" aria-label={label}>
            {title && (
              <div className="ui-menu-title" aria-hidden="true">
                {title}
              </div>
            )}
            {children}
          </BaseMenu.Popup>
        </BaseMenu.Positioner>
      </BaseMenu.Portal>
    </BaseMenu.Root>
  );
}

export interface MenuItemProps {
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onClick: () => void;
  children: ReactNode;
}

export function MenuItem({ icon, danger, disabled, onClick, children }: MenuItemProps) {
  return (
    <BaseMenu.Item className={cx("ui-menu-item", danger && "ui-menu-item--danger")} disabled={disabled} onClick={onClick}>
      {icon && (
        <span className="ui-menu-icon" aria-hidden="true">
          {icon}
        </span>
      )}
      <span>{children}</span>
    </BaseMenu.Item>
  );
}

export function MenuSeparator() {
  return <BaseMenu.Separator className="ui-menu-separator" />;
}
