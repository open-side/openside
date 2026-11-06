import type { ComponentType } from "react";
import type { SettingsPageId } from "./nav";
import { AccountSettings } from "./pages/Account";
import { AgentsSettings } from "./pages/Agents";
import { AppearanceSettings } from "./pages/Appearance";
import { ArchivedChats } from "./pages/Archived";
import { ChannelsSettings } from "./pages/Channels";
import { ContextAwareness } from "./pages/ContextAwareness";
import { DevelopersSettings } from "./pages/Developers";
import { GeneralSettings } from "./pages/General";
import { MemoryOverview } from "./pages/Memory";
import { MemoryConfigure } from "./pages/MemoryConfigure";
import { MemoryHistory } from "./pages/MemoryHistory";
import { LassoSettingsPage } from "./pages/Lasso";
import { MiniPopupSettings } from "./pages/MiniPopup";
import { PasswordsSettings } from "./pages/Passwords";
import { ModelsSettings } from "./pages/Models";
import { PluginsSettings } from "./pages/Plugins";
import { ProjectsSettings } from "./pages/Projects";
import { RoutinesSettings } from "./pages/Routines";
import { SecuritySettings } from "./pages/Security";

export const SETTINGS_BODY: Record<SettingsPageId, ComponentType<{ onSignOut: () => void }>> = {
  general: GeneralSettings,
  appearance: AppearanceSettings,
  account: AccountSettings,
  security: SecuritySettings,
  agents: AgentsSettings,
  projects: ProjectsSettings,
  models: ModelsSettings,
  plugins: PluginsSettings,
  memory: MemoryOverview,
  "memory/history": MemoryHistory,
  "memory/context-awareness": ContextAwareness,
  "memory/configure": MemoryConfigure,
  passwords: PasswordsSettings,
  routines: RoutinesSettings,
  channels: ChannelsSettings,
  developers: DevelopersSettings,
  "mini-popup": MiniPopupSettings,
  lasso: LassoSettingsPage,
  archived: ArchivedChats,
};
