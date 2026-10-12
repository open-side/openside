import { useEffect } from "react";
import type { Client } from "../api/client";
import type { Ticket } from "../api/types";
import { haptic } from "./haptics";

/** Matches the session list's cadence: a ticket raised on another device shows up within seconds. */
const POLL_MS = 15_000;

/** The installed app's icon badge (Android, desktop PWAs, iOS 16.4+ Home Screen apps). */
export function setAppBadge(count: number): void {
  // lib.dom types these as always present; Firefox and desktop Safari do not have them.
  const nav: Partial<Pick<Navigator, "setAppBadge" | "clearAppBadge">> = navigator;
  const done = count > 0 ? nav.setAppBadge?.(count) : nav.clearAppBadge?.();
  done?.catch(() => undefined);
}

export function pendingTicketIds(tickets: readonly Pick<Ticket, "id" | "status">[]): string[] {
  return tickets.filter((ticket) => ticket.status === "pending").map((ticket) => ticket.id);
}

/**
 * A pushed "needs you" notification outlives the ticket when it is answered on another device;
 * once the daemon no longer lists the ticket as pending, take its notification down.
 */
async function dismissSettledNotifications(pending: ReadonlySet<string>): Promise<void> {
  const registration = await navigator.serviceWorker?.getRegistration("/");
  for (const notification of (await registration?.getNotifications()) ?? []) {
    const data: { kind?: string; ticket_id?: string } | null = notification.data;
    if (data?.kind === "ticket" && data.ticket_id && !pending.has(data.ticket_id)) notification.close();
  }
}

/**
 * Keeps the app icon badge equal to the number of pending tickets while the app is in the
 * foreground (the service worker keeps it roughly right from pushes while it is closed), and
 * nudges with a short haptic when a new one arrives.
 */
export function usePendingTicketBadge(client: Client): void {
  useEffect(() => {
    let cancelled = false;
    let seen: ReadonlySet<string> | null = null;
    const sync = async () => {
      if (document.visibilityState !== "visible") return;
      try {
        const { tickets } = await client.listTickets("pending");
        if (cancelled) return;
        const ids = new Set(pendingTicketIds(tickets));
        setAppBadge(ids.size);
        // The first response only establishes what was already waiting.
        if (seen && [...ids].some((id) => !seen?.has(id))) haptic("attention");
        seen = ids;
        await dismissSettledNotifications(ids);
      } catch {
        // Unreachable or not permitted: leave the badge as it was.
      }
    };
    void sync();
    const timer = window.setInterval(() => void sync(), POLL_MS);
    document.addEventListener("visibilitychange", sync);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      // The signed-in shell is gone (sign-out or a rejected token): the badge is not ours to keep.
      setAppBadge(0);
      document.removeEventListener("visibilitychange", sync);
    };
  }, [client]);
}
