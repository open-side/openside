import { BookOpen, Brain, CalendarClock, KeyRound, MonitorSmartphone, UserRound, Wrench } from "lucide-react";
import type { Location } from "react-router";

/** Workspace sections; each keeps its own deep-link route and opens inside the Workspace dialog. */
export const WORKSPACE_SECTIONS = [
  { path: "/skills", title: "Skills", description: "Create, edit and enable skills", Icon: BookOpen },
  { path: "/routines", title: "Routines", description: "Scheduled prompts and run history", Icon: CalendarClock },
  { path: "/memory", title: "Memory", description: "Saved context and retention", Icon: Brain },
  { path: "/accounts", title: "Accounts", description: "Model accounts, usage and rate limits", Icon: UserRound },
  { path: "/vault", title: "Vault", description: "Item use policy and audit log", Icon: KeyRound },
  { path: "/devices", title: "Devices", description: "Paired devices and pairing", Icon: MonitorSmartphone },
  { path: "/config", title: "Configuration", description: "Every daemon setting from config.toml", Icon: Wrench },
] as const;

export type WorkspacePath = (typeof WORKSPACE_SECTIONS)[number]["path"];

export type ManageTarget = { kind: "settings" } | { kind: "workspace"; section?: WorkspacePath };

export function manageTarget(pathname: string): ManageTarget | null {
  const path = pathname.replace(/\/+$/, "") || "/";
  if (path === "/settings") return { kind: "settings" };
  if (path === "/workspace") return { kind: "workspace" };
  const section = WORKSPACE_SECTIONS.find((item) => item.path === path);
  return section ? { kind: "workspace", section: section.path } : null;
}

export interface ManageState {
  /** The screen the dialog opened over; it stays rendered underneath. */
  background: Location;
}

export function backgroundOf(location: Location): Location | undefined {
  const state = location.state as Partial<ManageState> | null;
  return state?.background;
}

/**
 * Router state for links that open Settings or Workspace. Opening one from inside another keeps
 * the original screen underneath instead of stacking dialogs on dialogs.
 */
export function manageLinkState(location: Location): ManageState {
  return { background: backgroundOf(location) ?? location };
}
