import type { Attachment, BrowserState, Event, PolicyMode, QueueMode, SubagentStatus, Ticket } from "../api/types";

export interface QueuedMessage {
  id: string;
  text: string;
  mode: QueueMode;
  attachments?: Attachment[];
}

export type MessageOutcome = "completed" | "aborted" | "failed" | "deferred";

export type TimelineItem =
  | { type: "user"; id: string; text: string; outcome?: MessageOutcome; reason?: string; attachments?: Attachment[] }
  /** `finalSeq` is the `assistant.final` sequence that visuals (`Visual.seq`) refer to. */
  | { type: "assistant"; id: string; text: string; final: boolean; finalSeq?: number }
  | {
      type: "tool";
      id: string;
      name: string;
      args: unknown;
      updates: unknown[];
      result?: unknown;
      isError?: boolean;
      done: boolean;
    }
  | { type: "ticket"; id: string }
  | { type: "error"; id: string; code: string; message: string };

export interface SessionView {
  lastSeq: number;
  title: string;
  model: string;
  mode: PolicyMode | undefined;
  pinned: boolean;
  archived: boolean;
  incognito: boolean;
  status: "idle" | "running" | "stopped";
  /** Model is producing thinking output and no visible text yet. */
  thinking: boolean;
  items: TimelineItem[];
  /** Pending messages in delivery order: steering lane first, then follow-ups. */
  queue: QueuedMessage[];
  tickets: Record<string, Ticket>;
  /** Children started by `delegate` or `task_start` (`background`), in start order. */
  subagents: SubagentView[];
  /** Latest `browser.state` per agent tab, keyed by {@link browserStateKey}. */
  browserStates: Record<string, BrowserStateView>;
  /** Files tools saved into the artifact directory, latest write per path, in first-save order. */
  artifacts: SavedArtifact[];
}

export interface SubagentView {
  id: string;
  task: string;
  model: string;
  tools: string[];
  parentToolCallId: string;
  status: SubagentStatus;
  /** A Task the front agent started; it reports back with a `task-report-<id>` message. */
  background: boolean;
  reason?: string;
  /** Final answer of a completed child. */
  output?: string;
  /** Every `subagent.progress` payload in order: the child's live event stream. */
  progress: unknown[];
}

/** `seq` orders states across tabs: the highest is the most recent activity. */
export type BrowserStateView = BrowserState & { seq: number };

export interface SavedArtifact {
  path: string;
  mime: string;
  size: number;
  tool: string;
}

export const emptySessionView: SessionView = {
  lastSeq: 0,
  title: "",
  model: "",
  mode: undefined,
  pinned: false,
  archived: false,
  incognito: false,
  status: "idle",
  thinking: false,
  items: [],
  queue: [],
  tickets: {},
  subagents: [],
  browserStates: {},
  artifacts: [],
};

/** One card per Task and tab; the front agent's reads use the `front` slot. */
export function browserStateKey(state: Pick<BrowserState, "task_id" | "tab_id">): string {
  return `${state.task_id ?? "front"}:${state.tab_id}`;
}

function orderQueue(queue: QueuedMessage[]): QueuedMessage[] {
  return [...queue.filter((m) => m.mode === "steer"), ...queue.filter((m) => m.mode === "queue")];
}

function patchItem<T extends TimelineItem["type"]>(
  items: TimelineItem[],
  type: T,
  id: string,
  patch: (item: Extract<TimelineItem, { type: T }>) => Extract<TimelineItem, { type: T }>,
): TimelineItem[] {
  return items.map((item) =>
    item.type === type && item.id === id ? patch(item as Extract<TimelineItem, { type: T }>) : item,
  );
}

/**
 * Pure reducer from the durable event log to everything the session screen shows.
 * The screen never keeps state that cannot be rebuilt by replaying events, so a
 * reload or reconnect reproduces the same view (events at or below `lastSeq` are ignored).
 */
export function reduceSession(state: SessionView, event: Event): SessionView {
  if (event.seq <= state.lastSeq) return state;
  const next: SessionView = { ...state, lastSeq: event.seq };
  const last = state.items.at(-1);
  switch (event.kind) {
    case "session.created":
      return { ...next, title: event.title, model: event.model, mode: event.mode ?? undefined, incognito: event.incognito ?? false };
    case "session.updated":
      return {
        ...next,
        title: event.title ?? state.title,
        pinned: event.pinned ?? state.pinned,
        archived: event.archived ?? state.archived,
      };
    case "message.queued": {
      const attachments = event.attachments?.length ? { attachments: event.attachments } : {};
      // An idle session starts on this message; say so now rather than after the agent
      // process is up, which takes seconds when it is cold.
      const starting = state.status !== "running";
      return {
        ...next,
        ...(starting ? { status: "running" as const, thinking: true } : {}),
        queue: orderQueue([
          ...state.queue,
          { id: event.message_id, text: event.text, mode: event.mode, ...attachments },
        ]),
      };
    }
    case "message.edited":
      return {
        ...next,
        queue: state.queue.map((m) => (m.id === event.message_id ? { ...m, text: event.text } : m)),
      };
    case "message.promoted": {
      const promoted = state.queue.find((m) => m.id === event.message_id);
      if (!promoted) return next;
      const rest = state.queue.filter((m) => m.id !== event.message_id);
      return { ...next, queue: orderQueue([...rest, { ...promoted, mode: "steer" }]) };
    }
    case "message.moved": {
      const moved = state.queue.find((m) => m.id === event.message_id);
      if (!moved) return next;
      const lane = state.queue.filter((m) => m.mode === moved.mode && m.id !== event.message_id);
      lane.splice(Math.min(event.position, lane.length), 0, moved);
      return {
        ...next,
        queue: orderQueue([...state.queue.filter((m) => m.mode !== moved.mode), ...lane]),
      };
    }
    case "message.dropped": {
      const queue = state.queue.filter((m) => m.id !== event.message_id);
      const busy = queue.length > 0 || state.items.some((item) => item.type === "user" && item.outcome === undefined);
      return { ...next, queue, ...(busy ? {} : { status: "idle" as const, thinking: false }) };
    }
    case "message.injected": {
      const sent = state.queue.find((m) => m.id === event.message_id);
      return {
        ...next,
        status: "running",
        queue: state.queue.filter((m) => m.id !== event.message_id),
        items: [
          ...finalizeStreaming(state.items),
          {
            type: "user",
            id: event.message_id,
            text: sent?.text ?? "",
            ...(sent?.attachments ? { attachments: sent.attachments } : {}),
          },
        ],
      };
    }
    case "message.completed": {
      const items = patchItem(state.items, "user", event.message_id, (item) => ({
        ...item,
        outcome: event.outcome,
        reason: event.reason ?? undefined,
      }));
      const running = items.some((item) => item.type === "user" && item.outcome === undefined);
      return {
        ...next,
        status: state.status === "stopped" ? "stopped" : running ? "running" : "idle",
        thinking: running && state.thinking,
        items,
      };
    }
    case "turn.started":
      return { ...next, status: "running" };
    case "turn.ended":
      return { ...next, thinking: false, items: finalizeStreaming(state.items) };
    case "session.settled":
      return { ...next, status: "idle", thinking: false, items: finalizeStreaming(state.items) };
    case "session.stopped":
      return { ...next, status: "stopped", thinking: false, items: finalizeStreaming(state.items) };
    case "thinking.delta":
      return { ...next, thinking: true };
    case "assistant.delta":
      if (last?.type === "assistant" && !last.final) {
        return {
          ...next,
          thinking: false,
          items: [...state.items.slice(0, -1), { ...last, text: last.text + event.text }],
        };
      }
      return {
        ...next,
        thinking: false,
        items: [...state.items, { type: "assistant", id: `a${event.seq}`, text: event.text, final: false }],
      };
    case "assistant.final":
      // The sidecar emits `turn.ended` (which closes the streamed bubble) before the run's
      // `assistant.final`, so the final replaces that bubble instead of duplicating it.
      if (last?.type === "assistant" && last.finalSeq === undefined) {
        return {
          ...next,
          thinking: false,
          items: [...state.items.slice(0, -1), { ...last, text: event.text, final: true, finalSeq: event.seq }],
        };
      }
      return {
        ...next,
        thinking: false,
        items: [
          ...state.items,
          { type: "assistant", id: `a${event.seq}`, text: event.text, final: true, finalSeq: event.seq },
        ],
      };
    case "tool.started":
      return {
        ...next,
        thinking: false,
        items: [
          ...finalizeStreaming(state.items),
          { type: "tool", id: event.tool_call_id, name: event.name, args: event.args, updates: [], done: false },
        ],
      };
    case "tool.updated":
      return {
        ...next,
        items: patchItem(state.items, "tool", event.tool_call_id, (item) => ({
          ...item,
          updates: [...item.updates, event.update],
        })),
      };
    case "tool.ended":
      return {
        ...next,
        items: patchItem(state.items, "tool", event.tool_call_id, (item) => ({
          ...item,
          result: event.result,
          isError: event.is_error,
          done: true,
        })),
      };
    case "ticket.opened":
      return {
        ...next,
        tickets: { ...state.tickets, [event.ticket.id]: event.ticket },
        items: [...finalizeStreaming(state.items), { type: "ticket", id: event.ticket.id }],
      };
    case "ticket.resolved":
    case "ticket.rejected":
    case "ticket.expired": {
      const ticket = state.tickets[event.ticket_id];
      if (!ticket) return next;
      const updated: Ticket =
        event.kind === "ticket.resolved"
          ? { ...ticket, status: "resolved", resolution: event.resolution }
          : event.kind === "ticket.rejected"
            ? { ...ticket, status: "rejected", resolution: { status: "rejected", reason: event.reason } }
            : { ...ticket, status: "expired" };
      return { ...next, tickets: { ...state.tickets, [ticket.id]: updated } };
    }
    case "subagent.started":
      return {
        ...next,
        subagents: [
          ...state.subagents.filter((child) => child.id !== event.subagent_id),
          {
            id: event.subagent_id,
            task: event.task,
            model: event.model,
            tools: event.tools,
            parentToolCallId: event.parent_tool_call_id,
            status: "running",
            background: event.background ?? false,
            progress: [],
          },
        ],
      };
    case "subagent.lifecycle":
      return {
        ...next,
        subagents: state.subagents.map((child) =>
          child.id === event.subagent_id ? { ...child, status: event.status as SubagentStatus } : child,
        ),
      };
    case "subagent.progress":
      return {
        ...next,
        subagents: state.subagents.map((child) =>
          child.id === event.subagent_id ? { ...child, progress: [...child.progress, event.progress] } : child,
        ),
      };
    case "subagent.ended":
      return {
        ...next,
        subagents: state.subagents.map((child) =>
          child.id === event.subagent_id
            ? {
                ...child,
                status: event.status,
                ...(event.reason ? { reason: event.reason } : {}),
                ...(event.output ? { output: event.output } : {}),
              }
            : child,
        ),
      };
    case "browser.state": {
      const browser: BrowserStateView = {
        tab_id: event.tab_id, url: event.url, host: event.host, title: event.title,
        phase: event.phase, page_kind: event.page_kind, facts: event.facts, seq: event.seq,
        ...(event.task_id != null ? { task_id: event.task_id } : {}),
        ...(event.action != null ? { action: event.action } : {}),
        ...(event.thumbnail != null ? { thumbnail: event.thumbnail } : {}),
      };
      return { ...next, browserStates: { ...state.browserStates, [browserStateKey(browser)]: browser } };
    }
    case "artifact.saved": {
      const saved: SavedArtifact = { path: event.path, mime: event.mime, size: event.size, tool: event.tool };
      const known = state.artifacts.some((artifact) => artifact.path === event.path);
      return {
        ...next,
        artifacts: known
          ? state.artifacts.map((artifact) => (artifact.path === event.path ? saved : artifact))
          : [...state.artifacts, saved],
      };
    }
    case "runtime.error":
      return {
        ...next,
        items: [
          ...finalizeStreaming(state.items),
          { type: "error", id: `e${event.seq}`, code: event.code, message: event.message },
        ],
      };
    default:
      return next;
  }
}

/** A turn boundary or interruption closes all assistant text still streaming. */
function finalizeStreaming(items: TimelineItem[]): TimelineItem[] {
  if (!items.some((item) => item.type === "assistant" && !item.final)) return items;
  return items.map((item) => (item.type === "assistant" && !item.final ? { ...item, final: true } : item));
}

export function replaySession(events: Iterable<Event>, from: SessionView = emptySessionView): SessionView {
  let state = from;
  for (const event of events) state = reduceSession(state, event);
  return state;
}

export function pendingTickets(view: SessionView): Ticket[] {
  return Object.values(view.tickets).filter((ticket) => ticket.status === "pending");
}
