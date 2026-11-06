import { CircleAlert, FileText, RotateCw, X } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import type { Attachment, Visual } from "../api/types";
import { latestTaskState } from "../hooks/useTaskThumbnail";
import { t } from "../i18n";
import { pendingTickets, type SessionView, type TimelineItem } from "../state/session";
import { useClient } from "../context";
import { outgoing, type OutgoingMessage } from "../state/outgoing";
import { retryMessage } from "../state/retry";
import { ICON_STROKE } from "../ui";
import { ArtifactImage } from "./ArtifactImage";
import { citationKey, Markdown, MarkdownContext, type Citation } from "./Markdown";
import { DownloadChip } from "./DownloadChip";
import { RuntimeErrorRow, retrySource } from "./RuntimeError";
import { askerOf } from "../cards/attention";
import { ConversationCard } from "./ConversationCard";
import { MessageDocuments } from "./MessageDocument";
import { MessageMenu, ReplyFooter } from "./MessageMenu";
import { TaskUnit } from "./TaskUnit";
import { WorkGroup, type WorkEntry } from "./ToolActivity";
import { collectToolFacts, safeFavicon, spanSeconds, workLabel, type ToolFactsMap } from "./toolCopy";
import { outcomeNote } from "./toolStatus";
import { separatorLabel, separatorNeeded } from "./timeStamp";
import { VisualFrame } from "./VisualFrame";

/** Fenced ```html visual / ```svg visual blocks; shown as rendered frames instead of text. */
const VISUAL_BLOCK = /```[a-z]+ visual[^\n]*\n[\s\S]*?```\n?/g;

/** Calls that manage Tasks or speak for the agent: the Task rows and the replies already show them. */
const BOOKKEEPING: Record<string, true> = { create_task: true, steer_task: true, cancel_task: true, get_tasks: true, get_models: true, send_message: true };

type UserItem = Extract<TimelineItem, { type: "user" }>;
type AssistantItem = Extract<TimelineItem, { type: "assistant" }>;

const record = (value: unknown): Record<string, unknown> => (value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {});
const text = (value: unknown): string | null => (typeof value === "string" && value.trim() ? value : null);

/** The pages this chat's web searches and web reads returned, from the daemon's source records. */
export function citationsOf(items: readonly TimelineItem[]): Map<string, Citation> {
  const found = new Map<string, Citation>();
  const add = (source: Record<string, unknown>) => {
    const url = text(source.url);
    const key = url ? citationKey(url) : null;
    if (!key || found.has(key)) return;
    const icon = record(source.favicon_ref);
    const origin = text(source.origin);
    found.set(key, {
      title: text(source.title),
      favicon: icon.type === "data" ? safeFavicon(icon.data_url) : null,
      // Only the origin the record names; the client never asks for another site's icon.
      origin: icon.type === "origin" && origin && icon.origin === origin ? origin : null,
    });
  };
  for (const item of items) {
    if (item.type !== "tool" || !item.done || item.isError) continue;
    const name = item.name.toLowerCase();
    const result = record(item.result);
    if (name === "websearch" && Array.isArray(result.sources)) result.sources.forEach((source) => add(record(source)));
    else if (name === "webfetch") add(result);
  }
  return found;
}

/** What follows "Not sent" for the person's newest failed message: the reason, without the outcome word its note leads with. */
function failureDetail(reason: string | undefined): string {
  return reason === "auth_unavailable" ? t("timeline.thread.authUnavailableDetail") : (reason ?? "");
}

/** One message of the person's, right-aligned, with a note under it only when it went wrong or is still on its way. */
function UserBubble({
  sessionId,
  text,
  attachments,
  fileNames,
  note,
  sending,
  failure,
}: {
  sessionId: string;
  text: string;
  attachments?: Attachment[];
  /** Files of a message that is still on this device: no artifact exists to preview yet. */
  fileNames?: string[];
  note?: string;
  sending?: boolean;
  failure?: ReactNode;
}) {
  return (
    <>
      {attachments?.length || fileNames?.length ? (
        <ul className="attachments" aria-label={t("timeline.thread.attachments")}>
          {attachments?.map((attachment) => (
            <li key={attachment.path}>
              {attachment.kind === "image" ? (
                <ArtifactImage sessionId={sessionId} path={attachment.path} alt={attachment.name ?? attachment.path} />
              ) : (
                <span className="attachment-file">
                  <FileText size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  {attachment.name ?? attachment.path}
                </span>
              )}
            </li>
          ))}
          {fileNames?.map((name, index) => (
            <li key={`${index}-${name}`}>
              <span className="attachment-file">
                <FileText size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                {name}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
      {text ? <p>{text}</p> : null}
      {note ? <span className="message-outcome">{note}</span> : null}
      {sending ? (
        <span className="receipt" data-state="sending">
          {t("timeline.thread.sending")}
        </span>
      ) : null}
      {failure}
    </>
  );
}

/** A message this device sent that the daemon has not (yet) echoed into the conversation. */
function OutgoingBubble({ sessionId, message, latest, onQuote }: { sessionId: string; message: OutgoingMessage; latest: boolean; onQuote?: ((text: string) => void) | undefined }) {
  const client = useClient();
  const failed = message.state === "failed";
  return (
    <MessageMenu
      className="bubble user"
      data-message={message.message_id}
      data-receipt={failed ? "failed" : message.state === "sending" ? "sending" : "sent"}
      text={message.text}
      onQuote={onQuote}
    >
      <UserBubble
        sessionId={sessionId}
        text={message.text}
        fileNames={message.attachments.flatMap((file) => (file.name ? [file.name] : []))}
        sending={!failed && latest && message.state === "sending"}
        failure={
          failed ? (
            <FailedReceipt
              detail={message.error ?? ""}
              retry={() =>
                outgoing.submit(client, { message_id: message.message_id, session_id: message.session_id, text: message.text, attachments: message.attachments })
              }
              onDismiss={() => outgoing.forget(message.message_id)}
            />
          ) : undefined
        }
      />
    </MessageMenu>
  );
}

/**
 * "Not sent" with the way forward. A message still held on this device is posted again under its
 * own id; one the agent run dropped (interrupted) goes out as a new message with the same words.
 */
function FailedReceipt({
  detail,
  label,
  retry,
  onDismiss,
}: {
  detail?: string;
  /** Replaces "Not sent" when the message did arrive and the run behind it did not finish. */
  label?: string;
  /** Sends the message again; the button stays disabled until that attempt settles. */
  retry?: () => Promise<unknown>;
  onDismiss?: () => void;
}) {
  const [retrying, setRetrying] = useState(false);
  return (
    <span className="receipt receipt-failed" data-state="failed">
      <CircleAlert size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <span>{label ?? (detail ? t("timeline.thread.notSentDetail", { detail }) : t("timeline.thread.notSent"))}</span>
      {retry ? (
        <button
          type="button"
          className="receipt-action"
          disabled={retrying}
          onClick={() => {
            setRetrying(true);
            void retry().finally(() => setRetrying(false));
          }}
        >
          <RotateCw size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {t("common.retry")}
        </button>
      ) : null}
      {onDismiss ? (
        <button type="button" className="receipt-action" aria-label={t("timeline.thread.discard")} onClick={onDismiss}>
          <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      ) : null}
    </span>
  );
}

/** Wall-clock milliseconds, once a second while `active`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

/** The open turn's work: "Working for 4s", counting from the turn's start. */
function LiveWork({ entries, facts, start }: { entries: readonly WorkEntry[]; facts: ToolFactsMap; start: string | undefined }) {
  const now = useNow(true);
  return <WorkGroup entries={entries} facts={facts} label={workLabel(spanSeconds(start, now), true)} live />;
}

interface Group {
  key: string;
  entries: WorkEntry[];
  start: string | undefined;
  end: string | undefined;
}

export function Timeline({
  view,
  sessionId,
  visuals = {},
  outgoing: sending = [],
  browserCard,
  inertCardId,
  onShowPending = () => undefined,
  onQuote,
  onFork,
}: {
  view: SessionView;
  sessionId: string;
  visuals?: Record<number, Visual[]>;
  /** Messages sent from this device that the event log does not know yet. */
  outgoing?: readonly OutgoingMessage[];
  /** The live browser, shown right under the message that started the browsing (`null`: no such message, so first). */
  browserCard?: { anchorItemId: string | null; node: ReactNode };
  /** A card hosted somewhere else right now (a sheet); its inline copy stays visible but inert. */
  inertCardId?: string | null;
  /** Brings the card waiting on the person into view. */
  onShowPending?: () => void;
  /** Puts a quoted message into the composer; absent where nothing can be written. */
  onQuote?: ((text: string) => void) | undefined;
  /** Branches the chat at the reply to the person's message `messageId`; absent where a chat cannot branch. */
  onFork?: ((messageId: string) => Promise<void>) | undefined;
}) {
  const client = useClient();
  const facts = useMemo(() => collectToolFacts(view.items, view.browserStates), [view.items, view.browserStates]);
  const citations = useMemo(() => citationsOf(view.items), [view.items]);
  const scope = useMemo(() => ({ sessionId, citations }), [sessionId, citations]);
  const rows: ReactNode[] = [];
  const cardRow = browserCard ? (
    <li key="browser" className="browser-row">
      {browserCard.node}
    </li>
  ) : null;
  if (cardRow && browserCard?.anchorItemId === null) rows.push(cardRow);

  // The newest turn is open while the run goes on, unless its last words are out and the model is
  // not thinking again. Its words so far are narration inside the work; they become the reply once
  // the turn closes. Until the agent has picked the message up there is nothing to show.
  const last = view.items.at(-1);
  const unanswered = last?.type === "user" && last.outcome === undefined;
  const answered = last?.type === "assistant" && last.finalSeq !== undefined && !view.thinking;
  const turnOpen = view.status === "running" && !answered;
  const live = turnOpen && pendingTickets(view).length === 0 && view.delivering === null && !(unanswered && !view.engaged);

  // A turn runs from one of the person's messages to the next; its last finished words are its
  // reply, everything before them is its work.
  const replies = new Set<string>();
  // A Task started in a turn with a reply moves after that reply, never among the work.
  const tasks = new Map(view.subagents.filter((child) => child.background).map((child) => [child.parentToolCallId, child]));
  const deferTo = new Map<string, string>();
  {
    let candidate: string | null = null;
    let started: string[] = [];
    const end = (reply: string | null) => {
      if (reply) {
        replies.add(reply);
        for (const id of started) deferTo.set(id, reply);
      }
      candidate = null;
      started = [];
    };
    for (const item of view.items) {
      if (item.type === "user" && item.author_kind !== "internal") end(candidate);
      else if (item.type === "assistant" && item.finalSeq !== undefined) candidate = item.id;
      else if (item.type === "tool" && tasks.has(item.id)) started.push(item.id);
    }
    end(turnOpen ? null : candidate);
  }
  const newestReply = [...replies].at(-1);

  // The one delivery bubble of the thread: a message accepted while idle, not yet handed over.
  const delivering = view.delivering ? view.queue.find((message) => message.id === view.delivering && message.author_kind !== "internal") : undefined;
  const newestPerson = view.items.findLast((it) => it.type === "user" && it.author_kind === "person");
  const known = new Set([...view.queue.map((message) => message.id), ...view.items.flatMap((item) => (item.type === "user" ? [item.id] : []))]);
  const unechoed = sending.filter((message) => !known.has(message.message_id));
  const lastUnechoed = unechoed.at(-1);

  let previousAt: string | undefined;
  const stamp = (key: string, at: string | undefined) => {
    if (!at) return;
    if (separatorNeeded(previousAt, at)) {
      rows.push(
        <li key={`time-${key}`} className="time-separator">
          <time dateTime={at}>{separatorLabel(at)}</time>
        </li>,
      );
    }
    previousAt = at;
  };

  const turn: { user: UserItem | undefined; start: string | undefined; worked: boolean } = { user: undefined, start: undefined, worked: false };
  const work: { group: Group | null } = { group: null };
  const addWork = (key: string, entry: WorkEntry, start: string | undefined, end: string | undefined) => {
    if (!work.group) {
      // The turn's first work counts from the person's message; later work (after a card) from itself.
      work.group = { key: `w-${key}`, entries: [], start: turn.worked ? start : (turn.start ?? start), end: undefined };
      turn.worked = true;
    }
    work.group.entries.push(entry);
    if (end) work.group.end = end;
  };
  const closeWork = (end?: string) => {
    const group = work.group;
    if (!group) return;
    work.group = null;
    rows.push(
      <li key={group.key} className="work-row">
        <WorkGroup entries={group.entries} facts={facts} label={workLabel(spanSeconds(group.start, end ?? group.end), false)} live={false} />
      </li>,
    );
  };

  const deferred = new Map<string, ReactNode[]>();
  // Only the newest Task still at work opens its browser by itself; phones have room for one.
  const browsing = new Set(Object.values(view.browserStates).flatMap((state) => (state.task_id ? [state.task_id] : [])));
  const newestActive = view.subagents.findLast((child) => child.background && browsing.has(child.id) && (child.status === "running" || child.status === "waiting_human"))?.id;
  // A request is its card, so the call that raised it adds nothing next to it, and neither does
  // a `present` call whose card is on the timeline. An approved call's own outcome arrives on
  // its card too. A `present` call that failed has no card and keeps its row.
  const ticketCalls = new Set(Object.values(view.tickets).map((ticket) => ticket.tool_call_id));
  // Questions waiting for the person come to the end of the conversation, oldest first, so the
  // newest thing on screen is the thing that needs them.
  const waiting: { id: string; firstSeq: number }[] = [];
  const conversationRow = (id: string) => {
    const card = view.cards[id]!.card;
    const ticket = card.ticket_id ? view.tickets[card.ticket_id] : undefined;
    const child = ticket?.origin.subagent_id ? view.subagents.find((candidate) => candidate.id === ticket.origin.subagent_id) : undefined;
    return (
      <li key={`k-${id}`} className="card-row" data-card={id}>
        <ConversationCard card={card} sessionId={sessionId} inert={inertCardId === id} asker={ticket ? askerOf(ticket, child?.task) : null} />
      </li>
    );
  };

  const words = (item: AssistantItem) => {
    const frames = item.finalSeq !== undefined ? (visuals[item.finalSeq] ?? []) : [];
    const shown = frames.length ? item.text.replace(VISUAL_BLOCK, "").trim() : item.text.trim();
    return { frames, shown };
  };

  const reply = (item: AssistantItem) => {
    const { frames, shown } = words(item);
    if (work.group) closeWork(item.at);
    else if (!turn.worked) {
      // A turn with no steps still says how long it took, as the line above its reply.
      const seconds = spanSeconds(turn.start, item.at);
      if (seconds) {
        rows.push(
          <li key={`w-${item.id}`} className="work-row">
            <WorkGroup entries={[]} facts={facts} label={workLabel(seconds, false)} live={false} />
          </li>,
        );
      }
    }
    turn.worked = true;
    stamp(item.id, item.at);
    const from = turn.user;
    // Branching copies the conversation up to this reply, so it needs the person's message it answers.
    const forkable = onFork && from && from.author_kind === "person" && from.outcome !== undefined;
    rows.push(
      <MessageMenu key={item.id} className="bubble assistant" data-final={item.final} data-latest={item.id === newestReply || undefined} text={shown} onQuote={onQuote}>
        {item.documents ? <MessageDocuments sessionId={sessionId} documents={item.documents} /> : null}
        {shown ? <Markdown text={shown} /> : null}
        {item.final ? <ReplyFooter text={shown} at={item.at} {...(forkable ? { onFork: () => onFork(from.id) } : {})} /> : null}
      </MessageMenu>,
    );
    if (frames.length) {
      rows.push(
        <li key={`${item.id}-visuals`} className="visuals">
          {frames.map((visual) => (
            <VisualFrame key={visual.id} visual={visual} />
          ))}
        </li>,
      );
    }
    const units = deferred.get(item.id);
    if (units) rows.push(...units);
  };

  for (const item of view.items) {
    switch (item.type) {
      case "tool": {
        if (ticketCalls.has(item.id) || view.cards[`present:${item.id}`]) break;
        const task = tasks.get(item.id);
        if (!task) {
          if (!BOOKKEEPING[item.name]) addWork(item.id, { kind: "tool", item }, item.startedAt, item.endedAt ?? item.startedAt);
          break;
        }
        const unit = (
          <li key={`task-${task.id}`} className="task-row">
            <TaskUnit sessionId={sessionId} task={task} state={latestTaskState(view, task.id)} newest={task.id === newestActive} onShowPending={onShowPending} />
          </li>
        );
        const after = deferTo.get(item.id);
        if (after) {
          deferred.set(after, [...(deferred.get(after) ?? []), unit]);
          break;
        }
        closeWork();
        rows.push(unit);
        break;
      }
      case "user": {
        // Internal notices steer models; they are never the person's words.
        if (item.author_kind === "internal") break;
        closeWork();
        turn.user = item;
        turn.start = item.at;
        turn.worked = false;
        stamp(item.id, item.at);
        const failed = item.outcome === "failed";
        // Only the newest failed message is offered again, and not while something else is on its way.
        const newest = item === newestPerson;
        const idle = sending.length === 0 && view.queue.length === 0 && view.status !== "running";
        const note = item.outcome && item.outcome !== "completed" && item.outcome !== "deferred" ? outcomeNote(item.outcome, item.reason) : undefined;
        // The agent's own error row below carries the retry for a run that died; one is enough.
        const errorFollows = view.items.slice(view.items.indexOf(item) + 1).some((it) => it.type === "error");
        rows.push(
          <MessageMenu
            key={`u-${item.id}`}
            className="bubble user"
            data-message={item.id}
            {...(failed && newest ? { "data-receipt": "failed" } : {})}
            text={item.text}
            onQuote={onQuote}
          >
            <UserBubble
              sessionId={sessionId}
              text={item.text}
              {...(item.attachments ? { attachments: item.attachments } : {})}
              {...(note && !(failed && newest) ? { note } : {})}
              {...(failed && newest
                ? {
                    failure: (
                      <FailedReceipt
                        {...(item.reason === "interrupted" ? { label: t("timeline.thread.notFinished") } : { detail: failureDetail(item.reason) })}
                        {...(idle && !errorFollows ? { retry: () => retryMessage(client, sessionId, item) } : {})}
                      />
                    ),
                  }
                : {})}
            />
          </MessageMenu>,
        );
        if (cardRow && browserCard?.anchorItemId === item.id) rows.push(cardRow);
        break;
      }
      case "assistant": {
        // Deltas may precede a tool call, so only an explicit final is conversational text.
        if (item.finalSeq === undefined) break;
        if (replies.has(item.id)) {
          reply(item);
          break;
        }
        const { frames, shown } = words(item);
        if (!shown && !frames.length && !item.documents) break;
        addWork(
          item.id,
          {
            kind: "note",
            id: item.id,
            node: (
              <>
                {item.documents ? <MessageDocuments sessionId={sessionId} documents={item.documents} /> : null}
                {shown ? <Markdown text={shown} /> : null}
                {frames.map((visual) => (
                  <VisualFrame key={visual.id} visual={visual} />
                ))}
              </>
            ),
          },
          item.at,
          item.at,
        );
        break;
      }
      case "card": {
        const entry = view.cards[item.id];
        if (!entry) break;
        closeWork();
        if (entry.card.interaction === "ask" && entry.card.status === "pending") waiting.push({ id: item.id, firstSeq: entry.firstSeq });
        else rows.push(conversationRow(item.id));
        break;
      }
      case "download":
        closeWork();
        rows.push(
          <li key={`dl-${item.id}`} className="download-row">
            <DownloadChip sessionId={sessionId} item={item} />
          </li>,
        );
        break;
      case "error": {
        closeWork();
        // A follow-up still waiting its turn would run after the retried message and repeat the
        // person's intent, so Retry waits until nothing else is on its way.
        const settled = sending.length === 0 && view.queue.length === 0 && view.status !== "running";
        const retry = settled ? retrySource(view.items, view.items.indexOf(item)) : undefined;
        rows.push(
          <li key={item.id} className="runtime-error-row">
            <RuntimeErrorRow error={item} sessionId={sessionId} {...(retry ? { retry } : {})} />
          </li>,
        );
        break;
      }
    }
  }
  if (live) {
    const group = work.group;
    work.group = null;
    rows.push(
      <li key={group?.key ?? "work-live"} className="work-row">
        <LiveWork entries={group?.entries ?? []} facts={facts} start={group?.start ?? turn.start} />
      </li>,
    );
  } else closeWork();
  for (const entry of waiting.sort((a, b) => a.firstSeq - b.firstSeq)) rows.push(conversationRow(entry.id));

  if (delivering) {
    rows.push(
      <MessageMenu key={`d-${delivering.id}`} className="bubble user" data-message={delivering.id} text={delivering.text} onQuote={onQuote}>
        <UserBubble sessionId={sessionId} text={delivering.text} {...(delivering.attachments ? { attachments: delivering.attachments } : {})} />
      </MessageMenu>,
    );
  }
  for (const message of unechoed) {
    rows.push(<OutgoingBubble key={`o-${message.message_id}`} sessionId={sessionId} message={message} latest={message === lastUnechoed} onQuote={onQuote} />);
  }

  return (
    <MarkdownContext.Provider value={scope}>
      <ol className="timeline" aria-live="polite">
        {rows}
      </ol>
    </MarkdownContext.Provider>
  );
}
