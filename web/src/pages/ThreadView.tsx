import { ArrowDown } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
import { AppHeader } from "../components/AppHeader";
import { FilesTray } from "../components/FilesTray";
import { BrowserPreview, browserAnchor } from "../components/BrowserPreview";
import { Composer } from "../components/Composer";
import { CardSheet } from "../components/CardSheet";
import { CardHostContext, type CardHost } from "../components/ConversationCard";
import { QueueList } from "../components/QueueList";
import { StopTaskDialog } from "../components/StopTaskDialog";
import { AgentsTray } from "../components/SubagentPanel";
import { Timeline } from "../components/Timeline";
import { useClient } from "../context";
import { useSessionFiles } from "../hooks/useSessionFiles";
import { useSessionView } from "../hooks/useSessionView";
import { useShell } from "../shell";
import { isAttentionTicket } from "../cards/attention";
import type { ShareDraft } from "../pwa/share";
import { useOutgoing, outgoing } from "../state/outgoing";
import { pendingTickets, type SessionView } from "../state/session";
import { ICON_STROKE, IconButton, useToast } from "../ui";
import { distanceFromEnd, FOLLOW_SLACK, nextFollowing, type ScrollSample } from "./scrollFollow";

const sampleOf = (element: HTMLElement): ScrollSample => ({ top: element.scrollTop, height: element.scrollHeight, client: element.clientHeight });

export interface ThreadState {
  view: SessionView;
  running: boolean;
  waiting: boolean;
}

export interface ThreadViewProps {
  sessionId: string;
  /** The screen's header, built from the live thread state. */
  header: (state: ThreadState) => ReactNode;
  /** Who reads what the person types ("Message Juno"). */
  addressee?: string;
  /** Replaces the composer, e.g. for a retired teammate's thread. */
  readOnly?: ReactNode;
  /** Shown above an empty thread, before anything was said. */
  empty?: (pick: (text: string) => void) => ReactNode;
  /** Content shared to the app; it becomes the composer's starting message. */
  draft?: ShareDraft | null;
  onDraftApplied?: () => void;
  /** Every new event sequence this view rendered, for the device's read cursor. */
  onSeen?: (lastSeq: number) => void;
}

/** One continuous conversation: transcript, live browser card, queue and composer. */
export function ThreadView({ sessionId: id, header, addressee, readOnly, empty, draft, onDraftApplied, onSeen }: ThreadViewProps) {
  const client = useClient();
  const shell = useShell();
  const toast = useToast();
  const { view, connection, error } = useSessionView(client, id);
  const files = useSessionFiles(client, id, view);
  const sending = useOutgoing(id);
  const [starter, setStarter] = useState<ShareDraft | null>(null);
  // Once the event log has the message, the log (not this device's copy) shows its delivery state.
  useEffect(() => {
    outgoing.prune(id, new Set([...view.queue.map((message) => message.id), ...view.items.flatMap((item) => (item.type === "user" ? [item.id] : []))]));
  }, [id, view.queue, view.items]);
  useEffect(() => {
    if (view.lastSeq > 0) onSeen?.(view.lastSeq);
  }, [view.lastSeq, onSeen]);
  const pending = pendingTickets(view);
  const waiting = pending.length > 0;
  const running = view.status === "running";

  const main = useRef<HTMLElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const dock = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const sample = useRef<ScrollSample>({ top: 0, height: 0, client: 0 });
  const [atBottom, setAtBottom] = useState(true);
  const [confirmStop, setConfirmStop] = useState(false);

  // `?ticket=` comes from a tapped push notification. A pending code, login or choice request
  // opens its sheet; any other ticket is scrolled into view. The parameter is dropped once
  // handled, so the conversation is what Back and a reload show afterwards.
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const linked = params.get("ticket");
  const linkedTicket = linked ? view.tickets[linked] : undefined;
  const sheetCard = linkedTicket?.status === "pending" && isAttentionTicket(linkedTicket) ? (view.cards[linkedTicket.card?.id ?? ""]?.card ?? null) : null;
  const sheetTicket = sheetCard ? linkedTicket : null;
  const closeSheet = useCallback(
    () =>
      setParams(
        (current) => {
          current.delete("ticket");
          return current;
        },
        { replace: true },
      ),
    [setParams],
  );
  useEffect(() => {
    if (!linkedTicket || sheetTicket) return;
    following.current = false;
    main.current?.querySelector(`[data-ticket="${CSS.escape(linkedTicket.id)}"]`)?.scrollIntoView({ block: "center" });
    closeSheet();
  }, [linkedTicket, sheetTicket, closeSheet]);

  // What a card's follow_up / open_ticket / open_connection actions do on this screen.
  const cardHost = useMemo<CardHost>(
    () => ({
      followUp: (text) => setStarter({ id: `follow-up-${Date.now()}`, text, files: [] }),
      openTicket: (ticketId) =>
        setParams(
          (current) => {
            current.set("ticket", ticketId);
            return current;
          },
          { replace: true },
        ),
      openConnection: () => navigate("/connections"),
    }),
    [setParams, navigate],
  );

  const scrollToEnd = useCallback((smooth = false) => {
    const element = scroller.current;
    if (!element) return;
    following.current = true;
    setAtBottom(true);
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    element.scrollTo({ top: element.scrollHeight, behavior: smooth && !reduced ? "smooth" : "auto" });
  }, []);

  const showPending = () => {
    following.current = false;
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    main.current?.querySelector('.oc-card[data-ask="pending"]')?.scrollIntoView({ block: "center", behavior: reduced ? "auto" : "smooth" });
  };

  // Stay pinned to the latest turn while it streams, unless the reader scrolled up. Growth
  // moves the end away without a scroll event, so the jump control is re-evaluated here too.
  const pin = useCallback(() => {
    const element = scroller.current;
    if (!element) return;
    if (following.current) {
      element.scrollTop = element.scrollHeight;
      sample.current = sampleOf(element);
    }
    setAtBottom(following.current || distanceFromEnd(sampleOf(element)) < FOLLOW_SLACK);
  }, []);
  // A different thread always opens on its latest turn, whatever the previous one was doing.
  useLayoutEffect(() => {
    following.current = true;
    setAtBottom(true);
  }, [id]);
  useLayoutEffect(pin, [pin, view.lastSeq, view.queue.length, sending.length]);
  // Content that grows without a new event (images, highlighted code, a card opening) must
  // not push the latest turn out of view while following.
  useEffect(() => {
    const column = scroller.current?.firstElementChild;
    if (!column || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(pin);
    observer.observe(column);
    return () => observer.disconnect();
  }, [pin, error]);

  // The transcript keeps its last turn clear of the floating composer, whatever its height.
  // Set on the document too: the browser preview and code dock are portaled to the body.
  useEffect(() => {
    // The anchor is `display: contents`; its only child is the absolutely placed composer dock.
    const element = dock.current?.firstElementChild;
    const root = main.current;
    if (!element || !root || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      const height = `${Math.round(element.getBoundingClientRect().height)}px`;
      root.style.setProperty("--composer-height", height);
      document.documentElement.style.setProperty("--composer-height", height);
      pin();
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
      document.documentElement.style.removeProperty("--composer-height");
    };
  }, [error, pin]);

  // A dropped or restored stream re-checks the daemon at once, so the shell's connection
  // banner follows it instead of waiting for the next list sync.
  const { refreshSessions } = shell;
  useEffect(() => {
    if (connection === "reconnecting" || connection === "open") refreshSessions();
  }, [connection, refreshSessions]);

  if (error) {
    return (
      <main className="conversation session-error">
        <AppHeader title="Conversation" />
        <div className="conversation-scroll thin-scroll">
          <div className="empty-chat">
            <p role="alert" className="error">
              {error.message}
            </p>
            <Link to="/history" className="action-link">
              Open History
            </Link>
          </div>
        </div>
      </main>
    );
  }

  const blank = view.lastSeq > 0 && view.items.length === 0 && view.queue.length === 0 && sending.length === 0;

  return (
    <main className="session conversation" data-status={view.status} data-last-seq={view.lastSeq} ref={main}>
      {header({ view, running, waiting })}
      <div
        className="conversation-scroll thin-scroll"
        ref={scroller}
        onScroll={(event) => {
          const element = event.currentTarget;
          const current = sampleOf(element);
          following.current = nextFollowing(following.current, sample.current, current);
          sample.current = current;
          setAtBottom(following.current || distanceFromEnd(current) < FOLLOW_SLACK);
        }}
      >
        <div className="conversation-column">
          {blank && empty && !readOnly ? empty((text) => setStarter({ id: `starter-${Date.now()}`, text, files: [] })) : null}
          <CardHostContext.Provider value={cardHost}>
            <Timeline
              view={view}
              sessionId={id}
              visuals={files.visuals}
              outgoing={sending}
              inertCardId={sheetCard?.id ?? null}
              browserCard={{
                anchorItemId: browserAnchor(view.items),
                node: <BrowserPreview key={id} sessionId={id} view={view} onShowPending={showPending} onCancel={() => setConfirmStop(true)} />,
              }}
            />
          </CardHostContext.Provider>
          <QueueList sessionId={id} queue={view.queue.filter((message) => message.id !== view.delivering)} running={running} />
        </div>
      </div>
      {!atBottom ? (
        <IconButton label="Scroll to latest" outlined className="jump-latest" onClick={() => scrollToEnd(true)}>
          <ArrowDown size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      ) : null}
      <div ref={dock} className="composer-anchor">
        {readOnly ?? (
          <Composer
            sessionId={id}
            addressee={addressee}
            running={running}
            waiting={waiting}
            reconnecting={connection === "reconnecting"}
            onStop={() => setConfirmStop(true)}
            onShowPending={waiting ? showPending : undefined}
            onSent={() => scrollToEnd()}
            draft={starter ?? draft}
            onDraftApplied={() => {
              setStarter(null);
              onDraftApplied?.();
            }}
            tools={
              <>
                <AgentsTray sessionId={id} subagents={view.subagents} />
                <FilesTray sessionId={id} artifacts={files.artifacts} />
              </>
            }
          />
        )}
      </div>
      {sheetCard ? <CardSheet card={sheetCard} sessionId={id} onClose={closeSheet} /> : null}
      <StopTaskDialog
        open={confirmStop && running}
        scope={{ kind: "all", tasks: view.subagents.filter((task) => task.status === "running" || task.status === "waiting_human").length }}
        onClose={() => setConfirmStop(false)}
        onConfirm={() => void client.stopSession(id).catch(() => toast.notify("Could not stop the agent", "error"))}
      />
    </main>
  );
}
