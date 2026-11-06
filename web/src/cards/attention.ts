// Which pending requests are worth interrupting someone for, and how they are announced. Shared
// by the web dock, the extension's notifications and the deep-link handling of a tapped push.
import type { Ticket } from "../api/types";
import { t } from "../i18n";

/**
 * A code, a login, a choice, a phone approval or an authenticator offer: time-limited or holding
 * a page open, so they surface over any screen. Other questions wait in their conversation.
 */
export function isAttentionTicket(ticket: Ticket): boolean {
  if (!ticket.card) return false;
  if (ticket.kind === "login" || ticket.kind === "choice" || ticket.kind === "device_approval") return true;
  if (ticket.kind === "secret_input") return !!ticket.otp;
  return ticket.kind === "confirm" && !!ticket.otp_register;
}

/** Codes expire within minutes, then a login or choice holds a page open; approvals last. */
export function attentionRank(ticket: Ticket): number {
  if (ticket.otp) return 0;
  return isAttentionTicket(ticket) ? 1 : 2;
}

export function byAttention(a: Ticket, b: Ticket): number {
  return attentionRank(a) - attentionRank(b) || a.created_at.localeCompare(b.created_at);
}

/** Title of the desktop notification for a waiting request. */
export function announcementTitle(ticket: Ticket): string {
  if (ticket.otp_register) return t("cards.attention.saveAuthenticator");
  switch (ticket.kind) {
    case "login":
      return t("cards.attention.login");
    case "choice":
      return t("cards.attention.choice");
    case "device_approval":
      return t("cards.attention.deviceApproval");
    case "secret_input":
      return ticket.otp ? t("cards.attention.code") : t("cards.attention.question");
    case "confirm":
      return t("cards.attention.confirm");
    case "takeover":
      return t("cards.attention.takeover");
    default:
      return t("cards.attention.question");
  }
}

/** One line for a notification or a dock header; the card's own safe summary, never field values. */
export function announcementMessage(ticket: Ticket): string {
  return ticket.card?.collapsed_summary ?? ticket.summary;
}

/** Who raised a request when a child agent did: the helper task's title, or a short id when it is unknown here. */
export function askerOf(ticket: Ticket, task?: string | null): { id: string; label: string } | null {
  const id = ticket.origin.subagent_id;
  return id ? { id, label: task?.trim() || t("cards.asker.subagent", { id: id.slice(0, 8) }) } : null;
}
