import { AppWindow, Bookmark, ChevronDown, ChevronUp, Copy, FolderMinus, FolderPlus, Globe, Pin, PinOff, Plus, RotateCw, SquareArrowOutUpRight, Volume2, VolumeX, X } from "lucide-react";
import { useContext, useEffect, useState, type DragEvent, type ReactNode } from "react";
import { Link } from "react-router";
import type { PersonTab, PersonTabCommand, PersonTabGroup } from "../api/types";
import { useClient } from "../context";
import { AgentTabsSettledContext, type OwnedTab } from "../hooks/useAgentTabs";
import { originOf, useFavicon } from "../hooks/useFavicon";
import { AGENT_TAB_GROUP_TITLE, bookmarkableUrl, usePersonBrowser } from "../hooks/usePersonBrowser";
import { markTabUsed, useSidebarPrefs } from "../shell";
import { t } from "../i18n";
import { BrandMark, ContextMenu, cx, ICON_STROKE, MenuItem, MenuSeparator, useToast } from "../ui";
import { browserFailure } from "./SidebarChats";

/** Group keys the reader folded away; shared by the desktop rail and the phone drawer. */
export const COLLAPSED_TAB_GROUPS_KEY = "openside.sidebar.collapsedTabGroups";
/** Drag payload of a sidebar tab row: `{ title, url }` as JSON, dropped on Bookmarks to save it. */
export const TAB_DRAG_TYPE = "application/x-openside-tab";

const AGENT_GROUP = "agent";
const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** The chat that opened the tab, with the browser sheet open on it; an agent's home tab opens its chat too (D1). */
export function tabTarget(tab: OwnedTab): string {
  return `/sessions/${encodeURIComponent(tab.owner_session_id)}?browser=open`;
}

function loadCollapsed(): Set<string> {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(COLLAPSED_TAB_GROUPS_KEY) ?? "[]");
    return new Set(Array.isArray(stored) ? stored.filter((key): key is string => typeof key === "string") : []);
  } catch {
    return new Set();
  }
}

function saveCollapsed(keys: ReadonlySet<string>) {
  if (keys.size === 0) localStorage.removeItem(COLLAPSED_TAB_GROUPS_KEY);
  else localStorage.setItem(COLLAPSED_TAB_GROUPS_KEY, JSON.stringify([...keys]));
}

/** Lets a row be dropped on Bookmarks (`SidebarBookmarks`) and anywhere that takes a link. */
export function dragTab(event: DragEvent, title: string, url: string) {
  event.dataTransfer.setData(TAB_DRAG_TYPE, JSON.stringify({ title, url }));
  event.dataTransfer.setData("text/uri-list", url);
  event.dataTransfer.setData("text/plain", url);
  event.dataTransfer.effectAllowed = "copyLink";
}

/**
 * A site icon: Chrome's own favicon when it is a data URL (the page CSP allows no other remote
 * image), else the daemon's cached one for the origin, else a globe.
 */
export function Favicon({ url, favicon }: { url: string; favicon?: string | null }) {
  const inline = favicon?.startsWith("data:") ? favicon : null;
  const cached = useFavicon(inline ? null : originOf(url));
  const src = inline ?? cached;
  if (src) return <img className="sidebar-favicon" src={src} alt="" width={16} height={16} />;
  return <Globe className="sidebar-favicon sidebar-favicon--globe" size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
}

type Block = { kind: "tab"; tab: PersonTab } | { kind: "group"; group: PersonTabGroup; tabs: PersonTab[] };

export interface SidebarTabsProps {
  /** The agents' open tabs, polled once for the shell. */
  agentTabs: readonly OwnedTab[];
  /** Names openside's own pages ("Settings › Models") when a person tab shows one; `null` for Home. */
  pageLabel: (pathname: string) => string | null;
  /** Called after a row is chosen, so the phone drawer can close (switching tabs changes no route). */
  onPicked?: () => void;
}

/** Tabs: New tab, the Agent Tabs group, then the person's own tabs and tab groups (`L 64`). */
export function SidebarTabs({ agentTabs, pageLabel, onPicked }: SidebarTabsProps) {
  const toast = useToast();
  const browser = usePersonBrowser();
  const agentTabsSettled = useContext(AgentTabsSettledContext);
  const [collapsed, setCollapsed] = useState(loadCollapsed);
  const [{ newTabPosition }] = useSidebarPrefs();
  const toggle = (key: string) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(key)) next.add(key);
      saveCollapsed(next);
      return next;
    });

  const groups = new Map(browser.groups.map((group) => [group.group_id, group]));
  // Agent tabs already show from the ownership ledger, with their conversation behind them; until
  // that list has answered (or after it fails) the browser's own Agent Tabs group stays visible.
  const own = agentTabsSettled ? browser.tabs.filter((tab) => tab.group_id == null || groups.get(tab.group_id)?.title !== AGENT_TAB_GROUP_TITLE) : browser.tabs;
  const blocks: Block[] = [];
  for (const tab of own) {
    const group = tab.group_id == null ? undefined : groups.get(tab.group_id);
    const last = blocks.at(-1);
    if (!group) blocks.push({ kind: "tab", tab });
    else if (last?.kind === "group" && last.group.group_id === group.group_id) last.tabs.push(tab);
    else blocks.push({ kind: "group", group, tabs: [tab] });
  }
  // The browser appends new tabs to the end of their window, so "At top" lists the strip newest
  // first; pinned tabs stay ahead of the rest as they do in the strip.
  if (newTabPosition === "top") {
    const pinned = blocks.filter((block) => block.kind === "tab" && block.tab.pinned);
    const rest = blocks.filter((block) => !(block.kind === "tab" && block.tab.pinned)).reverse();
    blocks.splice(0, blocks.length, ...pinned, ...rest);
  }

  const run = (command: PersonTabCommand) => void browser.command(command).catch((cause: unknown) => toast.notify(browserFailure(cause), "error"));
  const newTab = () => {
    // Without the browser bridge the page can still open a fresh openside tab itself.
    if (browser.connected) run({ command: "create" });
    else window.open("/", "_blank", "noopener");
    onPicked?.();
  };

  // Forget folds of groups that are gone, but only while some tabs are listed: an empty poll must not unfold everything.
  const live = [agentTabs.length > 0 ? AGENT_GROUP : "", ...browser.groups.map((group) => `g:${group.group_id}`)].filter(Boolean).join("\n");
  useEffect(() => {
    if (live === "") return;
    const keys = new Set(live.split("\n"));
    setCollapsed((current) => {
      const kept = new Set([...current].filter((key) => keys.has(key)));
      if (kept.size === current.size) return current;
      saveCollapsed(kept);
      return kept;
    });
  }, [live]);

  const row = (tab: PersonTab, child = false) => (
    <PersonTabRow key={tab.tab_id} tab={tab} selected={tab.active && tab.window_id === browser.focusedWindow} child={child} pageLabel={pageLabel} all={own} run={run} onPicked={onPicked} />
  );

  return (
    <section className="sidebar-section" aria-labelledby="sidebar-tabs-heading">
      <h2 id="sidebar-tabs-heading" className="sidebar-heading">
        {t("shell.tabs.heading")}
      </h2>
      <ul className="sidebar-rows sidebar-tabs">
        <li>
          <button type="button" className="sidebar-row sidebar-row--secondary" onClick={newTab}>
            <Plus {...icon} />
            <span>{t("shell.tabs.newTab")}</span>
          </button>
        </li>
        {agentTabs.length > 0 ? (
          <GroupRows label={t("shell.tabs.agentGroup")} count={agentTabs.length} open={!collapsed.has(AGENT_GROUP)} onToggle={() => toggle(AGENT_GROUP)} glyph={<AppWindow {...icon} />}>
            {agentTabs.map((tab) => (
              <AgentTabRow key={tab.tab_id} tab={tab} to={tabTarget(tab)} />
            ))}
          </GroupRows>
        ) : null}
        {blocks.map((block) =>
          block.kind === "tab" ? (
            row(block.tab)
          ) : (
            <GroupRows
              key={`g:${block.group.group_id}`}
              label={block.group.title || t("shell.tabs.untitledGroup")}
              count={block.tabs.length}
              open={!collapsed.has(`g:${block.group.group_id}`)}
              onToggle={() => toggle(`g:${block.group.group_id}`)}
              glyph={<span className="tab-group-dot" data-color={block.group.color} aria-hidden="true" />}
            >
              {block.tabs.map((tab) => row(tab, true))}
            </GroupRows>
          ),
        )}
      </ul>
    </section>
  );
}

function GroupRows({ label, count, open, onToggle, glyph, children }: { label: string; count: number; open: boolean; onToggle: () => void; glyph: ReactNode; children: ReactNode }) {
  return (
    <li className="tab-group">
      <button type="button" className="sidebar-row tab-group-toggle" aria-expanded={open} aria-label={t("shell.tabs.groupToggle", { label, count })} onClick={onToggle}>
        {glyph}
        <span className="tab-group-label">{label}</span>
        {open ? <ChevronUp className="tab-group-caret" size={12} strokeWidth={ICON_STROKE} aria-hidden="true" /> : <ChevronDown className="tab-group-caret" size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />}
        <span className="tab-group-count" aria-hidden="true">
          {count}
        </span>
      </button>
      {open ? (
        <ul className="sidebar-rows tab-group-rows" aria-label={label}>
          {children}
        </ul>
      ) : null}
    </li>
  );
}

function AgentTabRow({ tab, to }: { tab: OwnedTab; to: string }) {
  const origin = originOf(tab.url);
  const host = origin ? new URL(origin).host : "";
  const title = tab.title.trim() || host || tab.url;
  return (
    <li className="tab-row tab-row--child">
      <Link
        to={to}
        className="sidebar-row"
        title={title}
        aria-description={tab.url}
        draggable
        onDragStart={(event) => dragTab(event, title, tab.url)}
        onClick={() => markTabUsed(tab.tab_id)}
      >
        <Favicon url={tab.url} />
        <span className="sidebar-row-label">{title}</span>
      </Link>
    </li>
  );
}

interface PersonTabRowProps {
  tab: PersonTab;
  selected: boolean;
  child: boolean;
  pageLabel: (pathname: string) => string | null;
  /** The listed tabs in order, for Close Other Tabs and Close Tabs Below. */
  all: readonly PersonTab[];
  run: (command: PersonTabCommand) => void;
  onPicked?: () => void;
}

function PersonTabRow({ tab, selected, child, pageLabel, all, run, onPicked }: PersonTabRowProps) {
  const client = useClient();
  const toast = useToast();
  const browser = usePersonBrowser();
  const page = ourPage(tab.url);
  const title = page !== null ? (pageLabel(page) ?? t("shell.tabs.newTabTitle")) : tab.title.trim() || tab.url;
  const closeOthers = all.filter((other) => other.window_id === tab.window_id && other.tab_id !== tab.tab_id && !other.pinned).map((other) => other.tab_id);
  const below = all.filter((other) => other.window_id === tab.window_id && other.index > tab.index).map((other) => other.tab_id);

  const saveable = bookmarkableUrl(tab.url);

  async function bookmark() {
    if (!saveable) {
      toast.notify(t("shell.bookmarks.notSaved"), "error");
      return;
    }
    try {
      await client.createPersonBookmark({ parent_id: browser.bookmarkBar, title, url: saveable });
      toast.notify(t("shell.tabs.bookmarked"));
      browser.refreshBookmarks();
    } catch (cause) {
      toast.notify(browserFailure(cause), "error");
    }
  }

  return (
    <ContextMenu
      label={t("shell.tabs.menu", { title })}
      trigger={
        <li className={cx("tab-row", child && "tab-row--child", selected && "current")} data-tab={tab.tab_id}>
          <button
            type="button"
            className="sidebar-row"
            aria-current={selected ? "page" : undefined}
            title={title}
            draggable
            onDragStart={(event) => dragTab(event, title, tab.url)}
            onClick={() => {
              run({ command: "activate", tab_id: tab.tab_id });
              onPicked?.();
            }}
            onAuxClick={(event) => event.button === 1 && run({ command: "close", tab_ids: [tab.tab_id] })}
          >
            {page !== null ? <BrandMark size={16} /> : <Favicon url={tab.url} favicon={tab.favicon_url} />}
            <span className="sidebar-row-label">{page !== null ? <PageLabel label={title} /> : title}</span>
            {tab.audible || tab.muted ? tab.muted ? <VolumeX className="tab-sound" size={14} strokeWidth={ICON_STROKE} aria-hidden="true" /> : <Volume2 className="tab-sound" size={14} strokeWidth={ICON_STROKE} aria-hidden="true" /> : null}
          </button>
          <button type="button" className="sidebar-row-trailing sidebar-row-action tab-close" aria-label={t("shell.tabs.closeNamed", { title })} title={t("shell.tabs.close")} onClick={() => run({ command: "close", tab_ids: [tab.tab_id] })}>
            <X size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </button>
        </li>
      }
    >
      <MenuItem icon={<Plus {...icon} />} onClick={() => run({ command: "create", after_tab_id: tab.tab_id })}>
        {t("shell.tabs.newTabBelow")}
      </MenuItem>
      <MenuItem icon={<FolderPlus {...icon} />} onClick={() => run({ command: "group", tab_ids: [tab.tab_id] })}>
        {t("shell.tabs.addToNewGroup")}
      </MenuItem>
      {tab.group_id != null ? (
        <MenuItem icon={<FolderMinus {...icon} />} onClick={() => run({ command: "ungroup", tab_ids: [tab.tab_id] })}>
          {t("shell.tabs.removeFromGroup")}
        </MenuItem>
      ) : null}
      <MenuItem icon={<SquareArrowOutUpRight {...icon} />} onClick={() => run({ command: "move_to_new_window", tab_id: tab.tab_id })}>
        {t("shell.tabs.moveToNewWindow")}
      </MenuItem>
      <MenuSeparator />
      <MenuItem icon={<RotateCw {...icon} />} onClick={() => run({ command: "reload", tab_id: tab.tab_id })}>
        {t("shell.tabs.reload")}
      </MenuItem>
      <MenuItem icon={<Copy {...icon} />} onClick={() => run({ command: "duplicate", tab_id: tab.tab_id })}>
        {t("shell.tabs.duplicate")}
      </MenuItem>
      <MenuItem icon={tab.pinned ? <PinOff {...icon} /> : <Pin {...icon} />} onClick={() => run({ command: "set_pinned", tab_id: tab.tab_id, pinned: !tab.pinned })}>
        {t(tab.pinned ? "shell.tabs.unpin" : "shell.tabs.pin")}
      </MenuItem>
      <MenuItem icon={tab.muted ? <Volume2 {...icon} /> : <VolumeX {...icon} />} onClick={() => run({ command: "set_muted", tab_id: tab.tab_id, muted: !tab.muted })}>
        {t(tab.muted ? "shell.tabs.unmute" : "shell.tabs.mute")}
      </MenuItem>
      <MenuSeparator />
      <MenuItem icon={<Bookmark {...icon} />} disabled={!saveable} onClick={() => void bookmark()}>
        {t("shell.tabs.addToBookmarks")}
      </MenuItem>
      <MenuSeparator />
      <MenuItem icon={<X {...icon} />} onClick={() => run({ command: "close", tab_ids: [tab.tab_id] })}>
        {t("shell.tabs.close")}
      </MenuItem>
      <MenuItem disabled={closeOthers.length === 0} onClick={() => run({ command: "close", tab_ids: closeOthers })}>
        {t("shell.tabs.closeOthers")}
      </MenuItem>
      <MenuItem disabled={below.length === 0} onClick={() => run({ command: "close", tab_ids: below })}>
        {t("shell.tabs.closeBelow")}
      </MenuItem>
    </ContextMenu>
  );
}

/** "Settings › Models": the separator is drawn in the label's colour with a little air (`L 64`). */
export function PageLabel({ label }: { label: string }) {
  const parts = label.split(" › ");
  return (
    <>
      {parts.map((part, index) => (
        <span key={index} className="page-crumb">
          {index > 0 ? <span className="page-crumb-sep" aria-hidden="true">›</span> : null}
          {part}
        </span>
      ))}
    </>
  );
}

/** The path of a tab that shows this openside app, or `null` for any other page. */
export function ourPage(url: string): string | null {
  try {
    const parsed = new URL(url);
    return parsed.origin === window.location.origin ? parsed.pathname : null;
  } catch {
    return null;
  }
}
