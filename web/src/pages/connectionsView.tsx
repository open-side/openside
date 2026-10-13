import type { AgentSummary, Connection, ConnectionAccess, ConnectionAuditAction, ConnectionSource, ConnectionStatus } from "../api/types";
import { CHIEF_ID } from "../hooks/useAgents";
import { isImageDataUrl } from "../cards/values";
import type { BadgeTone } from "./blocks";

export const STATUS: Record<ConnectionStatus, { label: string; tone: BadgeTone }> = {
  active: { label: "Active", tone: "success" },
  needs_login: { label: "Sign in again", tone: "attention" },
  expired: { label: "Expired", tone: "danger" },
  unverified: { label: "Not checked", tone: "neutral" },
  candidate: { label: "Not checked", tone: "neutral" },
};

export const SOURCE: Record<ConnectionSource, string> = {
  login: "Signed in during a chat",
  vault: "Saved login",
  takeover: "Signed in by hand",
  otp: "Signed in with a one-time code",
  import: "Imported from your browser",
  sweep: "Found in the browser",
  share: "Shared from your browser",
};

export const ACCESS_LABEL: Record<ConnectionAccess, string> = {
  allowed: "Allowed",
  ask: "Ask me first",
  denied: "Blocked",
};

export const ACCESS_OPTIONS = (Object.keys(ACCESS_LABEL) as ConnectionAccess[]).map((value) => ({ value, label: ACCESS_LABEL[value] }));

export const AUDIT_LABEL: Record<ConnectionAuditAction, string> = {
  detected: "Signed in",
  verified: "Checked",
  expired: "Expired",
  needs_login: "Asked to sign in again",
  relogin: "Sign in again requested",
  open: "Opened the site",
  fetch: "Read from the site",
  grant: "Access changed",
  forget: "Forgotten",
  sign_out: "Signed out",
};

/** What an agent may do when nobody set it: the assistant is trusted, teammates ask first. */
export function accessOf(connection: Connection, agent: Pick<AgentSummary, "id">): ConnectionAccess {
  const grant = connection.grants.find((candidate) => candidate.agent_id === agent.id);
  if (grant) return grant.access;
  return agent.id === CHIEF_ID ? "allowed" : "ask";
}

/** Retired teammates keep their grants but cannot act, so they are not offered. */
export function activeAgents(agents: readonly AgentSummary[] | null): AgentSummary[] {
  return (agents ?? []).filter((agent) => agent.status === "active");
}

export function agentName(agents: readonly AgentSummary[] | null, id: string): string {
  const agent = agents?.find((candidate) => candidate.id === id);
  if (agent) return agent.name;
  return id === CHIEF_ID ? "Your assistant" : id;
}

export function identityLine(connection: Connection): string {
  return connection.identity ?? "Account not identified yet";
}

/** The page's own favicon (a `data:` URL the daemon bounded), else the site's initial. */
export function SiteMark({ connection, size = 36 }: { connection: Pick<Connection, "favicon" | "site">; size?: 24 | 36 }) {
  return (
    <span className={size === 36 ? "resource-card-icon site-mark site-mark--36" : "site-mark site-mark--24"} aria-hidden="true">
      {isImageDataUrl(connection.favicon) ? (
        <img src={connection.favicon} alt="" width={size - 10} height={size - 10} />
      ) : (
        <span className="site-mark-letter">{(connection.site[0] ?? "?").toUpperCase()}</span>
      )}
    </span>
  );
}
