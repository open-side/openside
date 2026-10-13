import { ArrowLeft } from "lucide-react";
import { useEffect } from "react";
import type { Card } from "../api/types";
import { ICON_STROKE, IconButton } from "../ui";
import { ConversationCard } from "./ConversationCard";
import "./card-surfaces.css";

/**
 * Full-screen sheet opened by tapping a code, login or choice push notification (`?ticket=` on
 * the conversation URL). The phone's keyboard offers the texted code or saved password to the
 * card's inputs; once the request is answered, `onClose` drops the parameter and the
 * conversation is back. The card is the same one the timeline shows (which goes inert meanwhile).
 */
export function CardSheet({ card, sessionId, onClose }: { card: Card; sessionId: string; onClose: () => void }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="card-sheet" role="dialog" aria-modal="true" aria-label={card.title}>
      <div className="card-sheet-bar">
        <IconButton label="Back to conversation" onClick={onClose}>
          <ArrowLeft size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      </div>
      <div className="card-sheet-body">
        <ConversationCard card={card} sessionId={sessionId} layout="sheet" focus="force" onSettled={onClose} />
      </div>
    </div>
  );
}
