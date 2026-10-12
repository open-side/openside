import { createContext, useContext } from "react";

/** What screens inside the app shell can ask of it. */
export interface Shell {
  /** Opens the conversation drawer (phones; the sidebar is always visible on desktop). */
  openDrawer: () => void;
  /** Reloads the sidebar conversation list after a create, rename, pin or archive. */
  refreshSessions: () => void;
}

// No-op defaults keep screens renderable on their own in unit tests.
export const ShellContext = createContext<Shell>({ openDrawer: () => undefined, refreshSessions: () => undefined });

export const useShell = () => useContext(ShellContext);
