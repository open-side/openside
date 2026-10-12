import { ArrowUpRight } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Link } from "react-router";
import type { Session, Ticket } from "../api/types";
import { useClient } from "../context";
import { isApprovalTicket, isChoiceTicket, isLoginTicket, ticketSite } from "../otp/loginForm";
import { isOtpTicket, otpRegistrationOf } from "../otp/otpForm";
import { TicketCard } from "./TicketCard";
import { Button, ICON_STROKE } from "../ui";
import "./otp-surfaces.css";

const POLL_MS = 5000;

/** Codes expire within minutes, then a login or choice holds a page open; approvals last. */
function urgency(ticket: Ticket): number {
  if (isOtpTicket(ticket)) return 0;
  if (isLoginTicket(ticket) || isChoiceTicket(ticket) || isApprovalTicket(ticket)) return 1;
  return 2;
}

function byUrgency(a: Ticket, b: Ticket): number {
  return urgency(a) - urgency(b) || a.created_at.localeCompare(b.created_at);
}

/**
 * A code, login or choice request from a conversation that is not on screen (another
 * conversation, a new chat, Settings over the conversation) floats here, so the person answers
 * where they are instead of being taken anywhere. `sessions` (the sidebar's list) names the
 * conversation that asked.
 */
export function OtpDock({ visibleSession, sessions }: { visibleSession: string | null; sessions: Session[] | null }) {
  const client = useClient();
  const [tickets, setTickets] = useState<Ticket[]>([]);
  const [done, setDone] = useState<ReadonlySet<string>>(new Set());
  const dock = useRef<HTMLElement>(null);

  const load = useCallback(async () => {
    try {
      const list = await client.listTickets("pending");
      setTickets(
        list.tickets
          .filter((ticket) => isOtpTicket(ticket) || otpRegistrationOf(ticket) || isLoginTicket(ticket) || isChoiceTicket(ticket) || isApprovalTicket(ticket))
          .sort(byUrgency),
      );
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
  // Phones fold the dock to its header row (otp-surfaces.css); opening it is per ticket.
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const expanded = !!first && expandedId === first.id;

  useEffect(() => {
    const card = dock.current;
    if (expanded) (card?.querySelector<HTMLElement>(".ticket .otp-input, .ticket .auth-account, .ticket .auth-input, .ticket .auth-radio") ?? card?.querySelector<HTMLElement>(".ticket button"))?.focus();
  }, [expanded]);

  // The dock's height (a collapsed row, or the open card) is kept free at the top of the
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

  if (!first) return null;
  const title = sessions?.find((session) => session.id === first.session_id)?.title;
  const [label, open] = isOtpTicket(first)
    ? ["Code needed", "Enter code"]
    : isLoginTicket(first)
      ? ["Login needed", "Log in"]
      : isChoiceTicket(first)
        ? ["Choice needed", "Choose"]
        : isApprovalTicket(first)
          ? ["Approve on your phone", "View"]
          : ["Approval needed", "Review"];
  // Portaled beside the dialogs rather than inside the app shell, which a modal Settings dialog
  // hides from assistive tech and pointer input.
  return createPortal(
    <aside ref={dock} className="otp-dock" aria-label={`${label} for ${ticketSite(first)}`} data-expanded={expanded || undefined}>
      <header className="otp-dock-head">
        <Link className="otp-dock-from" to={`/sessions/${encodeURIComponent(first.session_id)}`} aria-label={`Go to ${title ?? "the conversation"}`}>
          <strong>{label}</strong>
          <span>
            {ticketSite(first)} · {title ?? "Another conversation"}
          </span>
          <ArrowUpRight className="otp-dock-go" size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </Link>
        <Button
          variant={expanded ? "ghost" : "primary"}
          size="compact"
          className="otp-dock-toggle"
          aria-expanded={expanded}
          onClick={() => setExpandedId(expanded ? null : first.id)}
        >
          {expanded ? "Hide" : open}
        </Button>
      </header>
      <TicketCard
        key={first.id}
        ticket={first}
        onResolved={() => {
          setDone((previous) => new Set(previous).add(first.id));
          void load();
        }}
      />
      {shown.length > 1 ? <p className="otp-dock-more">{shown.length - 1} more waiting</p> : null}
    </aside>,
    document.body,
  );
}
