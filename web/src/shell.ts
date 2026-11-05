import { createContext, useContext } from "react";

/** What screens inside the app shell can ask of it. */
export interface Shell {
  /** Opens the team drawer (phones; the sidebar is a column on desktop). */
  openDrawer: () => void;
  /** Reloads the sidebar conversation list after a create, rename, pin or archive. */
  refreshSessions: () => void;
  /** Desktop: the sidebar column is folded away (`localStorage["openside.sidebar"]`). */
  sidebarCollapsed: boolean;
  toggleSidebar: () => void;
}

// No-op defaults keep screens renderable on their own in unit tests.
export const ShellContext = createContext<Shell>({
  openDrawer: () => undefined,
  refreshSessions: () => undefined,
  sidebarCollapsed: false,
  toggleSidebar: () => undefined,
});

export const useShell = () => useContext(ShellContext);
