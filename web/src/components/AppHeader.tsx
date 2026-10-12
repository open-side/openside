import { Menu, SquarePen } from "lucide-react";
import type { ReactNode } from "react";
import { useNavigate } from "react-router";
import { useShell } from "../shell";
import { cx, ICON_STROKE, IconButton } from "../ui";

export interface AppHeaderProps {
  /** Omitted on the new-chat screen, which has nothing to name yet. */
  title?: string;
  subtitle?: ReactNode;
  /** Extra controls placed after New chat in the glass pill (the session menu). */
  actions?: ReactNode;
  titleClassName?: string;
  /** Off where the screen already is a new chat. */
  newChat?: boolean;
}

/** The 64px top bar of every screen inside the shell. */
export function AppHeader({ title, subtitle, actions, titleClassName, newChat = true }: AppHeaderProps) {
  const shell = useShell();
  const navigate = useNavigate();
  // The pinned sidebar has its own New chat, so the header offers it only beside the drawer.
  const newChatButton = newChat ? (
    <IconButton label="New chat" className="drawer-only" onClick={() => navigate("/")}>
      <SquarePen size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </IconButton>
  ) : null;
  return (
    <header className="app-header">
      <div className="app-header-start">
        <span className="glass-pill drawer-only">
          <IconButton label="Open conversations" onClick={shell.openDrawer}>
            <Menu size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        </span>
        {title ? (
          <div className="app-header-heading">
            <h1 className={cx("app-title", titleClassName)} title={title}>
              {title}
            </h1>
            {subtitle && <p className="app-subtitle">{subtitle}</p>}
          </div>
        ) : null}
      </div>
      {actions ? (
        <div className="glass-pill app-header-actions">
          {newChatButton}
          {actions}
        </div>
      ) : newChatButton ? (
        <div className="glass-pill app-header-actions drawer-only">{newChatButton}</div>
      ) : null}
    </header>
  );
}
