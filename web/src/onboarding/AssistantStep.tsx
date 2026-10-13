import { useEffect, useState, type FormEvent } from "react";
import { ApiError } from "../api/client";
import { Avatar } from "../components/Avatar";
import { useClient } from "../context";
import { CHIEF_ID, useTeam } from "../hooks/useAgents";
import { ErrorNote, Loading } from "../pages/blocks";
import { Button } from "../ui";
import type { StepProps } from "./summary";

/** Names the assistant. The daemon already gave it a name, so skipping keeps that one. */
export function AssistantStep({ update }: StepProps) {
  const client = useClient();
  const team = useTeam();
  const chief = team.agents?.find((agent) => agent.id === CHIEF_ID) ?? null;
  const [name, setName] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (chief && name === null) setName(chief.name);
  }, [chief, name]);

  if (!chief || name === null) return team.error ? <ErrorNote>{team.error}</ErrorNote> : <Loading />;
  const trimmed = name.trim();
  const initials = (Array.from(trimmed)[0] ?? "?").toUpperCase();

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!chief || !trimmed) return;
    setBusy(true);
    setError(null);
    try {
      if (trimmed !== chief.name) await client.updateAgent(chief.id, { name: trimmed, initials });
      team.refresh();
      update({ assistant: trimmed });
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Could not save the name");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="onb-assistant" onSubmit={(event) => void save(event)} aria-label="Name your assistant">
      <Avatar initials={initials} color={chief.color} size={64} />
      <label className="onb-field">
        Name
        <input value={name} maxLength={24} required onChange={(event) => setName(event.target.value)} />
      </label>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <div className="onb-actions">
        <Button type="submit" variant="primary" size="compact" disabled={busy || !trimmed}>
          {trimmed === chief.name ? `Keep ${trimmed}` : `Call them ${trimmed || "…"}`}
        </Button>
      </div>
    </form>
  );
}
