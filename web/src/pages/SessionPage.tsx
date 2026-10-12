import { ArrowDown, Ellipsis, Hand } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router";
import { AppHeader } from "../components/AppHeader";
import { ArtifactsPanel } from "../components/ArtifactsPanel";
import { BrowserPreview } from "../components/BrowserPreview";
import { Composer } from "../components/Composer";
import { OtpSheet } from "../components/OtpSheet";
import { QueueList } from "../components/QueueList";
import { SessionMenu } from "../components/SessionMenu";
import { SubagentPanel } from "../components/SubagentPanel";
import { Timeline } from "../components/Timeline";
import { useClient } from "../context";
import { useSessionFiles } from "../hooks/useSessionFiles";
import { useSessionView } from "../hooks/useSessionView";
import { useShell } from "../shell";
import { isOtpTicket, otpRegistrationOf } from "../otp/otpForm";
import { pendingTickets } from "../state/session";
import { ICON_STROKE, IconButton, useToast } from "../ui";

/** Distance from the bottom (px) that still counts as "following" the live turn. */
const FOLLOW_SLACK = 80;

export function SessionPage() {
  const { id = "" } = useParams();
  const client = useClient();
  const shell = useShell();
  const toast = useToast();
  const { view, connection, error } = useSessionView(client, id);
  const files = useSessionFiles(client, id, view);
  const waiting = pendingTickets(view).length > 0;
  const running = view.status === "running";

  const main = useRef<HTMLElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const dock = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const [atBottom, setAtBottom] = useState(true);

  // `?ticket=` comes from a tapped push notification. A pending code request opens the code
  // sheet; any other ticket is scrolled into view. The parameter is dropped once handled, so the
  // conversation is what Back and a reload show afterwards.
  const [params, setParams] = useSearchParams();
  const linked = params.get("ticket");
  const linkedTicket = linked ? view.tickets[linked] : undefined;
  const sheetTicket =
    linkedTicket?.status === "pending" && (isOtpTicket(linkedTicket) || otpRegistrationOf(linkedTicket)) ? linkedTicket : null;
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
    main.current?.querySelector("section.ticket-pending")?.scrollIntoView({ block: "center", behavior: reduced ? "auto" : "smooth" });
  };

  // Stay pinned to the latest turn while it streams, unless the reader scrolled up.
  useLayoutEffect(() => {
    const element = scroller.current;
    if (element && following.current) element.scrollTop = element.scrollHeight;
  }, [view.lastSeq, view.queue.length]);

  // The transcript keeps its last turn clear of the floating composer, whatever its height.
  useEffect(() => {
    // The anchor is `display: contents`; its only child is the absolutely placed composer dock.
    const element = dock.current?.firstElementChild;
    const root = main.current;
    if (!element || !root || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => root.style.setProperty("--composer-height", `${Math.round(element.getBoundingClientRect().height)}px`));
    observer.observe(element);
    return () => observer.disconnect();
  }, [error]);

  if (error) {
    return (
      <main className="conversation session-error">
        <AppHeader title="Conversation" />
        <div className="conversation-scroll">
          <div className="empty-chat">
            <p role="alert" className="error">
              {error.message}
            </p>
            <Link to="/">Start a new chat</Link>
          </div>
        </div>
      </main>
    );
  }

  const subtitle = [
    view.model,
    waiting ? "Needs you" : running ? "Working…" : view.status === "stopped" ? "Stopped" : null,
    connection === "reconnecting" ? "Reconnecting…" : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <main className="session conversation" data-status={view.status} data-last-seq={view.lastSeq} ref={main}>
      <AppHeader
        title={view.title || "…"}
        titleClassName="session-heading"
        subtitle={subtitle}
        actions={
          <SessionMenu
            session={{ id, title: view.title, pinned: view.pinned, archived: view.archived }}
            label="Session options"
            onChanged={shell.refreshSessions}
            trigger={
              <IconButton label="Session options">
                <Ellipsis size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
              </IconButton>
            }
          />
        }
      />
      <div
        className="conversation-scroll"
        ref={scroller}
        onScroll={(event) => {
          const element = event.currentTarget;
          const near = element.scrollHeight - element.scrollTop - element.clientHeight < FOLLOW_SLACK;
          following.current = near;
          setAtBottom(near);
        }}
      >
        <div className="conversation-column">
          <Timeline view={view} sessionId={id} visuals={files.visuals} />
          {view.thinking || (running && view.items.at(-1)?.type !== "assistant") ? (
            <p className="thinking" role="status">
              Working…
            </p>
          ) : null}
          <SubagentPanel sessionId={id} subagents={view.subagents} />
          <ArtifactsPanel sessionId={id} artifacts={files.artifacts} />
          <QueueList sessionId={id} queue={view.queue} running={running} />
        </div>
      </div>
      {!atBottom ? (
        <IconButton label="Scroll to latest" outlined className="jump-latest" onClick={() => scrollToEnd(true)}>
          <ArrowDown size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      ) : null}
      <div ref={dock} className="composer-anchor">
        <Composer
          sessionId={id}
          running={running}
          waiting={waiting}
          onStop={() =>
            void client.stopSession(id).catch(() => toast.notify("Could not stop the response", "error"))
          }
          onSent={() => scrollToEnd()}
          tools={
            <>
              {waiting ? (
                <button type="button" className="tool-pill waiting-pill" onClick={showPending}>
                  <Hand size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  <span className="tool-pill-label">Waiting for you above</span>
                </button>
              ) : null}
              <BrowserPreview key={id} sessionId={id} privateSession={view.incognito} />
            </>
          }
        />
      </div>
      {sheetTicket ? <OtpSheet ticket={sheetTicket} onClose={closeSheet} /> : null}
    </main>
  );
}
