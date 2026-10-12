import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import type { Ticket } from "../api/types";
import { useClient } from "../context";
import { isOtpTicket, otpRegistrationOf } from "../otp/otpForm";
import { TicketCard } from "./TicketCard";
import "./otp-surfaces.css";

const POLL_MS = 5000;

/**
 * A code request from a conversation that is not on screen (another conversation, a new chat,
 * Settings over the conversation) floats here, so the person types the code where they are
 * instead of being taken anywhere.
 */
export function OtpDock({ visibleSession }: { visibleSession: string | null }) {
  const client = useClient();
  const [tickets, setTickets] = useState<Ticket[]>([]);
  const [done, setDone] = useState<ReadonlySet<string>>(new Set());

  const load = useCallback(async () => {
    try {
      const list = await client.listTickets("pending");
      setTickets(list.tickets.filter((ticket) => isOtpTicket(ticket) || otpRegistrationOf(ticket)));
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
  if (!first) return null;
  // Portaled beside the dialogs rather than inside the app shell, which a modal Settings dialog
  // hides from assistive tech and pointer input.
  return createPortal(
    <aside className="otp-dock" aria-label="Code needed">
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
