import { Menu, PanelLeftOpen, SquarePen } from "lucide-react";
import type { ReactNode } from "react";
import { useNavigate } from "react-router";
import { useShell } from "../shell";
import { cx, ICON_STROKE, IconButton } from "../ui";

const icon = { size: 20, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

export interface AppHeaderProps {
  title?: string;
  subtitle?: ReactNode;
  /** Icon buttons on the right (the thread menu). */
  actions?: ReactNode;
  titleClassName?: string;
  /** Replaces the plain title, e.g. a person's avatar and name that opens their profile. */
  heading?: ReactNode;
  /** Phone only: a New task button before `actions`. Off on Home, where New task leads. */
  newTask?: boolean;
}

/** The top bar of every screen inside the shell: 56px on desktop, 64px on a phone. */
export function AppHeader({ title, subtitle, actions, titleClassName, heading, newTask = true }: AppHeaderProps) {
  const shell = useShell();
  const navigate = useNavigate();
  return (
    <header className="app-header">
      <div className="app-header-start">
        <IconButton label="Open menu" className="drawer-only" onClick={shell.openDrawer}>
          <Menu {...icon} />
        </IconButton>
        {shell.sidebarCollapsed ? (
          <IconButton label="Open sidebar" className="desktop-only" onClick={shell.toggleSidebar}>
            <PanelLeftOpen {...icon} />
          </IconButton>
        ) : null}
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
      <div className="app-header-actions">
        {newTask ? (
          <IconButton label="New task" className="drawer-only" onClick={() => navigate("/")}>
            <SquarePen {...icon} />
          </IconButton>
        ) : null}
        {actions}
      </div>
    </header>
  );
}
