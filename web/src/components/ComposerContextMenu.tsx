import { Blocks, Folder, Globe, type LucideIcon, Plus, Wand } from "lucide-react";
import type { MouseEvent as ReactMouseEvent } from "react";
import type { McpServer, PersonTab, PersonTabGroup, SkillSummary } from "../api/types";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { AGENT_TAB_GROUP_TITLE } from "../hooks/usePersonBrowser";
import { t, type MessageKey } from "../i18n";
import { cx, ICON_STROKE } from "../ui";

/** The `@prefix` or `/prefix` being typed right before the caret. */
export const CONTEXT_TYPING = /(?:^|[\s([{"'])([@/])([^\s@/]*)$/;

export type ContextTrigger = "@" | "/";
export type ContextSection = "groups" | "commands" | "tabs" | "skills" | "apps";

export interface ContextItem {
  id: string;
  section: ContextSection;
  label: string;
  detail: string;
  /** Icon of a command row; other rows use their section's. */
  icon?: LucideIcon;
  /** Text that replaces the typed `@prefix`. */
  insert?: string;
  /** Person tabs the pick attaches to the chat, so its agent may read them. */
  attach?: number[];
  /** A command: runs instead of inserting, and the typed prefix is removed. */
  run?: () => void;
  /** Shown for what it would do, with the reason in `detail`, but never picked. */
  disabled?: boolean;
}

export interface ContextCommand {
  id: string;
  label: string;
  detail: string;
  icon: LucideIcon;
  run?: () => void;
  disabled?: boolean;
}

export interface ContextSources {
  trigger: ContextTrigger;
  query: string;
  /** This chat's agent tabs: Aside's "Agent Tabs" group. */
  chatTabs: readonly OwnedTab[];
  /** Every agent tab, listed as open tabs while the person's own browser is not connected. */
  agentTabs: readonly OwnedTab[];
  personConnected: boolean;
  personTabs: readonly PersonTab[];
  personGroups: readonly PersonTabGroup[];
  commands: readonly ContextCommand[];
  skills: readonly SkillSummary[];
  apps: readonly McpServer[];
}

const SECTION_LABEL: Record<ContextSection, MessageKey> = {
  groups: "thread.context.section.groups",
  commands: "thread.context.section.commands",
  tabs: "thread.context.section.tabs",
  skills: "thread.context.section.skills",
  apps: "thread.context.section.apps",
};
const ORDER: readonly ContextSection[] = ["groups", "commands", "tabs", "skills", "apps"];
const SECTION_ICON: Record<ContextSection, LucideIcon> = { groups: Folder, commands: Plus, tabs: Globe, skills: Wand, apps: Blocks };

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

// The message carries text only (no tab-reference field), so a tab goes in as a Markdown link
// the agent can open; the title keeps it readable in the transcript. Only http(s) becomes a
// link, and the title and destination are escaped so a page cannot inject Markdown of its own.
function linkTarget(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
    return parsed.href.replace(/[()<>\s]/g, (char) => `%${char.charCodeAt(0).toString(16).toUpperCase().padStart(2, "0")}`);
  } catch {
    return null;
  }
}

const escapeText = (text: string) => text.replace(/\s+/g, " ").replace(/[\\[\]]/g, "\\$&");

export function tabLink(tab: Pick<OwnedTab, "title" | "url">): string {
  const target = linkTarget(tab.url);
  const title = escapeText(tab.title.trim() || (target ? hostOf(tab.url) : "") || t("thread.context.untitledTab"));
  return target ? `[${title}](${target})` : title;
}

const tabCount = (count: number) => t("thread.context.tabCount", { count });

/**
 * Rows for the `@` menu (`L 37`: Tab Groups, Commands, Open Tabs, then skills and apps) or the
 * `/` menu (commands and skills), narrowed to those matching the typed query.
 */
export function contextItems(sources: ContextSources): ContextItem[] {
  const { trigger, query } = sources;
  const commands: ContextItem[] = sources.commands.map((command) => ({ ...command, id: `command-${command.id}`, section: "commands" }));
  const skills: ContextItem[] = sources.skills.map((skill) => ({
    id: `skill-${skill.name}`,
    section: "skills",
    label: `/${skill.name}`,
    detail: skill.description,
    insert: `/${skill.name}`,
  }));
  let items: ContextItem[];
  if (trigger === "/") items = [...commands, ...skills];
  else {
    const agentGroup = sources.personGroups.filter((group) => group.title === AGENT_TAB_GROUP_TITLE).map((group) => group.group_id);
    const ownTabs = sources.personTabs.filter((tab) => tab.group_id == null || !agentGroup.includes(tab.group_id));
    const groups: ContextItem[] = [
      ...(sources.chatTabs.length
        ? [{ id: "group-agent", section: "groups" as const, label: AGENT_TAB_GROUP_TITLE, detail: tabCount(sources.chatTabs.length), insert: sources.chatTabs.map(tabLink).join(" ") }]
        : []),
      ...sources.personGroups.flatMap((group) => {
        const members = ownTabs.filter((tab) => tab.group_id === group.group_id);
        if (agentGroup.includes(group.group_id) || members.length === 0) return [];
        return [
          {
            id: `group-${group.group_id}`,
            section: "groups" as const,
            label: group.title.trim() || t("thread.context.untitledGroup"),
            detail: tabCount(members.length),
            insert: members.map(tabLink).join(" "),
            attach: members.map((tab) => tab.tab_id),
          },
        ];
      }),
    ];
    const tabs: ContextItem[] = sources.personConnected
      ? ownTabs.map((tab) => ({
          id: `tab-${tab.tab_id}`,
          section: "tabs",
          label: tab.title.trim() || hostOf(tab.url) || tab.url,
          detail: hostOf(tab.url),
          insert: tabLink(tab),
          attach: [tab.tab_id],
        }))
      : sources.agentTabs.map((tab) => ({
          id: `agent-tab-${tab.tab_id}`,
          section: "tabs",
          label: tab.title.trim() || hostOf(tab.url) || tab.url,
          detail: hostOf(tab.url),
          insert: tabLink(tab),
        }));
    const apps: ContextItem[] = sources.apps
      .filter((app) => app.status === "connected")
      .map((app) => ({ id: `app-${app.id}`, section: "apps", label: app.label, detail: t("thread.context.appTools", { count: app.tools.length }), insert: `@${app.label}` }));
    items = [...groups, ...commands, ...tabs, ...skills, ...apps];
  }
  const needle = query.toLowerCase();
  return items.filter((item) => !needle || item.label.toLowerCase().includes(needle) || item.detail.toLowerCase().includes(needle));
}

/** The next pickable row from `from` in `step` direction, wrapping; `from` itself when none is. */
export function stepActive(items: readonly ContextItem[], from: number, step: 1 | -1): number {
  for (let offset = 1; offset <= items.length; offset += 1) {
    const index = (from + step * offset + items.length * offset) % items.length;
    if (!items[index]?.disabled) return index;
  }
  return from;
}

// The textarea keeps focus (and the phone keyboard stays up); the pick itself runs on click,
// which touch taps, pens and mice all produce.
const keepFocus = (event: ReactMouseEvent) => event.preventDefault();

/**
 * The `@` / `/` popup (362×242 in Aside). Focus stays in the textarea, which moves `active` and
 * picks with Enter. On phones it is a bottom sheet over a scrim; the header echoes what is being
 * typed because the sheet covers the box.
 */
export function ComposerContextMenu({
  items,
  active,
  below,
  trigger,
  query,
  onPick,
  onClose,
}: {
  items: readonly ContextItem[];
  active: number;
  below: boolean;
  trigger: ContextTrigger;
  query: string;
  onPick: (item: ContextItem) => void;
  onClose: () => void;
}) {
  return (
    <>
      <div className="context-menu-scrim" aria-hidden="true" onMouseDown={keepFocus} onClick={onClose} />
      <div id="context-menu" className="context-menu surface-overlay thin-scroll" role="listbox" aria-label={t("thread.context.title")} data-placement={below ? "below" : "above"}>
        <div className="context-menu-sheet-head" role="presentation">
          <span>{t("thread.context.title")}</span>
          <span className="context-menu-query">
            {trigger}
            {query}
          </span>
        </div>
        {ORDER.map((section) => {
          const rows = items.filter((item) => item.section === section);
          if (!rows.length) return null;
          return (
            <div key={section} role="group" aria-labelledby={`context-${section}`} className="context-menu-group">
              <div id={`context-${section}`} className="context-menu-label" role="presentation">
                {t(SECTION_LABEL[section])}
              </div>
              {rows.map((item) => {
                const Icon = item.icon ?? SECTION_ICON[section];
                return (
                  <div
                    key={item.id}
                    id={`context-${item.id}`}
                    role="option"
                    aria-selected={items[active]?.id === item.id}
                    aria-disabled={item.disabled || undefined}
                    className={cx("context-menu-item", section === "groups" && "context-menu-item--group")}
                    onMouseDown={keepFocus}
                    onClick={() => !item.disabled && onPick(item)}
                  >
                    <Icon size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                    <span className="context-menu-title">{item.label}</span>
                    {item.detail ? <span className="context-menu-detail">{item.detail}</span> : null}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </>
  );
}
