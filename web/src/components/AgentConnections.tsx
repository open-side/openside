import { useCallback, useState } from "react";
import { Link, useLocation } from "react-router";
import type { Connection, ConnectionAccess } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { ErrorNote, Loading } from "../pages/blocks";
import { ACCESS_OPTIONS, accessOf, identityLine, SiteMark } from "../pages/connectionsView";
import { manageLinkState } from "../pages/manageRoutes";
import { Select } from "../ui";
import "../pages/connections.css";

/**
 * Which signed-in sites one teammate may use. Rendered inside the profile form, so it submits
 * nothing: each choice is saved as soon as it is made, like on the Connections page.
 */
export function AgentConnections({ agentId, agentName }: { agentId: string; agentName: string }) {
  const client = useClient();
  const location = useLocation();
  const load = useCallback(() => client.connections(), [client]);
  const list = useResource(load);
  const [pending, setPending] = useState<string | null>(null);
  const { reload, setError } = list;

  async function change(connection: Connection, access: ConnectionAccess) {
    setPending(connection.id);
    setError(null);
    try {
      await client.updateConnection(connection.id, { grants: [{ agent_id: agentId, access }], home_url: null });
      await reload();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setPending(null);
    }
  }

  if (!list.data) {
    return list.error ? <p className="muted profile-note">{list.forbidden ? "This device needs admin access to manage connections." : list.error}</p> : <Loading />;
  }
  const { connections } = list.data;
  return (
    <>
      {list.error ? <ErrorNote>{list.error}</ErrorNote> : null}
      {connections.length === 0 ? (
        <p className="muted profile-note">No sites are signed in yet.</p>
      ) : (
        <ul className="resource-list conn-access-list" aria-label={`${agentName}'s access to signed-in sites`}>
          {connections.map((connection) => (
            <li key={connection.id} className="conn-access-row">
              <SiteMark connection={connection} size={24} />
              <span className="conn-access-name">
                {connection.site}
                <span className="conn-access-identity">{identityLine(connection)}</span>
              </span>
              <Select
                variant="inline"
                aria-label={`${agentName}'s access to ${connection.site}${connection.identity ? ` (${connection.identity})` : ""}`}
                disabled={pending === connection.id}
                value={accessOf(connection, { id: agentId })}
                onChange={(value) => void change(connection, value as ConnectionAccess)}
                options={ACCESS_OPTIONS}
              />
            </li>
          ))}
        </ul>
      )}
      <div className="profile-links">
        <Link to="/connections" state={manageLinkState(location)} className="action-link">
          Manage connections
        </Link>
      </div>
    </>
  );
}
