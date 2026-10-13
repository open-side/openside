import type { AgentSummary } from "../api/types";

/** A roster row as GET /v1/agents returns it; `id === "chief"` makes the assistant. */
export function agentSummary(id: string, name: string, extra: Partial<AgentSummary> = {}): AgentSummary {
  return {
    id,
    kind: id === "chief" ? "chief" : "teammate",
    name,
    initials: name.slice(0, 1).toUpperCase(),
    color: "#2f6fed",
    persona: { role: `${name}'s role`, voice: "", responsibilities: id === "chief" ? [] : ["Weekly check"], boundaries: [] },
    status: "active",
    hidden: false,
    pinned: false,
    mode: "guard",
    model: null,
    account_pool: null,
    channel: null,
    skills: null,
    home_session_id: `s-${id}`,
    session_status: "idle",
    activity: "",
    last_activity: "",
    last_message_at: "2026-10-12T09:00:00Z",
    last_seq: 1,
    needs_you: false,
    created_at: "2026-10-01T09:00:00Z",
    retired_at: null,
    persona_version: 1,
    ...extra,
  };
}
