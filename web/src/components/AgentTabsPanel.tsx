import { Globe, LayoutList, X } from "lucide-react";
import { useContext, useEffect, useMemo, type RefObject } from "react";
import { AgentTabsContext, AgentTabsSettledContext, type OwnedTab } from "../hooks/useAgentTabs";
import { useBrowserPreview } from "../hooks/useBrowserPreview";
import { originOf, useFavicon } from "../hooks/useFavicon";
import { useTaskThumbnail } from "../hooks/useTaskThumbnail";
import { t } from "../i18n";
import type { BrowserStateView, SessionView } from "../state/session";
import { ICON_STROKE, IconButton } from "../ui";
import { AGENT_TABS_MIN_WIDTH, setAgentTabsFit, syncAgentTabs, useAgentTabsPanel } from "./agentTabsStore";
import { useBrowserDock } from "./browserDock";
import { BrowserFrame } from "./BrowserFrame";
import { safeFavicon } from "./toolCopy";
import "./agentTabs.css";

/** The newest `browser.state` the conversation has for one tab, and the newest one with a picture. */
function tabStates(view: SessionView, tabId: string): { latest?: BrowserStateView; pictured?: BrowserStateView } {
  let latest: BrowserStateView | undefined;
  let pictured: BrowserStateView | undefined;
  for (const state of Object.values(view.browserStates)) {
    if (state.tab_id !== tabId) continue;
    if (!latest || state.seq > latest.seq) latest = state;
    if (state.thumbnail && (!pictured || state.seq > pictured.seq)) pictured = state;
  }
  return { latest, pictured };
}

/**
 * Tab ids in the order the conversation's browser calls first named them (the opening call's
 * result, or a later call's `tab_id` argument): when each tab was opened, as far as the log tells.
 */
function openedOrder(items: SessionView["items"]): string[] {
  const ids: string[] = [];
  const note = (value: unknown) => {
    if (typeof value === "string" && !ids.includes(value)) ids.push(value);
  };
  for (const item of items) {
    if (item.type !== "tool") continue;
    if (item.result && typeof item.result === "object") note((item.result as Record<string, unknown>).tab_id);
    if (item.args && typeof item.args === "object") note((item.args as Record<string, unknown>).tab_id);
  }
  return ids;
}

export interface AgentTabsPanelProps {
  sessionId: string;
  view: SessionView;
  /** The conversation's `<main>`: its width decides whether the column fits beside the thread. */
  container: RefObject<HTMLElement | null>;
}

/**
 * The desktop Agent Tabs column beside a conversation: one card per agent tab this conversation
 * owns, newest first, with the tab's live screencast (the tab the agent is on) or its last
 * picture. It appears by itself when the agent opens a tab and gives way to the inline card and
 * PiP when the conversation is too narrow. A card opens the browser sheet on its tab; hiding a
 * card leaves the tab open. Private conversations never show it.
 */
export function AgentTabsPanel({ sessionId, view, container }: AgentTabsPanelProps) {
  const panel = useAgentTabsPanel(sessionId);
  const tabs = useContext(AgentTabsContext);
  const settled = useContext(AgentTabsSettledContext);
  const ids = tabs.flatMap((tab) => (tab.owner_session_id === sessionId ? [tab.tab_id] : [])).join("\n");
  const opened = useMemo(() => openedOrder(view.items).join("\n"), [view.items]);
  // Only the poller's latest successful answer is authoritative: before the first one and after a
  // failure it holds an empty list that says nothing about which tabs are open. The log must have
  // loaded too, or tabs open at page load would be ranked without it.
  const polled = settled && view.lastSeq > 0;
  useEffect(() => {
    if (polled) syncAgentTabs(sessionId, ids ? ids.split("\n") : [], opened.split("\n"));
  }, [sessionId, ids, opened, polled]);

  useEffect(() => {
    const element = container.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => setAgentTabsFit(sessionId, element.getBoundingClientRect().width >= AGENT_TABS_MIN_WIDTH));
    observer.observe(element);
    return () => observer.disconnect();
  }, [container, sessionId]);

  const shown = panel.shown && !view.incognito;
  // One screencast for the column; it follows the tab the agent used last.
  const stream = useBrowserPreview(sessionId, shown);
  const { setSheet } = useBrowserDock(sessionId);
  if (!shown) return null;
  const message = stream.status === "open" ? stream.message : null;
  const frame = message?.type === "frame" ? message : null;
  const liveTabId = message && "tab_id" in message ? message.tab_id : null;
  return (
    <aside className="agent-tabs-panel thin-scroll" aria-label={t("shell.tabs.agentTabs")}>
      <ul className="agent-tabs-list">
        {panel.cards.map((tab) => (
          <li key={tab.tab_id}>
            <TabCard
              sessionId={sessionId}
              tab={tab}
              {...tabStates(view, tab.tab_id)}
              frame={liveTabId === tab.tab_id ? (frame?.data ?? null) : null}
              onOpen={() => setSheet(true, tab.tab_id)}
              onHide={() => panel.dismiss(tab.tab_id)}
            />
          </li>
        ))}
      </ul>
    </aside>
  );
}

function TabCard({
  sessionId,
  tab,
  latest,
  pictured,
  frame,
  onOpen,
  onHide,
}: {
  sessionId: string;
  tab: OwnedTab;
  latest?: BrowserStateView;
  pictured?: BrowserStateView;
  frame: string | null;
  onOpen: () => void;
  onHide: () => void;
}) {
  const origin = originOf(tab.url);
  const stateIcon = latest ? safeFavicon(latest.favicon) : null;
  const icon = useFavicon(stateIcon ? null : origin) ?? stateIcon;
  const host = origin ? new URL(origin).host : "";
  const title = tab.title.trim() || host || tab.url;
  // The newest picture the log has for the tab, kept while the screencast is on it so it is ready
  // the moment the agent moves to another tab.
  const thumbnail = useTaskThumbnail(sessionId, pictured?.thumbnail ?? undefined, true);
  const picture = frame ? <BrowserFrame data={frame} /> : thumbnail ? <img src={thumbnail} alt="" draggable={false} /> : null;
  return (
    <article className="agent-tab-card" data-live={!!frame || undefined} data-empty={!picture || undefined}>
      <button type="button" className="agent-tab-screen" aria-label={t("shell.agentTabs.open", { title })} title={tab.url} onClick={onOpen}>
        {picture}
      </button>
      {/* Drawn over the picture; clicks other than the close button fall through to the screen. */}
      <div className="agent-tab-bar">
        {icon ? (
          <img className="agent-tab-favicon" src={icon} alt="" width={14} height={14} />
        ) : (
          <Globe className="agent-tab-favicon" data-fallback size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        )}
        <span className="agent-tab-title">{title}</span>
        <IconButton label={t("shell.agentTabs.hide", { title })} className="agent-tab-hide" onClick={onHide}>
          <X size={16} strokeWidth={2.5} aria-hidden="true" />
        </IconButton>
      </div>
    </article>
  );
}

/**
 * Header control for the Agent Tabs column once it can show: hides it, or brings it (and any
 * hidden cards) back. Absent on a phone and wherever the column does not fit.
 */
export function AgentTabsToggle({ sessionId, view }: { sessionId: string; view: SessionView }) {
  const { available, shown, toggle } = useAgentTabsPanel(sessionId);
  if (!available || view.incognito) return null;
  return (
    <IconButton label={shown ? t("shell.agentTabs.hidePanel") : t("shell.agentTabs.showPanel")} aria-pressed={shown} className="agent-tabs-toggle" onClick={toggle}>
      <LayoutList size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </IconButton>
  );
}
