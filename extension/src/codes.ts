import { announcementMessage, announcementTitle, isAttentionTicket } from "../../web/src/cards/attention";
import type { Ticket } from "./api";
export const CODE_NOTIFICATION = "code:";
// Chrome opens a side panel only inside a user gesture, so a code, login or choice request
// cannot open it by itself: the toolbar badge counts waiting requests and each new one is
// announced once with a notification whose click opens the panel. Nothing takes focus on its own.
// With desktop notifications on (`all`), every pending request is announced, not just the ones that
// need a human step, except those of the conversation the person is already looking at.
export interface AnnounceOptions { all?: boolean; focused?: (sessionId: string) => boolean }
export async function announceCodes(tickets: Ticket[], { all = false, focused }: AnnounceOptions = {}): Promise<void> {
  const waiting = tickets.filter(ticket => all || isAttentionTicket(ticket));
  await chrome.action.setBadgeBackgroundColor({ color: "#2f6fed" });
  await chrome.action.setBadgeText({ text: waiting.length ? String(waiting.length) : "" });
  const stored: unknown = (await chrome.storage.session.get("codesNotified")).codesNotified;
  const notified = new Set(Array.isArray(stored) ? stored : []);
  const ids = new Set(waiting.map(ticket => ticket.id));
  for (const id of notified) if (!ids.has(id)) void chrome.notifications.clear(`${CODE_NOTIFICATION}${id}`);
  for (const ticket of waiting) {
    if (notified.has(ticket.id)) continue;
    // Marked as announced below either way, so leaving the conversation later does not replay it.
    if (focused?.(ticket.session_id)) continue;
    chrome.notifications.create(`${CODE_NOTIFICATION}${ticket.id}`, {
      type: "basic", iconUrl: "icon.png",
      title: announcementTitle(ticket),
      message: announcementMessage(ticket),
      contextMessage: "Click to answer in the openside side panel",
    });
  }
  await chrome.storage.session.set({ codesNotified: [...ids] });
}
