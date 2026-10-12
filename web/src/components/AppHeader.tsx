import { Menu, SquarePen } from "lucide-react";
import type { ReactNode } from "react";
import { useNavigate } from "react-router";
import { useShell } from "../shell";
import { cx, ICON_STROKE, IconButton } from "../ui";

export interface AppHeaderProps {
  title: string;
  subtitle?: ReactNode;
  /** Extra controls placed after New chat in the glass pill (the session menu). */
  actions?: ReactNode;
  titleClassName?: string;
}

/** The 64px top bar of every screen inside the shell. */
export function AppHeader({ title, subtitle, actions, titleClassName }: AppHeaderProps) {
  const shell = useShell();
  const navigate = useNavigate();
  return (
    <header className="app-header">
      <div className="app-header-start">
        <span className="glass-pill drawer-only">
          <IconButton label="Open conversations" onClick={shell.openDrawer}>
            <Menu size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        </span>
        <div className="app-header-heading">
          <h1 className={cx("app-title", titleClassName)}>{title}</h1>
          {subtitle && <p className="app-subtitle">{subtitle}</p>}
        </div>
      </div>
      <div className="glass-pill app-header-actions">
        <IconButton label="New chat" onClick={() => navigate("/")}>
          <SquarePen size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
        {actions}
      </div>
    </header>
  );
}
