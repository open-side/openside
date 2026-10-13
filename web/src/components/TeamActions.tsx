import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../api/client";
import type { Action, Card, HireDraft } from "../api/types";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { useToast } from "../ui";
import { RetireDialog } from "./AgentProfileSheet";

/** Edited card fields, keyed by field id. */
export interface CardActionPayload {
  values?: Record<string, string | boolean>;
}

/**
 * The hire card lets the person rename the proposed teammate and nothing else; every other
 * field of the daemon's draft (accounts, channel, ownership) is sent back exactly as proposed.
 */
export function mergeHireDraft(draft: HireDraft, values: CardActionPayload["values"]): HireDraft {
  const field = (key: string) => (typeof values?.[key] === "string" ? (values[key] as string).trim() : "");
  const name = field("name");
  const role = field("role");
  const renamed = name !== "" && name !== draft.name;
  const reroled = role !== "" && role !== draft.persona.role;
  if (!renamed && !reroled) return draft;
  return {
    ...draft,
    // Initials follow a new name; the daemon derives them again from null.
    ...(renamed ? { name, initials: null } : {}),
    ...(reroled ? { persona: { ...draft.persona, role } } : {}),
  };
}

interface TeamActions {
  /** Opens the retire flow (successor picker) for a teammate, over whatever screen is showing. */
  retire: (agentId: string) => void;
  /**
   * The card renderer's handler for team targets: `open_thread`, `team_command` and a hire
   * ticket's resolution. Returns `false` for targets it does not own, so the renderer falls
   * back; throws on failure so the card shows the reason.
   */
  onCardAction: (action: Action, card: Card, payload?: CardActionPayload) => boolean | Promise<void>;
}

const TeamActionsContext = createContext<TeamActions | null>(null);

export function useTeamActions(): TeamActions {
  const actions = useContext(TeamActionsContext);
  if (!actions) throw new Error("useTeamActions outside TeamActionsProvider");
  return actions;
}

/** For renderers that also mount outside the shell (tests, previews): no provider, no team targets. */
export function useOptionalTeamActions(): TeamActions | null {
  return useContext(TeamActionsContext);
}

export function TeamActionsProvider({ children }: { children: ReactNode }) {
  const client = useClient();
  const team = useTeam();
  const toast = useToast();
  const navigate = useNavigate();
  const [retiringId, setRetiringId] = useState<string | null>(null);
  const retiring = team.agents?.find((agent) => agent.id === retiringId);
  const { refresh, agents } = team;

  const onCardAction = useCallback(
    (action: Action, card: Card, payload?: CardActionPayload): boolean | Promise<void> => {
      const target = action.target;
      if (target.type === "open_thread") {
        navigate(`/agents/${encodeURIComponent(target.agent_id)}`);
        return true;
      }
      if (target.type === "ticket_resolve" && target.answer.type === "hire") {
        const ticketId = card.ticket_id;
        if (!ticketId) return false;
        const decision = target.answer.decision;
        const resolve = async () => {
          if (decision !== "hire") {
            await client.resolveTicket(ticketId, decision);
            return;
          }
          // The card carries only what it shows; the authoritative draft lives on the ticket.
          const ticket = (await client.listTickets("pending")).tickets.find((candidate) => candidate.id === ticketId);
          if (!ticket?.hire) throw new Error("This proposal is no longer open.");
          await client.resolveTicket(ticketId, decision, { hire: mergeHireDraft(ticket.hire, payload?.values) });
          refresh();
        };
        return resolve().catch((cause: unknown) => {
          throw cause instanceof ApiError ? new Error(cause.message) : cause;
        });
      }
      if (target.type !== "team_command") return false;
      if (target.command === "retire") {
        setRetiringId(target.agent_id);
        return true;
      }
      // Undo returns what the hire took over to its previous owners and archives the thread.
      return client
        .retireAgent(target.agent_id, { successor: null, pause_routines: false, revert_hire: true })
        .then((agent) => {
          refresh();
          toast.notify(`Undid hiring ${agent.name}`, "success");
        })
        .catch((cause: unknown) => {
          throw new Error(cause instanceof ApiError ? cause.message : `Could not undo hiring ${agents?.find((agent) => agent.id === target.agent_id)?.name ?? "them"}`);
        });
    },
    [client, navigate, refresh, toast, agents],
  );

  const value = useMemo<TeamActions>(() => ({ retire: setRetiringId, onCardAction }), [onCardAction]);
  return (
    <TeamActionsContext.Provider value={value}>
      {children}
      {retiring ? <RetireDialog agent={retiring} open onClose={() => setRetiringId(null)} /> : null}
    </TeamActionsContext.Provider>
  );
}
