import { ArrowLeft } from "lucide-react";
import { useEffect } from "react";
import type { Ticket } from "../api/types";
import { isApprovalTicket, isChoiceTicket, isLoginTicket } from "../otp/loginForm";
import { isOtpTicket } from "../otp/otpForm";
import { ICON_STROKE, IconButton } from "../ui";
import { AuthForm } from "./AuthForm";
import { TicketCard } from "./TicketCard";
import "./otp-surfaces.css";

/**
 * Full-screen sheet opened by tapping a code, login or choice push notification (`?ticket=` on
 * the conversation URL). The phone's keyboard offers the texted code or saved password to the
 * card's inputs; after submit, `onClose` drops the parameter and the conversation is back. An
 * authenticator approval opened the same way shows its approval card.
 */
export function OtpSheet({ ticket, onClose }: { ticket: Ticket; onClose: () => void }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="otp-sheet" role="dialog" aria-modal="true" aria-label={isLoginTicket(ticket) ? "Log in" : isChoiceTicket(ticket) ? "Choose an option" : isApprovalTicket(ticket) ? "Approve on your phone" : "Enter the code"}>
      <div className="otp-sheet-bar">
        <IconButton label="Back to conversation" onClick={onClose}>
          <ArrowLeft size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      </div>
      <div className="otp-sheet-body">
        {isOtpTicket(ticket) || isLoginTicket(ticket) || isChoiceTicket(ticket) || isApprovalTicket(ticket) ? (
          <AuthForm ticket={ticket} layout="sheet" focus="force" onSubmitted={onClose} />
        ) : (
          <TicketCard ticket={ticket} onResolved={onClose} />
        )}
      </div>
    </div>
  );
}
