import { useCallback, useState, type FormEvent } from "react";
import { Link } from "react-router";
import { ApiError } from "../api/client";
import type { AgentSummary, SuppressedProposal } from "../api/types";
import { Avatar } from "../components/Avatar";
import { useTeamActions } from "../components/TeamActions";
import { useClient } from "../context";
import { people, useTeam } from "../hooks/useAgents";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Dialog, DialogActions, MenuItem, useToast } from "../ui";
import { AddButton, Badge, ErrorNote, Loading, RowMenu, SectionList } from "./blocks";

/** Workspace → Team: everyone on the team at a glance, plus hiring and restoring by hand. */
export function TeamPage() {
  const team = useTeam();
  const [adding, setAdding] = useState(false);
  if (team.agents === null) return team.error ? <ErrorNote>{team.error}</ErrorNote> : <Loading />;
  const { chief, teammates, hidden } = people(team.agents);
  const active = [...(chief ? [chief] : []), ...teammates, ...hidden];
  const retired = team.agents.filter((agent) => agent.status === "retired");

  return (
    <>
      <SectionList
        summary={`${teammates.length + hidden.length} ${teammates.length + hidden.length === 1 ? "teammate" : "teammates"}${chief ? ` and ${chief.name}` : ""}`}
        description="Teammates own recurring work. Usually your assistant suggests one when something keeps coming back; you can also add one yourself."
        actions={<AddButton onClick={() => setAdding(true)}>Add teammate</AddButton>}
      >
        <ul className="resource-list" aria-label="Team">
          {active.map((agent) => (
            <li key={agent.id}>
              <TeamRow agent={agent} />
            </li>
          ))}
        </ul>
      </SectionList>
      {retired.length > 0 ? (
        <SectionList title="Retired" description="Retired teammates keep their thread in History. Restoring brings them back with their profile.">
          <ul className="resource-list" aria-label="Retired teammates">
            {retired.map((agent) => (
              <li key={agent.id}>
                <TeamRow agent={agent} />
              </li>
            ))}
          </ul>
        </SectionList>
      ) : null}
      <TurnedOffSuggestions />
      <AddTeammateDialog open={adding} onClose={() => setAdding(false)} />
    </>
  );
}

/** Topics answered with Never: the assistant stays quiet about them until allowed again. */
function TurnedOffSuggestions() {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.suppressedProposals(), [client]);
  const { data, error, reload } = useResource(load);
  const [clearing, setClearing] = useState<string | null>(null);
  if (!data?.length && !error) return null;

  const allow = async (proposal: SuppressedProposal) => {
    setClearing(proposal.key);
    try {
      await client.clearSuppressedProposal(proposal.key);
      await reload();
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    } finally {
      setClearing(null);
    }
  };

  return (
    <SectionList title="Suggestions you turned off" description="Your assistant won’t suggest a teammate for these again unless you allow it.">
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <ul className="resource-list" aria-label="Suggestions you turned off">
        {(data ?? []).map((proposal) => (
          <li key={proposal.key}>
            <article className="resource-card">
              <div className="resource-card-header">
                <div className="resource-card-copy">
                  <h4 className="resource-card-title">
                    <span>{proposal.label || proposal.key}</span>
                  </h4>
                  <p className="resource-card-detail">Turned off {new Date(proposal.decided_at).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })}</p>
                </div>
                <div className="resource-card-trailing">
                  <Button size="compact" disabled={clearing === proposal.key} onClick={() => void allow(proposal)}>
                    Allow again
                  </Button>
                </div>
              </div>
            </article>
          </li>
        ))}
      </ul>
    </SectionList>
  );
}

function TeamRow({ agent }: { agent: AgentSummary }) {
  const client = useClient();
  const team = useTeam();
  const toast = useToast();
  const { retire } = useTeamActions();
  const status =
    agent.status === "retired"
      ? null
      : agent.needs_you
        ? ({ tone: "attention", label: "Needs you" } as const)
        : agent.session_status === "running"
          ? ({ tone: "neutral", label: "Working" } as const)
          : null;
  const owns = agent.persona.responsibilities;
  const restore = async () => {
    try {
      await client.restoreAgent(agent.id);
      team.refresh();
    } catch (cause) {
      toast.notify(cause instanceof ApiError ? cause.message : `Could not restore ${agent.name}`, "error");
    }
  };
  const thread = agent.status === "retired" ? `/sessions/${encodeURIComponent(agent.home_session_id)}` : `/agents/${encodeURIComponent(agent.id)}`;
  return (
    <article className="resource-card team-row" data-agent={agent.id}>
      <div className="resource-card-header">
        <Avatar initials={agent.initials} color={agent.color} size={40} />
        <div className="resource-card-copy">
          <h4 className="resource-card-title">
            <span>{agent.name}</span>
            {agent.kind === "chief" ? <Badge>Assistant</Badge> : null}
            {agent.hidden ? <Badge>Hidden</Badge> : null}
            {status ? <Badge tone={status.tone}>{status.label}</Badge> : null}
          </h4>
          <p className="resource-card-detail">{agent.persona.role}</p>
          {owns.length > 0 ? <p className="resource-card-detail team-owns">Owns: {owns.join(" · ")}</p> : null}
        </div>
        <div className="resource-card-trailing">
          <Link to={thread} className="ui-button ui-button--secondary ui-button--compact">
            Open
          </Link>
          {agent.kind === "chief" ? null : (
            <RowMenu name={agent.name}>
              {agent.status === "retired" ? (
                <MenuItem onClick={() => void restore()}>Restore {agent.name}</MenuItem>
              ) : (
                <MenuItem danger onClick={() => retire(agent.id)}>
                  Retire {agent.name}…
                </MenuItem>
              )}
            </RowMenu>
          )}
        </div>
      </div>
    </article>
  );
}

/** A direct hire: who they are and what they own. Everything else starts from the defaults. */
function AddTeammateDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const client = useClient();
  const team = useTeam();
  const toast = useToast();
  const [name, setName] = useState("");
  const [role, setRole] = useState("");
  const [responsibilities, setResponsibilities] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const owned = responsibilities
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const agent = await client.createAgent({
        name: name.trim(),
        initials: null,
        color: null,
        persona: { role: role.trim(), voice: "", responsibilities: owned, boundaries: [] },
        skills: null,
        model: null,
        account_pool: null,
        channel: null,
        mode: null,
        routine_ids: [],
        memory_ids: [],
        history_summary: "",
        proposal_key: null,
      });
      team.refresh();
      toast.notify(`${agent.name} joined your team.`, "success");
      setName("");
      setRole("");
      setResponsibilities("");
      onClose();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Could not add the teammate");
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title="Add teammate" description="Give them a name, a specific job and what they own. You can change everything later in their profile.">
      <form className="dialog-form" onSubmit={(event) => void submit(event)} aria-label="Add teammate">
        <label className="field">
          <span>Name</span>
          <input value={name} maxLength={24} required onChange={(event) => setName(event.target.value)} placeholder="Mina" />
        </label>
        <label className="field">
          <span>Role</span>
          <input value={role} maxLength={60} required onChange={(event) => setRole(event.target.value)} placeholder="Watches prices and plans for you" />
        </label>
        <label className="field">
          <span>Responsibilities · one per line</span>
          <textarea rows={3} value={responsibilities} required onChange={(event) => setResponsibilities(event.target.value)} placeholder="Weekly price check on the Team plan" />
        </label>
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy || !name.trim() || !role.trim() || owned.length === 0}>
            Add {name.trim() || "teammate"}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}
