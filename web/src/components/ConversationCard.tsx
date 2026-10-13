import { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { Action, Card } from "../api/types";
import { mayUse } from "../auth";
import type { ActionPayload, HostActionHandler } from "../cards/actions";
import { mountCard, type CardMountOptions, type MountedCard, type Settled } from "../cards/render";
import { useClient } from "../context";
import { haptic } from "../pwa/haptics";
import { TakeoverView } from "./TakeoverView";
import { useOptionalTeamActions } from "./TeamActions";
import "../cards/cards.css";
import "./team.css";

/** What the surrounding screen can do for card actions that leave the card. */
export interface CardHost {
  /** Puts text in the composer; the person still has to send it. */
  followUp?: (text: string) => void;
  openTicket?: (ticketId: string) => void;
  openConnection?: (connectionId: string) => void;
}

export const CardHostContext = createContext<CardHost>({});

export interface ConversationCardProps {
  card: Card;
  sessionId?: string;
  /** Tried first for every host-owned target; return `false` to decline. */
  onAction?: HostActionHandler;
  onSettled?: (outcome: Settled, card: Card) => void;
  defaultOpen?: boolean;
  focus?: CardMountOptions["focus"];
  layout?: CardMountOptions["layout"];
  inert?: boolean;
  /** The helper task or teammate that raised the request; shown in the header as "<label> asked". */
  asker?: CardMountOptions["asker"];
}

/**
 * The one mount point for a conversation card in the web app. The card itself is plain DOM
 * (cards/render.ts, shared with the extension); this hosts it, wires the client and the
 * screen's actions, and shows the takeover view the card asks for. Remounts only when the card
 * id changes, so typed input survives a re-poll.
 */
export function ConversationCard({ card, sessionId, onAction, onSettled, defaultOpen, focus, layout, inert, asker }: ConversationCardProps) {
  const client = useClient();
  const team = useOptionalTeamActions();
  const host = useContext(CardHostContext);
  const [takeover, setTakeover] = useState(false);
  const element = useRef<HTMLDivElement>(null);
  const view = useRef<MountedCard | null>(null);

  const latest = useRef({ card, onAction, onSettled, team, host });
  latest.current = { card, onAction, onSettled, team, host };

  // Keyed on what decides the layout of the mounted card, never on callbacks.
  const supported = useMemo(
    () =>
      (action: Action): boolean => {
        switch (action.target.type) {
          case "follow_up":
            return !!host.followUp || !!onAction;
          case "open_ticket":
            return !!host.openTicket || !!onAction;
          case "open_connection":
            return !!host.openConnection || !!onAction;
          case "open_thread":
          case "team_command":
            return !!team || !!onAction;
          default:
            return true;
        }
      },
    [host, team, onAction],
  );
  const supports = useRef(supported);
  supports.current = supported;

  useEffect(() => {
    const handle = async (action: Action, current: Card, payload: ActionPayload) => {
      const { onAction: own, team: teamActions, host: screen } = latest.current;
      if (own) {
        const handled = await own(action, current, payload);
        if (handled !== false) return true;
      }
      const target = action.target;
      if (target.type === "takeover") {
        setTakeover(true);
        return true;
      }
      if (teamActions) {
        const handled = await teamActions.onCardAction(action, current, payload);
        if (handled !== false) return true;
      }
      if (target.type === "follow_up" && screen.followUp) screen.followUp(target.text);
      else if (target.type === "open_ticket" && screen.openTicket) screen.openTicket(target.ticket_id);
      else if (target.type === "open_connection" && screen.openConnection) screen.openConnection(target.connection_id);
      else return false;
      return true;
    };
    const mounted = mountCard(element.current!, latest.current.card, {
      port: client,
      onAction: handle,
      supports: (action) => supports.current(action),
      onSettled: (outcome, settled) => {
        haptic(outcome === "rejected" ? "reject" : "approve");
        latest.current.onSettled?.(outcome, settled);
      },
      ...(sessionId ? { sessionId } : {}),
      ...(mayUse("approve") === false ? { canApprove: false } : {}),
      ...(defaultOpen !== undefined ? { defaultOpen } : {}),
      ...(focus ? { focus } : {}),
      ...(layout ? { layout } : {}),
      ...(inert ? { inert } : {}),
      ...(asker ? { asker } : {}),
    });
    view.current = mounted;
    return () => {
      mounted.dispose();
      view.current = null;
    };
    // The card id identifies the card; defaults and layout are mount-time choices.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, card.id, sessionId, layout]);

  // The remote screen belongs to a pending step: once the daemon settles the card (Done inside the
  // dialog, a cancel, an expiry) the dialog goes away with it.
  useEffect(() => {
    if (card.status !== "pending") setTakeover(false);
  }, [card.status]);

  useEffect(() => {
    view.current?.update(card);
  }, [card]);

  useEffect(() => {
    view.current?.setInert(inert === true);
  }, [inert]);

  return (
    <>
      <div ref={element} className="conversation-card" data-card-host={card.id} />
      {takeover && card.ticket_id ? <TakeoverView ticket={{ id: card.ticket_id, summary: card.title, origin: card.site.origin ?? null }} onClose={() => setTakeover(false)} /> : null}
    </>
  );
}
