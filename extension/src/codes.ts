import { codeHelp, isOtpTicket, otpRegistrationOf, registrationQuestion } from "../../web/src/otp/otpForm";
import type { Ticket } from "./api";
export const CODE_NOTIFICATION = "code:";
// Chrome opens a side panel only inside a user gesture, so a code request cannot open it by
// itself: the toolbar badge counts waiting requests and each new one is announced once with a
// notification whose click opens the panel. Nothing takes focus on its own.
export async function announceCodes(tickets: Ticket[]): Promise<void> {
  const waiting = tickets.filter(ticket => isOtpTicket(ticket) || otpRegistrationOf(ticket));
  await chrome.action.setBadgeBackgroundColor({ color: "#2f6fed" });
  await chrome.action.setBadgeText({ text: waiting.length ? String(waiting.length) : "" });
  const stored: unknown = (await chrome.storage.session.get("codesNotified")).codesNotified;
  const notified = new Set(Array.isArray(stored) ? stored : []);
  const ids = new Set(waiting.map(ticket => ticket.id));
  for (const id of notified) if (!ids.has(id)) void chrome.notifications.clear(`${CODE_NOTIFICATION}${id}`);
  for (const ticket of waiting) {
    if (notified.has(ticket.id)) continue;
    const registration = otpRegistrationOf(ticket);
    chrome.notifications.create(`${CODE_NOTIFICATION}${ticket.id}`, {
      type: "basic", iconUrl: "icon.png",
      title: registration ? "Save authenticator?" : "Code needed",
      message: registration ? registrationQuestion(registration) : isOtpTicket(ticket) ? codeHelp(ticket) : ticket.summary,
      contextMessage: "Click to answer in the openside side panel",
    });
  }
  await chrome.storage.session.set({ codesNotified: [...ids] });
}
