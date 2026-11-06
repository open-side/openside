import { Bookmark, Check, ChevronDown, CircleUserRound, Code, Download, Ellipsis, Folder, History, Pencil, Plus, Puzzle, Search, Settings, SquareDashed, Trash2 } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../api/client";
import type { PersonBookmark, PersonDownload, PersonExtension, PersonHistoryItem } from "../api/types";
import { useClient } from "../context";
import { openInBrowser, usePersonBrowser } from "../hooks/usePersonBrowser";
import { t } from "../i18n";
import { useShell } from "../shell";
import { keyLabel, shortcutKeys } from "../shortcuts";
import { ICON_STROKE, IconButton, MenuGroupLabel, MenuItem, MenuSeparator, PopoverMenu, SubmenuItem, useToast } from "../ui";
import { browserFailure } from "./SidebarChats";

const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;
const SUBMENU_ROWS = 10;

/** The person's name from Settings › Account; a refused or missing route leaves the fallback in place. */
function usePersonName(): string | null {
  const client = useClient();
  const [name, setName] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    let settled = false;
    const load = () => {
      if (settled) return;
      client.preferences().then(
        (preferences) => {
          settled = true;
          if (live) setName(preferences.name?.trim() || null);
        },
        (cause: unknown) => {
          // A 4xx (an old daemon, a refused token) is final; an unreachable daemon is asked again on focus.
          if (cause instanceof ApiError && cause.status >= 400 && cause.status < 500) settled = true;
        },
      );
    };
    load();
    window.addEventListener("focus", load);
    return () => {
      live = false;
      window.removeEventListener("focus", load);
    };
  }, [client]);
  return name;
}

/** Rows read when their submenu opens; `null` while loading, `"offline"` when the browser is not connected. */
function useOnOpen<T>(read: () => Promise<{ connected: boolean; rows: T[] }>) {
  const [rows, setRows] = useState<T[] | "offline" | null>(null);
  const load = () => {
    setRows(null);
    read().then(
      (result) => setRows(result.connected ? result.rows : "offline"),
      () => setRows("offline"),
    );
  };
  return [rows, load] as const;
}

/** A submenu's live rows, or the one line that says why there are none. */
function Rows<T>({ rows, empty, render }: { rows: T[] | "offline" | null; empty: string; render: (row: T) => ReactNode }) {
  if (rows === null) return <MenuItem disabled>{t("common.loading")}</MenuItem>;
  if (rows === "offline") return <MenuItem disabled>{t("shell.browser.notConnected")}</MenuItem>;
  if (rows.length === 0) return <MenuItem disabled>{empty}</MenuItem>;
  return <>{rows.map(render)}</>;
}

/**
 * The account menu (`V account-menu-light`), opened from the sidebar's profile button or from
 * Home's top-right avatar (`L 10`); both triggers open the same menu.
 */
export function AccountMenu({ placement, onClose }: { placement: "sidebar" | "home"; onClose?: () => void }) {
  const client = useClient();
  const toast = useToast();
  const navigate = useNavigate();
  const { needsYou } = useShell();
  // Home's avatar carries a dot while any chat waits for the person (an approval, an answer).
  const attention = placement === "home" && needsYou > 0;
  const browser = usePersonBrowser();
  const name = usePersonName() ?? t("shell.account.defaultProfile");
  const initial = [...name][0]?.toUpperCase() ?? "?";
  const sidebar = placement === "sidebar";

  // The whole tree: the Bookmarks bar's items, then the other roots ("Other bookmarks") as folders.
  const [bookmarks, loadBookmarks] = useOnOpen<PersonBookmark>(() => client.personBookmarks().then((tree) => ({ connected: tree.connected, rows: [...(tree.roots[0]?.children ?? []), ...tree.roots.slice(1)] })));
  const [downloads, loadDownloads] = useOnOpen<PersonDownload>(() => client.personDownloads(SUBMENU_ROWS).then((list) => ({ connected: list.connected, rows: list.downloads })));
  const [extensions, loadExtensions] = useOnOpen<PersonExtension>(() => client.personExtensions().then((list) => ({ connected: list.connected, rows: list.extensions.slice(0, SUBMENU_ROWS) })));
  const [history, loadHistory] = useOnOpen<PersonHistoryItem>(() => client.personHistory({ limit: SUBMENU_ROWS }).then((list) => ({ connected: list.connected, rows: list.items })));

  const go = (path: string) => {
    onClose?.();
    navigate(path);
  };
  const page = (url: string) => {
    onClose?.();
    void openInBrowser(client, url).then((opened) => {
      if (!opened && !/^https?:/i.test(url)) toast.notify(t("shell.browser.notConnected"), "error");
    });
  };
  const newTab = () => {
    onClose?.();
    if (!browser.connected) window.open("/", "_blank", "noopener");
    else void browser.command({ command: "create" }).catch((cause: unknown) => toast.notify(browserFailure(cause), "error"));
  };
  const incognito = () => {
    onClose?.();
    void browser.command({ command: "new_incognito_window" }).catch((cause: unknown) => toast.notify(browserFailure(cause), "error"));
  };
  const nativeTool = (tool: "task_manager" | "devtools") => {
    onClose?.();
    void client.openBrowserTool(tool).catch((cause: unknown) => {
      const code = cause instanceof ApiError ? cause.code : null;
      const message =
        code === "browser_not_running" ? t("shell.account.browserClosed") : code === "native_tools_unsupported" ? t("shell.account.toolUnsupported") : code === "browser_disconnected" ? t("shell.browser.notConnected") : browserFailure(cause);
      toast.notify(message, "error");
    });
  };
  const link = (url: string | null | undefined, title: string, key: string) => (
    <MenuItem key={key} icon={<Bookmark {...icon} />} disabled={!url} onClick={() => url && page(url)}>
      {title}
    </MenuItem>
  );
  const bookmarkRow = (item: PersonBookmark): ReactNode =>
    item.url == null ? (
      <SubmenuItem key={item.id} icon={<Folder {...icon} />} label={item.title || t("shell.bookmarks.untitled")} popupClassName="account-menu account-submenu">
        {item.children.length === 0 ? <MenuItem disabled>{t("shell.account.noBookmarks")}</MenuItem> : item.children.map(bookmarkRow)}
      </SubmenuItem>
    ) : (
      link(item.url, item.title || item.url, item.id)
    );

  return (
    <PopoverMenu
      label={t("shell.account.menu")}
      side={sidebar ? "top" : "bottom"}
      align={sidebar ? "start" : "end"}
      sideOffset={sidebar ? 1 : 6}
      popupClassName="account-menu"
      trigger={
        <button
          type="button"
          className={sidebar ? "sidebar-account" : "home-account"}
          aria-label={attention ? `${t("shell.account.trigger", { name })}, ${t("shell.person.needsYou")}` : t("shell.account.trigger", { name })}
          title={name}
        >
          <span className={sidebar ? "sidebar-avatar" : "sidebar-avatar home-avatar"} aria-hidden="true">
            {initial}
            {attention ? <span className="home-avatar-dot" /> : null}
          </span>
          <ChevronDown size={sidebar ? 12 : 20} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      }
    >
      <MenuGroupLabel>{t("shell.account.profiles")}</MenuGroupLabel>
      {/* The current profile with ✓ and a … that opens its own options, as the reference's profile rows do. */}
      <SubmenuItem
        icon={
          <span className="sidebar-avatar sidebar-avatar--menu" aria-hidden="true">
            {initial}
          </span>
        }
        label={name}
        value={<Check size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />}
        indicator={<Ellipsis size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />}
        popupClassName="account-menu account-submenu"
      >
        <MenuItem icon={<Pencil {...icon} />} onClick={() => go("/settings/account")}>
          {t("shell.account.editProfile")}
        </MenuItem>
        {/* D6: openside cannot remove a browser profile itself; the browser's own profile manager does. */}
        <MenuItem icon={<Trash2 {...icon} />} onClick={() => page("chrome://profile-picker")}>
          {t("shell.account.deleteProfile")}
        </MenuItem>
      </SubmenuItem>
      <MenuItem icon={<CircleUserRound {...icon} />} onClick={() => page("chrome://profile-picker")}>
        {t("shell.account.newProfile")}
      </MenuItem>
      <MenuSeparator />
      <SubmenuItem icon={<Bookmark {...icon} />} label={t("shell.account.bookmarks")} onOpen={loadBookmarks} popupClassName="account-menu account-submenu">
        <Rows rows={bookmarks} empty={t("shell.account.noBookmarks")} render={bookmarkRow} />
        <MenuSeparator />
        <MenuItem onClick={() => page("chrome://bookmarks")}>{t("shell.account.bookmarkManager")}</MenuItem>
      </SubmenuItem>
      <SubmenuItem icon={<Download {...icon} />} label={t("shell.account.downloads")} onOpen={loadDownloads} popupClassName="account-menu account-submenu">
        <Rows
          rows={downloads}
          empty={t("shell.account.noDownloads")}
          render={(item) => (
            <MenuItem key={item.download_id} icon={<Download {...icon} />} onClick={() => page("chrome://downloads")}>
              {item.filename.split(/[\\/]/).at(-1) || item.url}
            </MenuItem>
          )}
        />
        <MenuSeparator />
        <MenuItem onClick={() => page("chrome://downloads")}>{t("shell.account.allDownloads")}</MenuItem>
      </SubmenuItem>
      <SubmenuItem icon={<Puzzle {...icon} />} label={t("shell.account.extensions")} onOpen={loadExtensions} popupClassName="account-menu account-submenu">
        <Rows
          rows={extensions}
          empty={t("shell.account.noExtensions")}
          render={(item) => (
            <MenuItem key={item.id} icon={<Puzzle {...icon} />} checked={item.enabled} checkRole="checkbox" onClick={() => page("chrome://extensions")}>
              {item.name}
            </MenuItem>
          )}
        />
        <MenuSeparator />
        <MenuItem onClick={() => page("chrome://extensions")}>{t("shell.account.manageExtensions")}</MenuItem>
      </SubmenuItem>
      <SubmenuItem icon={<History {...icon} />} label={t("shell.account.history")} onOpen={loadHistory} popupClassName="account-menu account-submenu">
        <Rows rows={history} empty={t("shell.account.noHistory")} render={(item) => link(item.url, item.title || item.url, `${item.url}\n${item.last_visit_at}`)} />
        <MenuSeparator />
        <MenuItem onClick={() => page("chrome://history")}>{t("shell.account.fullHistory")}</MenuItem>
      </SubmenuItem>
      <SubmenuItem icon={<Code {...icon} />} label={t("shell.account.developers")} popupClassName="account-menu account-submenu">
        <MenuItem onClick={() => page("chrome://settings/performance")}>{t("shell.account.performance")}</MenuItem>
        <MenuItem onClick={() => nativeTool("task_manager")}>{t("shell.account.taskManager")}</MenuItem>
        <MenuItem onClick={() => nativeTool("devtools")}>{t("shell.account.devTools")}</MenuItem>
        <MenuSeparator />
        <SubmenuItem label={t("shell.account.advanced")} popupClassName="account-menu account-submenu">
          <MenuItem onClick={() => page("chrome://inspect")}>{t("shell.account.remoteDebugging")}</MenuItem>
        </SubmenuItem>
      </SubmenuItem>
      <MenuItem icon={<Settings {...icon} />} trailing={<span className="account-shortcut">{keyLabel(["Mod", ","])}</span>} onClick={() => go("/settings")}>
        {t("shell.account.settings")}
      </MenuItem>
      <MenuSeparator />
      <MenuItem icon={<Plus {...icon} />} trailing={<span className="account-shortcut">{keyLabel(["Mod", "T"])}</span>} onClick={newTab}>
        {t("shell.account.newTab")}
      </MenuItem>
      <MenuItem icon={<SquareDashed {...icon} />} disabled={!browser.connected} trailing={<span className="account-shortcut">{keyLabel(["Mod", "Shift", "N"])}</span>} onClick={incognito}>
        {t("shell.account.incognito")}
      </MenuItem>
    </PopoverMenu>
  );
}

/**
 * The sidebar's bottom bar: the profile button with the account menu and the search palette. It
 * sits outside the scrolling list so it is reachable however long it grows.
 */
export function SidebarAccount({ onClose }: { onClose?: () => void }) {
  const shell = useShell();
  const search = () => {
    onClose?.();
    shell.openSearch();
  };
  return (
    <div className="sidebar-utility">
      <AccountMenu placement="sidebar" onClose={onClose} />
      <IconButton label={t("common.search")} title={t("shell.account.searchTitle", { keys: keyLabel(shortcutKeys("search")) })} className="sidebar-search" size="small" onClick={search}>
        <Search size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </IconButton>
    </div>
  );
}
