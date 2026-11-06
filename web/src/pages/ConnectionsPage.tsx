import { Activity, History, Link2, LogOut, RefreshCw, ShieldCheck, Trash2, UserPlus } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
import type { Client } from "../api/client";
import type { Connection, ConnectionAuditAction, ConnectionList, ReloginResult } from "../api/types";
import { Avatar } from "../components/Avatar";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { errorMessage, useResource } from "../hooks/useResource";
import { t } from "../i18n";
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
import { accessLabel, accessOf, activeAgents, agentName, AUDIT_ACTIONS, auditLabel, identityLine, SiteMark, sourceLabel, statusOf } from "./connectionsView";
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
        toast.notify(t("connections.signedInToast", { site }), "success");
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
    counts.allowed ? t("connections.agents.allowed", { count: counts.allowed }) : null,
    counts.ask ? t("connections.agents.ask", { count: counts.ask }) : null,
    counts.denied ? t("connections.agents.denied", { count: counts.denied }) : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <button type="button" className="conn-agents" onClick={onOpen} aria-label={t("connections.agents.label", { site: connection.site, summary })}>
      <span className="conn-agent-stack">
        {agents.slice(0, 5).map((agent) => (
          <span key={agent.id} className="conn-agent" data-access={accessOf(connection, agent)} title={t("connections.agents.title", { agent: agent.name, access: accessLabel(accessOf(connection, agent)) })}>
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
    ? connection.used_by
      ? t("connections.card.lastUsedBy", { time: formatRelative(connection.used_at), agent: agentName(team.agents, connection.used_by) })
      : t("connections.card.lastUsed", { time: formatRelative(connection.used_at) })
    : t("connections.card.notUsed");
  return (
    <>
      <p className="conn-meta">
        <span>{used}</span>
        <span>{sourceLabel(connection.source)}</span>
        {connection.expires_at ? <span>{t("connections.card.signedInUntil", { date: formatDateTime(connection.expires_at) })}</span> : null}
      </p>
      {connection.identity_source === "agent" || connection.account_space || connection.accounts_on_site > 1 ? (
        <div className="resource-card-meta">
          {connection.identity_source === "agent" ? <span className="tag">{t("connections.card.namedByAgent")}</span> : null}
          {connection.account_space ? <span className="tag">{t("connections.card.separateProfile")}</span> : null}
          {connection.accounts_on_site > 1 ? <span className="tag">{t("connections.card.accountsOnSite", { count: connection.accounts_on_site })}</span> : null}
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
  /** Absent where the page is shown without its activity log. */
  onActivity?: (connection: Connection) => void;
  onAddAccount: (connection: Connection) => void;
  onRevoke: (connection: Connection, mode: RevokeMode) => void;
};

function ConnectionCard({ connection, handlers }: { connection: Connection; handlers: Handlers }) {
  const { browser, busy } = handlers;
  const working = busy === connection.id;
  const status = statusOf(connection.status);
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
            {t("connections.menu.check")}
          </MenuItem>
          <MenuItem icon={<Link2 size={18} strokeWidth={ICON_STROKE} />} disabled={!browser || working} onClick={() => handlers.onRelogin(connection)}>
            {t("connections.menu.relogin")}
          </MenuItem>
          <MenuItem icon={<ShieldCheck size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onAccess(connection)}>
            {t("connections.menu.access")}
          </MenuItem>
          {handlers.onActivity ? (
            <MenuItem icon={<History size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onActivity?.(connection)}>
              {t("connections.menu.activity")}
            </MenuItem>
          ) : null}
          {separate ? null : (
            <MenuItem icon={<UserPlus size={18} strokeWidth={ICON_STROKE} />} disabled={!browser} onClick={() => handlers.onAddAccount(connection)}>
              {t("connections.menu.addAccount")}
            </MenuItem>
          )}
          <MenuSeparator />
          {separate ? (
            <MenuItem danger icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onRevoke(connection, "sign_out")}>
              {t("connections.menu.removeAccount")}
            </MenuItem>
          ) : (
            <>
              <MenuItem danger icon={<LogOut size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onRevoke(connection, "sign_out")}>
                {t("connections.menu.signOut")}
              </MenuItem>
              <MenuItem danger icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />} onClick={() => handlers.onRevoke(connection, "forget")}>
                {t("connections.menu.forget")}
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
            <Button variant="primary" disabled={!browser || working} onClick={() => handlers.onRelogin(connection)}>
              {t("connections.menu.relogin")}
            </Button>
          ) : (
            <Button disabled={!browser || working} onClick={() => handlers.onCheck(connection)}>
              {t("connections.menu.check")}
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
        {t("connections.candidates.toggle", { count: candidates.length })}
      </Disclosure>
      {open ? (
        <>
          <p className="settings-text">{t("connections.candidates.note")}</p>
          <ul className="resource-list" aria-label={t("connections.candidates.label")}>
            {candidates.map((connection) => (
              <li key={connection.id}>
                <ResourceCard
                  data-connection={connection.id}
                  leading={<SiteMark connection={connection} />}
                  title={connection.site}
                  trailing={
                    <>
                      <Button disabled={!handlers.browser || handlers.busy === connection.id} onClick={() => handlers.onCheck(connection)}>
                        {t("connections.candidates.check")}
                      </Button>
                      <Button variant="ghost" disabled={handlers.busy === connection.id} onClick={() => onDismiss(connection)}>
                        {t("common.dismiss")}
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
    <SectionList title={t("connections.audit.title")} label={t("connections.audit.label")} description={t("connections.audit.description")}>
      <div className="conn-filters" role="group" aria-label={t("connections.audit.filters")}>
        <Select
          aria-label={t("connections.audit.connection")}
          value={filter}
          onChange={onFilter}
          options={[{ value: ALL, label: t("connections.audit.allConnections") }, ...connections.map((connection) => ({ value: connection.id, label: connection.identity ? `${connection.site} · ${connection.identity}` : connection.site }))]}
        />
        <Select
          aria-label={t("connections.audit.agent")}
          value={agent}
          onChange={setAgent}
          options={[{ value: ALL, label: t("connections.audit.anyAgent") }, ...activeAgents(team.agents).map((member) => ({ value: member.id, label: member.name }))]}
        />
        <Select
          aria-label={t("connections.audit.action")}
          value={action}
          onChange={(value) => setAction(value as ConnectionAuditAction | typeof ALL)}
          options={[{ value: ALL, label: t("connections.audit.anyAction") }, ...AUDIT_ACTIONS.map((value) => ({ value, label: auditLabel(value) }))]}
        />
      </div>
      <ResourceView resource={audit} what={t("connections.what")}>
        {(data) =>
          data.records.length === 0 ? (
            <p className="settings-text">{t("connections.audit.empty")}</p>
          ) : (
            <ol className="resource-list audit-list" aria-label={t("connections.audit.records")}>
              {data.records.map((record, index) => (
                <li key={`${record.at}-${index}`} data-action={record.action}>
                  <ResourceCard
                    title={`${auditLabel(record.action)} · ${sites.get(record.connection_id) ?? record.site}`}
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
 * `audit={false}` leaves out the activity log and its filters (Security & Privacy's short sheet).
 */
export function ConnectionsPage({ audit = true }: { audit?: boolean } = {}) {
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
          toast.notify(t(checked.status === "active" ? "connections.check.active" : "connections.check.needsSignIn", { site: checked.site }));
        }),
      onRelogin: (connection) =>
        void run(connection, async () => {
          follow(await client.reloginConnection(connection.id), connection.site, () => undefined);
        }),
      onAccess: (connection) => access.show(connection.id),
      onActivity: audit
        ? (connection) => {
            setAuditFilter(connection.id);
            window.requestAnimationFrame(() => auditRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }));
          }
        : undefined,
      onAddAccount: (connection) => add.show(connection.site),
      onRevoke: (connection, mode) => revoke.show({ connection, mode }),
    }),
    [list.data?.browser_available, busy, run, client, toast, follow, access, add, revoke, audit],
  );

  const dismiss = (connection: Connection) =>
    void run(connection, async () => {
      await client.dismissConnectionCandidate(connection.id);
    });

  if (list.forbidden) {
    return (
      <div className="manage-body" data-section="connections">
        <ScopeNotice what={t("connections.what")} onGranted={() => void reload()} />
      </div>
    );
  }

  const data = list.data;
  const accessTarget = data?.connections.find((connection) => connection.id === access.value) ?? null;
  return (
    <div className="manage-body" data-section="connections">
      <SectionList
        title={t("connections.page.title")}
        label={t("connections.page.label")}
        description={t("connections.page.description")}
        actions={data?.browser_available ? <AddButton variant="secondary" onClick={() => add.show("")}>{t("connections.page.addAccount")}</AddButton> : undefined}
      >
        <ResourceView resource={list} what={t("connections.what")}>
          {(value) => (
            <>
              {value.browser_available ? null : (
                <p className="conn-note" role="status">
                  {t("connections.page.browserUnavailable")}
                </p>
              )}
              {value.connections.length === 0 ? (
                <EmptyState
                  icon={Activity}
                  title={t("connections.page.emptyTitle")}
                  detail={t("connections.page.emptyDetail")}
                />
              ) : (
                <ul className="resource-list" aria-label={t("connections.page.label")}>
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
      {audit ? <div ref={auditRef}>{data ? <Audit client={client} connections={data.connections} filter={auditFilter} onFilter={setAuditFilter} /> : null}</div> : null}

      <RevokeDialog
        open={revoke.open}
        onClose={revoke.close}
        target={revoke.value}
        onConfirm={(connection, request) =>
          void run(connection, async () => {
            await client.revokeConnection(connection.id, request);
            toast.notify(
              t(connection.account_space !== null ? "connections.page.removedAccount" : request.sign_out ? "connections.page.signedOut" : "connections.page.forgot", { site: connection.site }),
            );
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
