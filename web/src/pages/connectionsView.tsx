import type { AgentSummary, Connection, ConnectionAccess, ConnectionAuditAction, ConnectionSource, ConnectionStatus } from "../api/types";
import { CHIEF_ID } from "../hooks/useAgents";
import { isImageDataUrl } from "../cards/values";
import { t, type MessageKey } from "../i18n";
import type { BadgeTone } from "./blocks";

const STATUS_LABEL: Record<ConnectionStatus, MessageKey> = {
  active: "connections.status.active",
  needs_login: "connections.status.needs_login",
  expired: "connections.status.expired",
  unverified: "connections.status.unverified",
  candidate: "connections.status.unverified",
};

const STATUS_TONE: Record<ConnectionStatus, BadgeTone> = {
  active: "success",
  needs_login: "attention",
  expired: "danger",
  unverified: "neutral",
  candidate: "neutral",
};

export function statusOf(status: ConnectionStatus): { label: string; tone: BadgeTone } {
  return { label: t(STATUS_LABEL[status]), tone: STATUS_TONE[status] };
}

const SOURCE_LABEL: Record<ConnectionSource, MessageKey> = {
  login: "connections.source.login",
  vault: "connections.source.vault",
  takeover: "connections.source.takeover",
  otp: "connections.source.otp",
  import: "connections.source.import",
  sweep: "connections.source.sweep",
  share: "connections.source.share",
};

export const sourceLabel = (source: ConnectionSource) => t(SOURCE_LABEL[source]);

const ACCESS_LABEL: Record<ConnectionAccess, MessageKey> = {
  allowed: "connections.access.allowed",
  ask: "connections.access.ask",
  denied: "connections.access.denied",
};

export const accessLabel = (access: ConnectionAccess) => t(ACCESS_LABEL[access]);

export function accessOptions(): { value: ConnectionAccess; label: string }[] {
  return (Object.keys(ACCESS_LABEL) as ConnectionAccess[]).map((value) => ({ value, label: accessLabel(value) }));
}

const AUDIT_LABEL: Record<ConnectionAuditAction, MessageKey> = {
  detected: "connections.auditAction.detected",
  verified: "connections.auditAction.verified",
  expired: "connections.auditAction.expired",
  needs_login: "connections.auditAction.needs_login",
  relogin: "connections.auditAction.relogin",
  open: "connections.auditAction.open",
  fetch: "connections.auditAction.fetch",
  grant: "connections.auditAction.grant",
  forget: "connections.auditAction.forget",
  sign_out: "connections.auditAction.sign_out",
};

export const AUDIT_ACTIONS = Object.keys(AUDIT_LABEL) as ConnectionAuditAction[];

export const auditLabel = (action: ConnectionAuditAction) => t(AUDIT_LABEL[action]);

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
  return id === CHIEF_ID ? t("connections.yourAssistant") : id;
}

export function identityLine(connection: Connection): string {
  return connection.identity ?? t("connections.identityUnknown");
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
