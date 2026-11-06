import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../api/client";
import type { Action, Agent, Card, HireDraft } from "../api/types";
import { useClient } from "../context";
import { CHIEF_ID, useTeam } from "../hooks/useAgents";
import { ErrorNote } from "../pages/blocks";
import { t } from "../i18n";
import { Button, Dialog, DialogActions, Segmented, Select, useToast } from "../ui";

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
  /**
   * The card renderer's handler for team targets: `open_thread`, `team_command` and a hire
   * ticket's resolution. Returns `false` for targets it does not own, so the renderer falls
   * back; throws on failure so the card shows the reason.
   */
  onCardAction: (action: Action, card: Card, payload?: CardActionPayload) => boolean | Promise<void>;
}

const TeamActionsContext = createContext<TeamActions | null>(null);

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
        // Every conversation is a chat now; a teammate's thread is its home session.
        const home = agents?.find((agent) => agent.id === target.agent_id)?.home_session_id;
        navigate(home ? `/sessions/${encodeURIComponent(home)}` : "/chats");
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
          if (!ticket?.hire) throw new Error(t("agents.hire.closed"));
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
          toast.notify(t("agents.hire.undone", { name: agent.name }), "success");
        })
        .catch((cause: unknown) => {
          if (cause instanceof ApiError) throw new Error(cause.message);
          const name = agents?.find((agent) => agent.id === target.agent_id)?.name;
          throw new Error(name ? t("agents.hire.undoFailed", { name }) : t("agents.hire.undoFailedUnknown"));
        });
    },
    [client, navigate, refresh, toast, agents],
  );

  const value = useMemo<TeamActions>(() => ({ onCardAction }), [onCardAction]);
  return (
    <TeamActionsContext.Provider value={value}>
      {children}
      {retiring ? <RetireDialog agent={retiring} open onClose={() => setRetiringId(null)} /> : null}
    </TeamActionsContext.Provider>
  );
}

/** Retire hands a teammate's work to someone else, or pauses it; the thread moves to History. */
function RetireDialog({ agent, open, onClose }: { agent: Agent; open: boolean; onClose: () => void }) {
  const client = useClient();
  const team = useTeam();
  const toast = useToast();
  const [successor, setSuccessor] = useState(CHIEF_ID);
  const [pause, setPause] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const others = (team.agents ?? []).filter((candidate) => candidate.status === "active" && candidate.id !== agent.id);

  async function retire() {
    setBusy(true);
    setError(null);
    try {
      await client.retireAgent(agent.id, { successor: pause ? null : successor, pause_routines: pause, revert_hire: false });
      team.refresh();
      toast.notify(t("agents.retire.done", { name: agent.name }), "success");
      onClose();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : t("agents.retire.failed", { name: agent.name }));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title={t("agents.retire.title", { name: agent.name })} description={t("agents.retire.description", { name: agent.name })}>
      <div className="dialog-form">
        <Segmented
          label={t("agents.retire.routines")}
          options={[
            { value: "hand", label: t("agents.retire.handOver") },
            { value: "pause", label: t("agents.retire.pause") },
          ]}
          value={pause ? "pause" : "hand"}
          onChange={(value) => setPause(value === "pause")}
        />
        {pause ? null : (
          <label className="field">
            <span>{t("agents.retire.successor")}</span>
            <Select
              value={successor}
              onChange={setSuccessor}
              variant="field"
              aria-label={t("agents.retire.successor")}
              options={others.map((candidate) => ({ value: candidate.id, label: candidate.name, detail: candidate.persona.role }))}
            />
          </label>
        )}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button onClick={onClose}>{t("agents.retire.keep", { name: agent.name })}</Button>
          <Button variant="danger" disabled={busy} onClick={() => void retire()}>
            {t("agents.retire.confirm")}
          </Button>
        </DialogActions>
      </div>
    </Dialog>
  );
}
