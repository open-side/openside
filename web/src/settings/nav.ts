import {
  AppWindow,
  Archive,
  Box,
  Brain,
  CircleUserRound,
  Contrast,
  FileText,
  History,
  KeyRound,
  Lasso,
  Layers,
  LayoutGrid,
  Lightbulb,
  Lock,
  MessageCircle,
  MessagesSquare,
  Plug,
  Puzzle,
  Repeat,
  Settings,
  SlidersHorizontal,
  Sparkle,
  SquareTerminal,
  Users,
  type LucideIcon,
} from "lucide-react";
import { t, type MessageKey } from "../i18n";

/** Every Settings page, by its path under `/settings/`. */
export const SETTINGS_PAGES = [
  "general",
  "appearance",
  "account",
  "security",
  "agents",
  "projects",
  "models",
  "plugins",
  "memory",
  "memory/history",
  "memory/context-awareness",
  "memory/configure",
  "passwords",
  "routines",
  "channels",
  "developers",
  "mini-popup",
  "lasso",
  "archived",
] as const;

export type SettingsPageId = (typeof SETTINGS_PAGES)[number];

export const settingsPath = (page: SettingsPageId) => `/settings/${page}`;

export interface SettingsNavPage {
  page: SettingsPageId;
  title: MessageKey;
  /** Words people search for that are not in the title (the page's row titles). */
  keywords: MessageKey;
  Icon: LucideIcon;
  isNew?: boolean;
  /** Memory's pages; the group opens while one of them is shown. */
  children?: readonly SettingsNavPage[];
  /** Context Awareness at the top level stands in for Memory's child while Memory is closed. */
  shortcut?: true;
}

export interface SettingsNavGroup {
  heading: MessageKey;
  pages: readonly SettingsNavPage[];
}

const CONTEXT_AWARENESS = {
  page: "memory/context-awareness",
  title: "settings.nav.contextAwareness.title",
  keywords: "settings.nav.contextAwareness.keywords",
  Icon: Lightbulb,
  isNew: true,
} as const satisfies SettingsNavPage;

/** Aside's Settings tree (`local://aside-measure-settings.md` §1), without Plan & Usage. */
export const SETTINGS_NAV: readonly SettingsNavGroup[] = [
  {
    heading: "settings.nav.group.personal",
    pages: [
      { page: "general", title: "settings.nav.general.title", keywords: "settings.nav.general.keywords", Icon: Settings },
      { page: "appearance", title: "settings.nav.appearance.title", keywords: "settings.nav.appearance.keywords", Icon: Contrast },
      { page: "account", title: "settings.nav.account.title", keywords: "settings.nav.account.keywords", Icon: CircleUserRound },
      { page: "security", title: "settings.nav.security.title", keywords: "settings.nav.security.keywords", Icon: Lock },
    ],
  },
  {
    heading: "settings.nav.group.agent",
    pages: [
      { page: "agents", title: "settings.nav.agents.title", keywords: "settings.nav.agents.keywords", Icon: Sparkle },
      { page: "projects", title: "settings.nav.projects.title", keywords: "settings.nav.projects.keywords", Icon: Layers },
      { page: "models", title: "settings.nav.models.title", keywords: "settings.nav.models.keywords", Icon: Box },
      { page: "plugins", title: "settings.nav.plugins.title", keywords: "settings.nav.plugins.keywords", Icon: Plug },
      {
        page: "memory",
        title: "settings.nav.memory.title",
        keywords: "settings.nav.memory.keywords",
        Icon: Brain,
        children: [
          { page: "memory", title: "settings.nav.memoryOverview.title", keywords: "settings.nav.memory.keywords", Icon: LayoutGrid },
          { page: "memory/history", title: "settings.nav.memoryHistory.title", keywords: "settings.nav.memoryHistory.keywords", Icon: History },
          CONTEXT_AWARENESS,
          { page: "memory/configure", title: "settings.nav.memoryConfigure.title", keywords: "settings.nav.memoryConfigure.keywords", Icon: SlidersHorizontal },
        ],
      },
      { ...CONTEXT_AWARENESS, shortcut: true },
    ],
  },
  {
    heading: "settings.nav.group.features",
    pages: [
      { page: "passwords", title: "settings.nav.passwords.title", keywords: "settings.nav.passwords.keywords", Icon: KeyRound },
      { page: "routines", title: "settings.nav.routines.title", keywords: "settings.nav.routines.keywords", Icon: Repeat },
      { page: "channels", title: "settings.nav.channels.title", keywords: "settings.nav.channels.keywords", Icon: MessagesSquare, isNew: true },
      { page: "developers", title: "settings.nav.developers.title", keywords: "settings.nav.developers.keywords", Icon: SquareTerminal },
      { page: "mini-popup", title: "settings.nav.miniPopup.title", keywords: "settings.nav.miniPopup.keywords", Icon: AppWindow },
      { page: "lasso", title: "settings.nav.lasso.title", keywords: "settings.nav.lasso.keywords", Icon: Lasso },
    ],
  },
  {
    heading: "settings.nav.group.archived",
    pages: [{ page: "archived", title: "settings.nav.archived.title", keywords: "settings.nav.archived.keywords", Icon: Archive }],
  },
];

export const REPOSITORY = "https://github.com/open-side/openside";

export interface SettingsFooterLink {
  key: string;
  title: MessageKey;
  Icon: LucideIcon;
  url: string;
  /** ↗: the link leaves Settings. Send feedback opens the issue form without the glyph, as in Aside. */
  external: boolean;
}

export const SETTINGS_FOOTER: readonly SettingsFooterLink[] = [
  { key: "feedback", title: "settings.nav.feedback.title", Icon: MessageCircle, url: `${REPOSITORY}/issues/new`, external: false },
  // A web page cannot open chrome:// itself; the person's browser opens it through the daemon.
  { key: "extensions", title: "settings.nav.extensions.title", Icon: Puzzle, url: "chrome://extensions", external: true },
  { key: "docs", title: "settings.nav.docs.title", Icon: FileText, url: `${REPOSITORY}/tree/master/docs`, external: true },
  { key: "community", title: "settings.nav.community.title", Icon: Users, url: `${REPOSITORY}/discussions`, external: true },
];

/** `/settings/<page>` → the page, or null for `/settings` itself and unknown pages. */
export function settingsPageOf(pathname: string): SettingsPageId | null {
  const rest = /^\/settings\/(.+?)\/*$/.exec(pathname)?.[1];
  return SETTINGS_PAGES.find((page) => page === rest) ?? null;
}

export const isMemoryPage = (page: SettingsPageId | null) => page === "memory" || (page?.startsWith("memory/") ?? false);

function navPage(page: SettingsPageId): SettingsNavPage | undefined {
  for (const group of SETTINGS_NAV) {
    for (const item of group.pages) {
      if (item.page === page && !item.children) return item;
      const child = item.children?.find((entry) => entry.page === page);
      if (child) return child;
    }
  }
  return undefined;
}

/** The page's own title, as its heading and the breadcrumb's last part. */
export function settingsTitle(page: SettingsPageId): string {
  if (page === "memory") return t("settings.nav.memory.title");
  const item = navPage(page);
  return item ? t(item.title) : t("common.settings");
}

/**
 * The toolbar breadcrumb: `Settings › <Page>`, with Memory's Overview and History showing
 * `Settings › Memory` and its other pages `Settings › Memory › <Page>` (Aside settings §0).
 * Null outside Settings.
 */
export function settingsBreadcrumb(pathname: string): string[] | null {
  if (!/^\/settings(\/|$)/.test(pathname)) return null;
  const root = t("common.settings");
  const page = settingsPageOf(pathname);
  if (!page) return [root];
  if (page === "memory" || page === "memory/history") return [root, t("settings.nav.memory.title")];
  if (isMemoryPage(page)) return [root, t("settings.nav.memory.title"), settingsTitle(page)];
  return [root, settingsTitle(page)];
}

/** Old deep links (the Settings and Workspace dialogs) and where each now lives. */
export const LEGACY_SETTINGS_PATHS: readonly (readonly [string, SettingsPageId])[] = [
  ["/models", "models"],
  ["/channels", "channels"],
  ["/remote", "developers"],
  ["/workspace", "general"],
  ["/team", "general"],
  ["/skills", "plugins"],
  ["/routines", "routines"],
  ["/memory", "memory"],
  ["/vault", "passwords"],
  ["/connections", "security"],
  ["/devices", "account"],
  ["/agents", "developers"],
  ["/sync", "account"],
  ["/policy", "agents"],
  ["/diagnostics", "general"],
  ["/config", "general"],
];
