import { CircleStop, Globe, Layers, Plus } from "lucide-react";
import type { AgentSummary } from "../api/types";
import type { MouseEvent as ReactMouseEvent } from "react";
import type { OwnedTab } from "../hooks/useAgentTabs";
import { ICON_STROKE } from "../ui";

/** The `@prefix` being typed right before the caret; `/` stays the skill list's. */
export const CONTEXT_TYPING = /(?:^|[\s([{"'])@([^\s@]*)$/;

export type ContextSection = "groups" | "commands" | "tabs";

export interface ContextItem {
  id: string;
  section: ContextSection;
  label: string;
  detail: string;
  /** Text that replaces the typed `@prefix`. */
  insert?: string;
  /** A command: runs instead of inserting, and the `@prefix` is removed. */
  run?: () => void;
}

export interface ContextCommand {
  id: string;
  label: string;
  detail: string;
  run: () => void;
}

const SECTION_LABEL: Record<ContextSection, string> = { groups: "Tab groups", commands: "Commands", tabs: "Open tabs" };
const ORDER: readonly ContextSection[] = ["groups", "commands", "tabs"];

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
  const title = escapeText(tab.title.trim() || (target ? hostOf(tab.url) : "") || "Untitled tab");
  return target ? `[${title}](${target})` : title;
}

/** Rows for the `@` menu, in section order, narrowed to those matching `query`. */
export function contextItems(query: string, tabs: readonly OwnedTab[], agents: readonly AgentSummary[], commands: readonly ContextCommand[]): ContextItem[] {
  const groups = new Map<string, OwnedTab[]>();
  for (const tab of tabs) groups.set(tab.owner_session_id, [...(groups.get(tab.owner_session_id) ?? []), tab]);
  const items: ContextItem[] = [
    ...[...groups].map(([session, list]) => ({
      id: `group-${session}`,
      section: "groups" as const,
      label: agents.find((agent) => agent.home_session_id === session)?.name ?? "Conversation tabs",
      detail: list.length === 1 ? "1 tab" : `${list.length} tabs`,
      insert: list.map(tabLink).join(" "),
    })),
    ...commands.map((command) => ({ ...command, id: `command-${command.id}`, section: "commands" as const })),
    ...tabs.map((tab) => ({
      id: `tab-${tab.tab_id}`,
      section: "tabs" as const,
      label: tab.title.trim() || hostOf(tab.url) || tab.url,
      detail: hostOf(tab.url),
      insert: tabLink(tab),
    })),
  ];
  const needle = query.toLowerCase();
  return items.filter((item) => !needle || item.label.toLowerCase().includes(needle) || item.detail.toLowerCase().includes(needle));
}

const ICON: Record<ContextSection, typeof Globe> = { groups: Layers, commands: Plus, tabs: Globe };

// The textarea keeps focus (and the phone keyboard stays up); the pick itself runs on click,
// which touch taps, pens and mice all produce.
const keepFocus = (event: ReactMouseEvent) => event.preventDefault();

/**
 * The `@` popup. Focus stays in the textarea, which moves `active` and picks with Enter. On
 * phones it is a bottom sheet over a scrim (thread.css); the header echoes what is being typed
 * because the sheet covers the box.
 */
export function ComposerContextMenu({
  items,
  active,
  below,
  query,
  onPick,
  onClose,
}: {
  items: readonly ContextItem[];
  active: number;
  below: boolean;
  query: string;
  onPick: (item: ContextItem) => void;
  onClose: () => void;
}) {
  return (
    <>
      <div className="context-menu-scrim" aria-hidden="true" onMouseDown={keepFocus} onClick={onClose} />
      <div id="context-menu" className="context-menu surface-overlay thin-scroll" role="listbox" aria-label="Add context" data-placement={below ? "below" : "above"}>
        <div className="context-menu-sheet-head" role="presentation">
          <span>Add context</span>
          <span className="context-menu-query">@{query}</span>
        </div>
        {ORDER.map((section) => {
          const rows = items.filter((item) => item.section === section);
          if (!rows.length) return null;
          return (
            <div key={section} role="group" aria-labelledby={`context-${section}`} className="context-menu-group">
              <div id={`context-${section}`} className="context-menu-label" role="presentation">
                {SECTION_LABEL[section]}
              </div>
              {rows.map((item) => {
                const Icon = item.id === "command-stop" ? CircleStop : ICON[section];
                return (
                  <div
                    key={item.id}
                    id={`context-${item.id}`}
                    role="option"
                    aria-selected={items[active]?.id === item.id}
                    className="context-menu-item"
                    onMouseDown={keepFocus}
                    onClick={() => onPick(item)}
                  >
                    <Icon size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
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
