import type { Ticket } from "../api/types";

/** How the app was opened: a manifest shortcut or the share target's redirect (public/sw.js). */
export type LaunchIntent =
  | { kind: "new" }
  | { kind: "needs-you" }
  | { kind: "share"; id: string }
  | { kind: "share-failed" };

const SHARE_ID = /^[A-Za-z0-9-]{8,64}$/;

export function parseLaunch(search: string): LaunchIntent | null {
  const params = new URLSearchParams(search);
  const share = params.get("share");
  if (share === "failed") return { kind: "share-failed" };
  if (share !== null) return SHARE_ID.test(share) ? { kind: "share", id: share } : null;
  switch (params.get("launch")) {
    case "new":
      return { kind: "new" };
    case "needs-you":
      return { kind: "needs-you" };
    default:
      return null;
  }
}

export interface NeedsYouTarget {
  /** The conversation that has waited longest. */
  sessionId: string;
  /** How many conversations are waiting in all. */
  conversations: number;
}

/** Where the "Needs you" shortcut goes: the oldest pending ticket's conversation, or nowhere. */
export function needsYouTarget(tickets: readonly Pick<Ticket, "session_id" | "status" | "created_at">[]): NeedsYouTarget | null {
  const pending = tickets.filter((ticket) => ticket.status === "pending").sort((a, b) => a.created_at.localeCompare(b.created_at));
  const first = pending[0];
  if (!first) return null;
  return { sessionId: first.session_id, conversations: new Set(pending.map((ticket) => ticket.session_id)).size };
}
