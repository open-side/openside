import { Activity, History, Link2, LogOut, RefreshCw, ShieldCheck, Trash2, UserPlus } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
import type { Client } from "../api/client";
import type { Connection, ConnectionAuditAction, ConnectionList, ReloginResult } from "../api/types";
import { Avatar } from "../components/Avatar";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, ICON_STROKE, MenuItem, MenuSeparator, Select, useToast } from "../ui";
import {
  AddButton,
  CardActions,
  Disclosure,
  EmptyState,
  ResourceCard,
  ResourceView,
  RowMenu,
  SectionList,
  useDialogState,
} from "./blocks";
import { AccessDialog, AddAccountDialog, RevokeDialog, type RevokeMode } from "./ConnectionDialogs";
import { ACCESS_LABEL, AUDIT_LABEL, accessOf, activeAgents, agentName, identityLine, SiteMark, SOURCE, STATUS } from "./connectionsView";
import { formatDateTime, formatRelative } from "./format";
import { ScopeNotice } from "./ScopeNotice";
import "./connections.css";

/** Where an unfinished sign-in waits: the conversation that owns the ticket opens it as a sheet. */
export function ticketPath(result: Extract<ReloginResult, { status: "needs_you" }>): string {
  return `/sessions/${encodeURIComponent(result.ticket.session_id)}?ticket=${encodeURIComponent(result.ticket.ticket_id)}`;
}

/**
 * Runs a sign-in and either says it finished or leaves for the ticket the person completes.
 * Shared by "Sign in again" and by creating a separate account.
 */
export function useFollowSignIn() {
  const navigate = useNavigate();
  const toast = useToast();
  return useCallback(
    (result: ReloginResult, site: string, done: () => void) => {
      if (result.status === "signed_in") {
        toast.notify(`Signed in to ${site}.`, "success");
        done();
      } else {
        navigate(ticketPath(result));
      }
    },
    [navigate, toast],
  );
}

function AgentAccess({ connection, onOpen }: { connection: Connection; onOpen: () => void }) {
  const team = useTeam();
  const agents = activeAgents(team.agents);
  if (agents.length === 0) return null;
  const counts = { allowed: 0, ask: 0, denied: 0 };
  for (const agent of agents) counts[accessOf(connection, agent)] += 1;
  const summary = [
    counts.allowed ? `${counts.allowed} allowed` : null,
    counts.ask ? `${counts.ask} ask first` : null,
    counts.denied ? `${counts.denied} blocked` : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <button type="button" className="conn-agents" onClick={onOpen} aria-label={`Agents with access to ${connection.site}: ${summary}`}>
      <span className="conn-agent-stack">
        {agents.slice(0, 5).map((agent) => (
          <span key={agent.id} className="conn-agent" data-access={accessOf(connection, agent)} title={`${agent.name}: ${ACCESS_LABEL[accessOf(connection, agent)]}`}>
            <Avatar chief={agent.kind === "chief"} initials={agent.initials} color={agent.color} size={24} />
          </span>
        ))}
        {agents.length > 5 ? <span className="conn-agent-more">+{agents.length - 5}</span> : null}
      </span>
      <span className="conn-agents-summary">{summary}</span>
    </button>
  );
}

function CardMeta({ connection }: { connection: Connection }) {
  const team = useTeam();
  const used = connection.used_at
    ? `Last used ${formatRelative(connection.used_at)}${connection.used_by ? ` by ${agentName(team.agents, connection.used_by)}` : ""}`
    : "Not used by an agent yet";
  return (
    <>
      <p className="conn-meta">
        <span>{used}</span>
        <span>{SOURCE[connection.source]}</span>
        {connection.expires_at ? <span>Signed in until {formatDateTime(connection.expires_at)}</span> : null}
      </p>
      {connection.identity_source === "agent" || connection.account_space || connection.accounts_on_site > 1 ? (
        <div className="resource-card-meta">
          {connection.identity_source === "agent" ? <span className="tag">Account named by an agent</span> : null}
          {connection.account_space ? <span className="tag">Separate browser profile</span> : null}
          {connection.accounts_on_site > 1 ? <span className="tag">{connection.accounts_on_site} accounts on this site</span> : null}
        </div>
      ) : null}
    </>
  );
}

type Handlers = {
  browser: boolean;
  busy: string | null;
  onCheck: (connection: Connection) => void;
  onRelogin: (connection: Connection) => void;
  onAccess: (connection: Connection) => void;
  onActivity: (connection: Connection) => void;
  onAddAccount: (connection: Connection) => void;
  onRevoke: (connection: Connection, mode: RevokeMode) => void;
};

function ConnectionCard({ connection, handlers }: { connection: Connection; handlers: Handlers }) {
  const { browser, busy } = handlers;
  const working = busy === connection.id;
  const status = STATUS[connection.status];
  const needsSignIn = connection.status === "needs_login" || connection.status === "expired";
  const separate = connection.account_space !== null;
  return (
    <ResourceCard
      data-connection={connection.id}
      data-status={connection.status}
      leading={<SiteMark connection={connection} />}
      title={connection.site}
      badge={status.label}
      badgeTone={status.tone}
      detail={<span className={connection.identity ? undefined : "conn-unknown"}>{identityLine(connection)}</span>}
      trailing={
        <RowMenu name={connection.site}>
          <MenuItem icon={<RefreshCw size={18} strokeWidth={ICON_STROKE} />} disabled={!browser || working} onClick={() => handlers.onCheck(connection)}>
            Check now
          </MenuItem>
          <MenuItem icon={<Link2 size={18} strokeWidth={ICON_STROKE} />} disabled={!browser || working} onClick={() => handlers.onRelogin(connection)}>
            Sign in again
          </MenuItem>
          <MenuItem icon={<ShieldCheck size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onAccess(connection)}>
            Agent access…
          </MenuItem>
          <MenuItem icon={<History size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onActivity(connection)}>
            Activity
          </MenuItem>
          {separate ? null : (
            <MenuItem icon={<UserPlus size={18} strokeWidth={ICON_STROKE} />} disabled={!browser} onClick={() => handlers.onAddAccount(connection)}>
              Add another account…
            </MenuItem>
          )}
          <MenuSeparator />
          {separate ? (
            <MenuItem danger icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onRevoke(connection, "sign_out")}>
              Remove account…
            </MenuItem>
          ) : (
            <>
              <MenuItem danger icon={<LogOut size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onRevoke(connection, "sign_out")}>
                Sign out…
              </MenuItem>
              <MenuItem danger icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onRevoke(connection, "forget")}>
                Forget…
              </MenuItem>
            </>
          )}
        </RowMenu>
      }
    >
      <CardMeta connection={connection} />
      <AgentAccess connection={connection} onOpen={() => handlers.onAccess(connection)} />
      {needsSignIn || connection.status === "unverified" ? (
        <CardActions>
          {needsSignIn ? (
            <Button variant="primary" size="compact" disabled={!browser || working} onClick={() => handlers.onRelogin(connection)}>
              Sign in again
            </Button>
          ) : (
            <Button size="compact" disabled={!browser || working} onClick={() => handlers.onCheck(connection)}>
              Check now
            </Button>
          )}
        </CardActions>
      ) : null}
    </ResourceCard>
  );
}

function Candidates({ candidates, handlers, onDismiss }: { candidates: Connection[]; handlers: Handlers; onDismiss: (connection: Connection) => void }) {
  const [open, setOpen] = useState(false);
  if (candidates.length === 0) return null;
  return (
    <div className="conn-candidates">
      <Disclosure expanded={open} onToggle={() => setOpen(!open)}>
        Possibly signed in ({candidates.length})
      </Disclosure>
      {open ? (
        <>
          <p className="settings-text">The agent browser holds cookies for these sites, but nobody confirmed a sign-in. Check opens the site in the background to find out.</p>
          <ul className="resource-list" aria-label="Possibly signed in">
            {candidates.map((connection) => (
              <li key={connection.id}>
                <ResourceCard
                  data-connection={connection.id}
                  leading={<SiteMark connection={connection} />}
                  title={connection.site}
                  trailing={
                    <>
                      <Button size="compact" disabled={!handlers.browser || handlers.busy === connection.id} onClick={() => handlers.onCheck(connection)}>
                        Check
                      </Button>
                      <Button size="compact" variant="ghost" disabled={handlers.busy === connection.id} onClick={() => onDismiss(connection)}>
                        Dismiss
                      </Button>
                    </>
                  }
                />
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </div>
  );
}

const ALL = "";

function Audit({ client, connections, filter, onFilter }: { client: Client; connections: Connection[]; filter: string; onFilter: (id: string) => void }) {
  const team = useTeam();
  const [agent, setAgent] = useState(ALL);
  const [action, setAction] = useState<ConnectionAuditAction | typeof ALL>(ALL);
  const load = useCallback(
    () => client.connectionAudit({ connection: filter || undefined, agent: agent || undefined, action: action || undefined, limit: 100 }),
    [client, filter, agent, action],
  );
  const audit = useResource(load);
  const sites = new Map(connections.map((connection) => [connection.id, connection.site]));
  return (
    <SectionList title="Activity" label="Connection activity" description="Every use and change of a sign-in, newest first. Cookies and page contents are never recorded.">
      <div className="conn-filters" role="group" aria-label="Activity filters">
        <Select
          aria-label="Connection"
          value={filter}
          onChange={onFilter}
          options={[{ value: ALL, label: "All connections" }, ...connections.map((connection) => ({ value: connection.id, label: connection.identity ? `${connection.site} · ${connection.identity}` : connection.site }))]}
        />
        <Select
          aria-label="Agent"
          value={agent}
          onChange={setAgent}
          options={[{ value: ALL, label: "Any agent" }, ...activeAgents(team.agents).map((member) => ({ value: member.id, label: member.name }))]}
        />
        <Select
          aria-label="Action"
          value={action}
          onChange={(value) => setAction(value as ConnectionAuditAction | typeof ALL)}
          options={[{ value: ALL, label: "Any action" }, ...(Object.keys(AUDIT_LABEL) as ConnectionAuditAction[]).map((value) => ({ value, label: AUDIT_LABEL[value] }))]}
        />
      </div>
      <ResourceView resource={audit} what="connections">
        {(data) =>
          data.records.length === 0 ? (
            <p className="settings-text">No activity matches.</p>
          ) : (
            <ol className="resource-list audit-list" aria-label="Activity records">
              {data.records.map((record, index) => (
                <li key={`${record.at}-${index}`} data-action={record.action}>
                  <ResourceCard
                    title={`${AUDIT_LABEL[record.action]} · ${sites.get(record.connection_id) ?? record.site}`}
                    detail={`${formatDateTime(record.at)} · ${agentName(team.agents, record.agent_id)}`}
                  >
                    {record.detail ? <p className="resource-card-detail">{record.detail}</p> : null}
                  </ResourceCard>
                </li>
              ))}
            </ol>
          )
        }
      </ResourceView>
    </SectionList>
  );
}

/**
 * Connections: the sites the agent browser is signed in to, one card per account. It shows
 * metadata only; cookies and passwords never reach the page. Every route needs the admin scope.
 */
export function ConnectionsPage() {
  const client = useClient();
  const toast = useToast();
  const follow = useFollowSignIn();
  const load = useCallback(() => client.connections(), [client]);
  const list = useResource<ConnectionList>(load);
  const { reload, setError } = list;
  const [busy, setBusy] = useState<string | null>(null);
  const [auditFilter, setAuditFilter] = useState("");
  const auditRef = useRef<HTMLDivElement>(null);
  const revoke = useDialogState<{ connection: Connection; mode: RevokeMode }>();
  const access = useDialogState<string>();
  const add = useDialogState<string>();

  // The registry is not event-sourced: sign-ins finish in other tabs and conversations, so
  // coming back to the page re-reads it.
  useEffect(() => {
    const refresh = () => {
      if (document.visibilityState === "visible") void reload();
    };
    document.addEventListener("visibilitychange", refresh);
    return () => document.removeEventListener("visibilitychange", refresh);
  }, [reload]);

  const run = useCallback(
    async (connection: Connection, action: () => Promise<void>) => {
      setBusy(connection.id);
      setError(null);
      try {
        await action();
        await reload();
      } catch (cause) {
        // reload() clears the error on success, so show ours after it.
        await reload();
        setError(errorMessage(cause));
      } finally {
        setBusy(null);
      }
    },
    [reload, setError],
  );

  const handlers: Handlers = useMemo(
    () => ({
      browser: list.data?.browser_available ?? false,
      busy,
      onCheck: (connection) =>
        void run(connection, async () => {
          const checked = await client.checkConnection(connection.id);
          toast.notify(checked.status === "active" ? `${checked.site} is still signed in.` : `${checked.site} needs a new sign-in.`);
        }),
      onRelogin: (connection) =>
        void run(connection, async () => {
          follow(await client.reloginConnection(connection.id), connection.site, () => undefined);
        }),
      onAccess: (connection) => access.show(connection.id),
      onActivity: (connection) => {
        setAuditFilter(connection.id);
        window.requestAnimationFrame(() => auditRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }));
      },
      onAddAccount: (connection) => add.show(connection.site),
      onRevoke: (connection, mode) => revoke.show({ connection, mode }),
    }),
    [list.data?.browser_available, busy, run, client, toast, follow, access, add, revoke],
  );

  const dismiss = (connection: Connection) =>
    void run(connection, async () => {
      await client.dismissConnectionCandidate(connection.id);
    });

  if (list.forbidden) {
    return (
      <div className="manage-body" data-section="connections">
        <ScopeNotice what="connections" onGranted={() => void reload()} />
      </div>
    );
  }

  const data = list.data;
  const accessTarget = data?.connections.find((connection) => connection.id === access.value) ?? null;
  return (
    <div className="manage-body" data-section="connections">
      <SectionList
        title="Signed in"
        label="Connections"
        description="Openside remembers which sites the agent browser is signed in to, never the passwords or cookies. Agents ask before using one unless you allow it."
        actions={data?.browser_available ? <AddButton variant="secondary" onClick={() => add.show("")}>Add another account</AddButton> : undefined}
      >
        <ResourceView resource={list} what="connections">
          {(value) => (
            <>
              {value.browser_available ? null : (
                <p className="conn-note" role="status">
                  The agent browser is not available, so openside cannot check sites or sign in again right now.
                </p>
              )}
              {value.connections.length === 0 ? (
                <EmptyState
                  icon={Activity}
                  title="No sign-ins yet"
                  detail="When an agent signs in to a site for you, it shows up here, and you decide which agents may use it."
                />
              ) : (
                <ul className="resource-list" aria-label="Connections">
                  {value.connections.map((connection) => (
                    <li key={connection.id}>
                      <ConnectionCard connection={connection} handlers={handlers} />
                    </li>
                  ))}
                </ul>
              )}
              <Candidates candidates={value.candidates} handlers={handlers} onDismiss={dismiss} />
            </>
          )}
        </ResourceView>
      </SectionList>
      <div ref={auditRef}>{data ? <Audit client={client} connections={data.connections} filter={auditFilter} onFilter={setAuditFilter} /> : null}</div>

      <RevokeDialog
        open={revoke.open}
        onClose={revoke.close}
        target={revoke.value}
        onConfirm={(connection, request) =>
          void run(connection, async () => {
            await client.revokeConnection(connection.id, request);
            toast.notify(connection.account_space !== null ? `Removed the ${connection.site} account.` : request.sign_out ? `Signed out of ${connection.site}.` : `Forgot ${connection.site}.`);
          })
        }
      />
      <AccessDialog open={access.open} onClose={access.close} connection={accessTarget} onChanged={() => void reload()} />
      <AddAccountDialog
        open={add.open}
        onClose={add.close}
        site={add.value ?? ""}
        sites={[...new Set(data?.connections.map((connection) => connection.site))]}
        onCreated={(result, site) => {
          add.close();
          follow(result, site, () => void reload());
        }}
      />
    </div>
  );
}
