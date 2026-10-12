import { ArrowLeft } from "lucide-react";
import { useEffect } from "react";
import type { Ticket } from "../api/types";
import { ICON_STROKE, IconButton } from "../ui";
import { TicketCard } from "./TicketCard";
import "./otp-surfaces.css";

/**
 * Full-screen code sheet opened by tapping a code push notification (`?ticket=` on the
 * conversation URL). The phone's keyboard offers the texted code to the one input; after
 * submit, `onClose` drops the parameter and the conversation is back.
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
    <div className="otp-sheet" role="dialog" aria-modal="true" aria-label="Enter the code">
      <div className="otp-sheet-bar">
        <IconButton label="Back to conversation" onClick={onClose}>
          <ArrowLeft size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      </div>
      <div className="otp-sheet-body">
        <TicketCard ticket={ticket} focus="force" onResolved={onClose} />
      </div>
    </div>
  );
}
