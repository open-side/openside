import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import type { Card } from "../api/types";
import { Dialog } from "../ui";
import { ConversationCard } from "./ConversationCard";
import "./card-surfaces.css";

const FIELD = "input:not([type=hidden]):not([disabled]), textarea:not([disabled]), select:not([disabled])";

/**
 * Sheet opened by tapping a code, login or choice push notification (`?ticket=` on the
 * conversation URL): a ui Dialog (phone bottom sheet, desktop modal), so focus stays inside and
 * the app behind is inert. The phone's keyboard offers the texted code or saved password to the
 * card's inputs; once the request is answered, `onClose` drops the parameter and the
 * conversation is back. The card is the same one the timeline shows (which goes inert meanwhile).
 */
export function CardSheet({ card, sessionId, onClose }: { card: Card; sessionId: string; onClose: () => void }) {
  const body = useRef<HTMLDivElement>(null);
  // Read during the first render, before the dialog moves focus in. The sheet unmounts rather
  // than animating closed, which skips Base UI's own focus return, so focus goes back here.
  const [returnTo] = useState(() => document.activeElement as HTMLElement | null);
  useEffect(
    () => () => {
      const current = document.activeElement;
      if (returnTo && returnTo !== document.body && returnTo.isConnected && (!current || current === document.body)) returnTo.focus({ preventScroll: true });
    },
    [returnTo],
  );
  // The card renders its fields outside React, so the dialog asks for them when it opens: the
  // person came to type the code, not to land on Close.
  const initialFocus = useMemo<RefObject<HTMLElement | null>>(
    () => ({
      get current() {
        return body.current?.querySelector<HTMLElement>(FIELD) ?? body.current?.querySelector<HTMLElement>("button") ?? null;
      },
    }),
    [],
  );
  return (
    <Dialog open onClose={onClose} title={card.title} className="card-sheet" initialFocus={initialFocus}>
      <div ref={body}>
        <ConversationCard card={card} sessionId={sessionId} layout="sheet" focus="force" onSettled={onClose} />
      </div>
    </Dialog>
  );
}
