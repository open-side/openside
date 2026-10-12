// Wording of the generic ticket cards (approval, question, secret, hands-on step), shared by the
// web app (components/TicketCard.tsx) and the extension side panel so the two say the same thing.
import type { Ticket } from "../api/types";

export const KIND_LABEL: Record<Ticket["kind"], string> = {
  confirm: "Approval needed",
  info: "Question",
  secret_input: "Secret needed",
  device_approval: "Approve on another device",
  takeover: "Hands-on step",
  login: "Login needed",
  choice: "Choice needed",
};

/** Under a takeover ticket that has no remote screen: the person finishes it in the browser itself. */
export const TAKEOVER_NOTE = "Finish this step in the agent’s browser, then mark it done.";

/** The daemon-verified target of a ticket. */
export function siteNote(origin: string): string {
  return `Site: ${origin}`;
}
