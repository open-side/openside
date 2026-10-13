import { Menu } from "lucide-react";
import type { ReactNode } from "react";
import { useShell } from "../shell";
import { cx, ICON_STROKE, IconButton } from "../ui";

export interface AppHeaderProps {
  title?: string;
  subtitle?: ReactNode;
  /** Controls in the glass pill on the right (the thread menu). */
  actions?: ReactNode;
  titleClassName?: string;
  /** Replaces the plain title pill, e.g. a person's avatar and name that opens their profile. */
  heading?: ReactNode;
}

/** The 64px top bar of every screen inside the shell. */
export function AppHeader({ title, subtitle, actions, titleClassName, heading }: AppHeaderProps) {
  const shell = useShell();
  return (
    <header className="app-header">
      <div className="app-header-start">
        <span className="glass-pill drawer-only">
          <IconButton label="Open menu" onClick={shell.openDrawer}>
            <Menu size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        </span>
        {heading ??
          (title ? (
            <div className="app-header-heading">
              <h1 className={cx("app-title", titleClassName)} title={title}>
                {title}
              </h1>
              {subtitle && <p className="app-subtitle">{subtitle}</p>}
            </div>
          ) : null)}
      </div>
      {actions ? <div className="glass-pill app-header-actions">{actions}</div> : null}
    </header>
  );
}
