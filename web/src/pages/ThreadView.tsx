import { ArrowDown } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
import { ApiError } from "../api/client";
import { AppHeader } from "../components/AppHeader";
import { FilesTray } from "../components/FilesTray";
import { AgentTabsPanel } from "../components/AgentTabsPanel";
import { useAgentTabsPanel } from "../components/agentTabsStore";
import { BrowserPreview, browserAnchor } from "../components/BrowserPreview";
import { ComputerUseCard } from "../components/ComputerUseCard";
import { Composer } from "../components/Composer";
import { quoteText } from "../components/MessageMenu";
import { CardSheet } from "../components/CardSheet";
import { CardHostContext, type CardHost } from "../components/ConversationCard";
import { QueueList } from "../components/QueueList";
import { StopTaskDialog } from "../components/StopTaskDialog";
import { AgentsTray } from "../components/SubagentPanel";
import { Timeline } from "../components/Timeline";
import { useClient } from "../context";
import { useSessionFiles } from "../hooks/useSessionFiles";
import { useSessionView } from "../hooks/useSessionView";
import { t } from "../i18n";
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
  /**
   * A side chat in the split pane: no Agent Tabs column, no `?ticket=` handling and no
   * document-wide layout variables, which belong to the chat the page is about.
   */
  side?: boolean;
}

/** What a failed branch says: the reply still being written has its own words; others keep the daemon's. */
function forkFailure(cause: unknown): string {
  if (!(cause instanceof ApiError)) return t("timeline.fork.failed");
  return cause.code === "message_not_finished" ? t("timeline.fork.notFinished") : cause.message;
}

/** One continuous conversation: transcript, live browser card, queue and composer. */
export function ThreadView({ sessionId: id, header, side = false }: ThreadViewProps) {
  const client = useClient();
  const shell = useShell();
  const toast = useToast();
  const { view, connection, error } = useSessionView(client, id);
  const files = useSessionFiles(client, id, view);
  const sending = useOutgoing(id);
  const [starter, setStarter] = useState<ShareDraft | null>(null);
  const [quote, setQuote] = useState<{ id: string; text: string } | null>(null);
  const onQuote = (text: string) => setQuote({ id: `quote-${Date.now()}`, text: quoteText(text) });
  // Once the event log has the message, the log (not this device's copy) shows its delivery state.
  useEffect(() => {
    outgoing.prune(id, new Set([...view.queue.map((message) => message.id), ...view.items.flatMap((item) => (item.type === "user" ? [item.id] : []))]));
  }, [id, view.queue, view.items]);
  const pending = pendingTickets(view);
  const waiting = pending.length > 0;
  const running = view.status === "running";
  const agentTabs = useAgentTabsPanel(id);

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
  const linked = side ? null : params.get("ticket");
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
      openConnection: () => navigate("/settings/security"),
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

  // The composer's height places the jump-to-latest control above it. Set on the document too:
  // the browser preview is portaled to the body and keeps clear of the composer.
  useEffect(() => {
    const element = dock.current?.firstElementChild;
    const root = main.current;
    if (!element || !root || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      const height = `${Math.round(element.getBoundingClientRect().height)}px`;
      root.style.setProperty("--composer-height", height);
      if (!side) document.documentElement.style.setProperty("--composer-height", height);
      pin();
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
      if (!side) document.documentElement.style.removeProperty("--composer-height");
    };
  }, [error, pin, side]);

  // A dropped or restored stream re-checks the daemon at once, so the shell's connection
  // banner follows it instead of waiting for the next list sync.
  const { refreshSessions } = shell;
  useEffect(() => {
    if (connection === "reconnecting" || connection === "open") refreshSessions();
  }, [connection, refreshSessions]);

  const onFork = useCallback(
    async (messageId: string) => {
      try {
        const branch = await client.forkSession(id, { from_message: messageId });
        refreshSessions();
        navigate(`/sessions/${encodeURIComponent(branch.id)}`);
      } catch (cause) {
        toast.notify(forkFailure(cause), "error");
      }
    },
    [client, id, refreshSessions, navigate, toast],
  );

  if (error) {
    return (
      <main className="conversation session-error">
        <AppHeader title={t("timeline.thread.conversation")} />
        <div className="conversation-scroll thin-scroll">
          <div className="empty-chat">
            <p role="alert" className="error">
              {error.message}
            </p>
            <Link to="/chats" className="action-link">
              {t("timeline.thread.openChats")}
            </Link>
          </div>
        </div>
      </main>
    );
  }

  const blank = view.lastSeq > 0 && view.items.length === 0 && view.queue.length === 0 && sending.length === 0;
  // Until the log's first events arrive the thread is only its header, as in Aside: no composer
  // to type into a chat that is not there yet.
  const loading = view.lastSeq === 0 && connection === "connecting" && sending.length === 0;

  return (
    <main
      className="session conversation"
      data-status={view.status}
      data-last-seq={view.lastSeq}
      data-loading={loading || undefined}
      data-side={side || undefined}
      data-agent-tabs={!side && agentTabs.shown && !view.incognito ? "open" : undefined}
      ref={main}
    >
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
          <CardHostContext.Provider value={cardHost}>
            <Timeline
              view={view}
              sessionId={id}
              visuals={files.visuals}
              outgoing={sending}
              inertCardId={sheetCard?.id ?? null}
              onShowPending={showPending}
              onQuote={onQuote}
              // Branching would replace the page's own chat; Aside's compact side chat offers no branch either.
              onFork={side ? undefined : onFork}
              // A side chat stays a compact transcript; the page's own chat carries the live browser.
              {...(side
                ? {}
                : {
                    browserCard: {
                      anchorItemId: browserAnchor(view.items),
                      node: <BrowserPreview key={id} sessionId={id} view={view} onShowPending={showPending} onCancel={() => setConfirmStop(true)} />,
                    },
                  })}
            />
          </CardHostContext.Provider>
          {side ? null : <ComputerUseCard key={`computer-${id}`} sessionId={id} view={view} />}
          <QueueList sessionId={id} queue={view.queue.filter((message) => message.id !== view.delivering)} running={running} onQuote={onQuote} />
        </div>
      </div>
      {!atBottom ? (
        <IconButton label={t("timeline.thread.scrollLatest")} outlined className="jump-latest" onClick={() => scrollToEnd(true)}>
          <ArrowDown size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      ) : null}
      <div ref={dock} className="composer-anchor">
        {loading ? null : (
          <Composer
            sessionId={id}
            fresh={blank}
            running={running}
            waiting={waiting}
            reconnecting={connection === "reconnecting"}
            onStop={() => setConfirmStop(true)}
            onShowPending={waiting ? showPending : undefined}
            onSent={() => scrollToEnd()}
            draft={starter}
            quote={quote}
            onQuoteApplied={() => setQuote(null)}
            onDraftApplied={() => setStarter(null)}
            tools={
              <>
                <AgentsTray subagents={view.subagents} />
                <FilesTray sessionId={id} artifacts={files.artifacts} />
              </>
            }
          />
        )}
      </div>
      {side ? null : <AgentTabsPanel sessionId={id} view={view} container={main} />}
      {sheetCard ? <CardSheet card={sheetCard} sessionId={id} onClose={closeSheet} /> : null}
      <StopTaskDialog
        open={confirmStop && running}
        scope={{ kind: "all", tasks: view.subagents.filter((task) => task.status === "running" || task.status === "waiting_human").length }}
        onClose={() => setConfirmStop(false)}
        onConfirm={() => {
          setConfirmStop(false);
          void client.stopSession(id).catch(() => toast.notify(t("timeline.thread.stopFailed"), "error"));
        }}
      />
    </main>
  );
}
