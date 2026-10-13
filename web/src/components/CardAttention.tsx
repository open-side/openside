import { ArrowUpRight } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Link } from "react-router";
import type { Session, Ticket } from "../api/types";
import { announcementTitle, askerOf, byAttention, isAttentionTicket } from "../cards/attention";
import { siteOf } from "../cards/values";
import { useClient } from "../context";
import { ICON_STROKE } from "../ui";
import { ConversationCard } from "./ConversationCard";
import "./card-surfaces.css";

const POLL_MS = 5000;

/**
 * A code, login, choice or phone approval from a conversation that is not on screen (another
 * conversation, a new chat, Settings over the conversation) floats here as the same collapsed
 * card the conversation shows, so the person answers where they are instead of being taken
 * anywhere. `sessions` (the sidebar's list) names the conversation that asked.
 */
export function CardAttention({ visibleSession, sessions }: { visibleSession: string | null; sessions: Session[] | null }) {
  const client = useClient();
  const [tickets, setTickets] = useState<Ticket[]>([]);
  const [done, setDone] = useState<ReadonlySet<string>>(new Set());
  const dock = useRef<HTMLElement>(null);

  const load = useCallback(async () => {
    try {
      const list = await client.listTickets("pending");
      setTickets(list.tickets.filter(isAttentionTicket).sort(byAttention));
    } catch {
      // A token without the read scope or a restarting daemon: keep the last answer.
    }
  }, [client]);

  useEffect(() => {
    const sync = () => {
      if (document.visibilityState === "visible") void load();
    };
    sync();
    const timer = window.setInterval(sync, POLL_MS);
    document.addEventListener("visibilitychange", sync);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", sync);
    };
  }, [load]);

  const shown = tickets.filter((ticket) => ticket.session_id !== visibleSession && !done.has(ticket.id));
  const first = shown[0];

  // The dock's height (a collapsed bubble, or the open card) is kept free at the top of the
  // conversation (styles.css reads `--otp-dock-space`), so the dock never covers the timeline.
  // `--otp-dock-height` is the same height for sheets and the remote screen.
  useLayoutEffect(() => {
    const element = dock.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const root = document.documentElement;
    const measure = () => {
      const height = Math.round(element.getBoundingClientRect().height);
      root.style.setProperty("--otp-dock-height", `${height}px`);
      root.style.setProperty("--otp-dock-space", `${height + 12}px`);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
      root.style.removeProperty("--otp-dock-height");
      root.style.removeProperty("--otp-dock-space");
    };
  }, [first?.id]);

  if (!first?.card) return null;
  const title = sessions?.find((session) => session.id === first.session_id)?.title;
  // Portaled beside the dialogs rather than inside the app shell, which a modal Settings dialog
  // hides from assistive tech and pointer input.
  return createPortal(
    <aside ref={dock} className="card-dock" aria-label={`${announcementTitle(first)} for ${siteOf(first.card.site.origin)}`}>
      <ConversationCard
        key={first.id}
        card={first.card}
        asker={askerOf(first)}
        sessionId={first.session_id}
        onSettled={() => {
          setDone((previous) => new Set(previous).add(first.id));
          void load();
        }}
      />
      <p className="card-dock-meta">
        <Link to={`/sessions/${encodeURIComponent(first.session_id)}`} aria-label={`Go to ${title ?? "the conversation"}`}>
          <span>{title ?? "Another conversation"}</span>
          <ArrowUpRight size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </Link>
        {shown.length > 1 ? <span className="card-dock-more">{shown.length - 1} more waiting</span> : null}
      </p>
    </aside>,
    document.body,
  );
}
