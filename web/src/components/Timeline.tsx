import { CircleAlert, FileText, RotateCw, X } from "lucide-react";
import { useMemo, type ReactNode } from "react";
import type { Attachment, Visual } from "../api/types";
import { pendingTickets, type SessionView } from "../state/session";
import { newMessageId, useClient } from "../context";
import { outgoing, type OutgoingMessage } from "../state/outgoing";
import { ICON_STROKE } from "../ui";
import { ArtifactImage } from "./ArtifactImage";
import { Markdown } from "./Markdown";
import { RuntimeErrorRow, retrySource } from "./RuntimeError";
import { askerOf } from "../cards/attention";
import { ConversationCard } from "./ConversationCard";
import { MessageDocuments } from "./MessageDocument";
import { TaskUnit } from "./TaskUnit";
import { Activity, StatusLine } from "./ToolActivity";
import { collectToolFacts, type ToolItem } from "./toolCopy";
import { lastHost, outcomeNote, statusPhrase } from "./toolStatus";
import { separatorLabel, separatorNeeded } from "./timeStamp";
import { VisualFrame } from "./VisualFrame";

/** Fenced ```html visual / ```svg visual blocks; shown as rendered frames instead of text. */
const VISUAL_BLOCK = /```[a-z]+ visual[^\n]*\n[\s\S]*?```\n?/g;

/** Calls that manage Tasks or speak for the agent: the Task rows and the bubbles already show them. */
const BOOKKEEPING: Record<string, true> = { create_task: true, steer_task: true, cancel_task: true, get_tasks: true, get_models: true, send_message: true };

type Receipt = "sending" | "sent" | "read";

const RECEIPT_LABEL: Record<Receipt, string> = { sending: "Sending…", sent: "Sent", read: "Read" };

/** One message of the person's, right-aligned, with the delivery note messengers put under it. */
function UserBubble({
  sessionId,
  text,
  attachments,
  fileNames,
  note,
  receipt,
  failure,
}: {
  sessionId: string;
  text: string;
  attachments?: Attachment[];
  /** Files of a message that is still on this device: no artifact exists to preview yet. */
  fileNames?: string[];
  note?: string;
  receipt?: Receipt;
  failure?: ReactNode;
}) {
  return (
    <>
      {attachments?.length || fileNames?.length ? (
        <ul className="attachments" aria-label="Attachments">
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
      {receipt ? (
        <span className="receipt" data-state={receipt}>
          {RECEIPT_LABEL[receipt]}
        </span>
      ) : null}
      {failure}
    </>
  );
}

/** A message this device sent that the daemon has not (yet) echoed into the conversation. */
function OutgoingBubble({ sessionId, message, latest }: { sessionId: string; message: OutgoingMessage; latest: boolean }) {
  const failed = message.state === "failed";
  return (
    <li className="bubble user" data-message={message.message_id} data-receipt={failed ? "failed" : message.state === "sending" ? "sending" : "sent"}>
      <UserBubble
        sessionId={sessionId}
        text={message.text}
        fileNames={message.attachments.flatMap((file) => (file.name ? [file.name] : []))}
        {...(!failed && latest ? { receipt: message.state === "sending" ? ("sending" as const) : ("sent" as const) } : {})}
        failure={
          failed ? (
            <FailedReceipt
              detail={message.error ?? ""}
              retry={{ sessionId: message.session_id, text: message.text, message }}
              onDismiss={() => outgoing.forget(message.message_id)}
            />
          ) : undefined
        }
      />
    </li>
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
  retry?: { sessionId: string; text: string; message?: OutgoingMessage };
  onDismiss?: () => void;
}) {
  const client = useClient();
  return (
    <span className="receipt receipt-failed" data-state="failed">
      <CircleAlert size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <span>{label ?? (detail ? `Not sent · ${detail}` : "Not sent")}</span>
      {retry ? (
        <button
          type="button"
          className="receipt-action"
          onClick={() =>
            void outgoing.submit(client, {
              message_id: retry.message?.message_id ?? newMessageId(),
              session_id: retry.sessionId,
              text: retry.text,
              attachments: retry.message?.attachments ?? [],
            })
          }
        >
          <RotateCw size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
          Retry
        </button>
      ) : null}
      {onDismiss ? (
        <button type="button" className="receipt-action" aria-label="Discard message" onClick={onDismiss}>
          <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      ) : null}
    </span>
  );
}

/**
 * Three dots that bounce one after another while the agent works before its first words; in a
 * front agent's thread they sit in a reply bubble of their own, as in a messenger.
 */
function TypingIndicator() {
  return (
    <p className="typing" role="status">
      <span className="sr-only">Agent is working</span>
      <span className="typing-dot" aria-hidden="true" />
      <span className="typing-dot" aria-hidden="true" />
      <span className="typing-dot" aria-hidden="true" />
    </p>
  );
}

interface Reply {
  key: string;
  parts: ReactNode[];
  final: boolean;
  frames: Visual[];
}

export function Timeline({
  view,
  sessionId,
  visuals = {},
  outgoing: sending = [],
  browserCard,
  inertCardId,
  front = false,
  onShowPending = () => undefined,
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
  /**
   * The front agent's thread: it only talks (each `send_message` is one bubble) and hands work
   * to Tasks, so its own calls never show; a Task is a row with its browser bubble.
   */
  front?: boolean;
  /** Brings the card waiting on the person into view. */
  onShowPending?: () => void;
}) {
  const facts = useMemo(() => collectToolFacts(view.items, view.browserStates), [view.items, view.browserStates]);
  const rows: ReactNode[] = [];
  const cardRow = browserCard ? (
    <li key="browser" className="browser-row">
      {browserCard.node}
    </li>
  ) : null;
  if (cardRow && browserCard?.anchorItemId === null) rows.push(cardRow);
  let tools: ToolItem[] = [];
  // A run without a completed reply or waiting card shows activity; provisional text is internal.
  // Until the agent has picked a message up (`Sent`, `Read`) there is nothing to show yet.
  // The front agent shows typing dots only while it composes a turn: its Tasks keep working
  // after it has answered, and they speak through their own rows.
  const last = view.items.at(-1);
  const unanswered = last?.type === "user" && last.outcome === undefined;
  const live = front
    ? view.composing && pendingTickets(view).length === 0 && !(last?.type === "assistant" && last.finalSeq !== undefined)
    : view.status === "running" &&
      pendingTickets(view).length === 0 &&
      view.delivering === null &&
      !(unanswered && !view.engaged) &&
      (view.thinking || !(last?.type === "assistant" && last.finalSeq !== undefined));
  const newestPerson = view.items.findLast((it) => it.type === "user" && it.author_kind === "person");
  // One delivery note in the whole thread, under the person's newest bubble, wherever it is.
  const delivering = view.delivering ? view.queue.find((message) => message.id === view.delivering && message.author_kind !== "internal") : undefined;
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

  // One reply, one bubble: the agent's text segments of a turn and the calls between them read
  // as one message. Calls before its first words stay a row above the bubble.
  const turn: { reply: Reply | null } = { reply: null };
  const closeReply = () => {
    const reply = turn.reply;
    if (!reply) return;
    turn.reply = null;
    if (reply.parts.length) {
      rows.push(
        <li key={reply.key} className={`bubble assistant${reply.final ? "" : " streaming"}`} data-final={reply.final}>
          {reply.parts}
        </li>,
      );
    }
    if (reply.frames.length) {
      rows.push(
        <li key={`${reply.key}-visuals`} className="visuals">
          {reply.frames.map((visual) => (
            <VisualFrame key={visual.id} visual={visual} />
          ))}
        </li>,
      );
    }
  };
  const place = (key: string, node: ReactNode, className: string, status = false) => {
    if (turn.reply) {
      turn.reply.parts.push(
        <div key={key} className={`reply-activity ${className}`.trim()} {...(status ? { role: "status" } : {})}>
          {node}
        </div>,
      );
    } else {
      rows.push(
        <li key={key} className={`activity-row ${className}`.trim()} {...(status ? { role: "status" } : {})}>
          {node}
        </li>,
      );
    }
  };
  const flush = (liveStatus = false) => {
    const first = tools[0];
    if (first) {
      place(
        `t-${first.id}`,
        liveStatus ? (
          <StatusLine label={view.thinking ? "Thinking" : statusPhrase(tools, lastHost(view.items))} tools={tools} facts={facts} />
        ) : (
          <Activity tools={tools} facts={facts} inline={turn.reply !== null} />
        ),
        liveStatus ? "live-status" : "",
        liveStatus,
      );
    }
    tools = [];
  };

  const tasks = new Map(view.subagents.filter((child) => child.background).map((child) => [child.parentToolCallId, child]));
  // Only the newest Task still at work opens its browser by itself; phones have room for one.
  const browsing = new Set(Object.values(view.browserStates).flatMap((state) => (state.task_id ? [state.task_id] : [])));
  const newestActive = view.subagents.findLast((child) => child.background && browsing.has(child.id) && (child.status === "running" || child.status === "waiting_human"))?.id;
  // A Task started before the turn's completed reply moves after that reply, never after narration.
  const deferTo = new Map<string, string>();
  {
    let started: string[] = [];
    for (const item of view.items) {
      if (item.type === "user") started = [];
      else if (item.type === "tool" && tasks.has(item.id)) started.push(item.id);
      else if (item.type === "assistant" && item.finalSeq !== undefined) for (const id of started) deferTo.set(id, item.id);
    }
  }
  const deferred = new Map<string, ReactNode[]>();
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

  for (const item of view.items) {
    if (item.type === "tool") {
      if (ticketCalls.has(item.id) || view.cards[`present:${item.id}`]) continue;
      const task = tasks.get(item.id);
      if (!task) {
        if (!front && !BOOKKEEPING[item.name]) tools.push(item);
        continue;
      }
      const tab = Object.values(view.browserStates)
        .filter((state) => state.task_id === task.id)
        .reduce<(typeof view.browserStates)[string] | undefined>((latest, state) => (!latest || state.seq > latest.seq ? state : latest), undefined);
      const unit = (
        <li key={`task-${task.id}`} className="task-row">
          <TaskUnit sessionId={sessionId} task={task} state={tab} newest={task.id === newestActive} onShowPending={onShowPending} />
        </li>
      );
      const after = deferTo.get(item.id);
      if (after) {
        deferred.set(after, [...(deferred.get(after) ?? []), unit]);
        continue;
      }
      flush();
      closeReply();
      rows.push(unit);
      continue;
    }
    flush();
    // The front agent's messages are each complete; one bubble per message.
    if (item.type !== "assistant" || front) closeReply();
    switch (item.type) {
      case "user": {
        // Internal notices steer models; they are never the person's words.
        if (item.author_kind === "internal") break;
        stamp(item.id, item.at);
        // The runtime took the message up (`message.injected`), so the person's newest one reads
        // "Read" and keeps it, as in a messenger; "Sent" belongs to the queued bubble before that.
        const failed = item.outcome === "failed";
        // Only the newest failed message is offered again, and not while something else is on its way.
        const newest = item === newestPerson;
        const idle = sending.length === 0 && view.queue.length === 0 && view.status !== "running";
        const note = item.outcome && item.outcome !== "completed" && item.outcome !== "deferred" ? outcomeNote(item.outcome, item.reason) : undefined;
        // The agent's own error row below carries the retry for a run that died; one is enough.
        const errorFollows = view.items.slice(view.items.indexOf(item) + 1).some((it) => it.type === "error");
        rows.push(
          <li key={`u-${item.id}`} className="bubble user" data-message={item.id} {...(failed && newest ? { "data-receipt": "failed" } : {})}>
            <UserBubble
              sessionId={sessionId}
              text={item.text}
              {...(item.attachments ? { attachments: item.attachments } : {})}
              {...(note && !(failed && newest) ? { note } : {})}
              {...(item === newestPerson && !failed && !delivering && !lastUnechoed ? { receipt: "read" as const } : {})}
              {...(failed && newest
                ? {
                    failure: (
                      <FailedReceipt
                        {...(item.reason === "interrupted" ? { label: "Not finished" } : { detail: (note ?? "").replace(/^(Failed|Not sent)(: )?/, "") })}
                        {...(idle && !item.attachments?.length && !errorFollows ? { retry: { sessionId, text: item.text } } : {})}
                      />
                    ),
                  }
                : {})}
            />
          </li>,
        );
        if (cardRow && browserCard?.anchorItemId === item.id) rows.push(cardRow);
        break;
      }
      case "assistant": {
        // Deltas may precede a tool call, so only an explicit final is conversational text.
        if (item.finalSeq === undefined) break;
        const frames = item.finalSeq !== undefined ? (visuals[item.finalSeq] ?? []) : [];
        const text = frames.length ? item.text.replace(VISUAL_BLOCK, "").trim() : item.text;
        if (!turn.reply) stamp(item.id, item.at);
        const current = (turn.reply ??= { key: item.id, parts: [], final: item.final, frames: [] });
        if (item.documents) current.parts.push(<MessageDocuments key={`${item.id}-documents`} sessionId={sessionId} documents={item.documents} />);
        if (text.trim()) current.parts.push(<Markdown key={item.id} text={text} />);
        current.final = item.final;
        current.frames.push(...frames);
        const units = deferred.get(item.id);
        if (units) {
          closeReply();
          rows.push(...units);
        }
        break;
      }
      case "card": {
        const entry = view.cards[item.id];
        if (!entry) break;
        if (entry.card.interaction === "ask" && entry.card.status === "pending") waiting.push({ id: item.id, firstSeq: entry.firstSeq });
        else rows.push(conversationRow(item.id));
        break;
      }
      case "error": {
        const retry = retrySource(view.items, view.items.indexOf(item));
        rows.push(
          <li key={item.id} className="runtime-error-row">
            <RuntimeErrorRow error={item} sessionId={sessionId} {...(retry ? { retry } : {})} />
          </li>,
        );
        break;
      }
    }
  }
  // A trailing group of calls is the live status line itself; with nothing trailing, the typing
  // dots stand in for it.
  const trailingLive = live && tools.length > 0;
  flush(trailingLive);
  closeReply();
  for (const entry of waiting.sort((a, b) => a.firstSeq - b.firstSeq)) rows.push(conversationRow(entry.id));
  if (live && !trailingLive) {
    rows.push(
      <li key="status" className={front ? "live-status typing-row typing-bubble" : "live-status typing-row"}>
        <TypingIndicator />
      </li>,
    );
  }

  if (delivering) {
    rows.push(
      <li key={`d-${delivering.id}`} className="bubble user" data-message={delivering.id}>
        <UserBubble
          sessionId={sessionId}
          text={delivering.text}
          {...(delivering.attachments ? { attachments: delivering.attachments } : {})}
          {...(lastUnechoed ? {} : { receipt: "sent" as const })}
        />
      </li>,
    );
  }
  for (const message of unechoed) {
    rows.push(<OutgoingBubble key={`o-${message.message_id}`} sessionId={sessionId} message={message} latest={message === lastUnechoed} />);
  }

  return (
    <ol className="timeline" aria-live="polite">
      {rows}
    </ol>
  );
}
