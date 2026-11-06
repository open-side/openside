import { ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";
import type { AccessRequest } from "../api/types";
import { mayUse } from "../auth";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { formatList, t, type MessageKey } from "../i18n";
import { Button, ICON_STROKE } from "../ui";
import "./access.css";

const POLL_MS = 5_000;

const SCOPE_LABEL: Record<string, MessageKey> = {
  read: "shell.access.scope.read",
  steer: "shell.access.scope.steer",
  browse: "shell.access.scope.browse",
  approve: "shell.access.scope.approve",
  human_step: "shell.access.scope.human_step",
  admin: "shell.access.scope.admin",
};

// A "unit" list joins with commas and no "and", so English keeps its original "a, b, c" wording.
const describe = (request: AccessRequest) =>
  formatList(
    request.scopes.map((scope) => {
      const key = SCOPE_LABEL[scope];
      return key ? t(key) : scope;
    }),
    { type: "unit", style: "short" },
  );

/**
 * A paired device that hit a screen it has no permission for asks to be allowed. The person sees
 * the request here, on the computer running openside, and decides; only an admin session on this
 * computer can grant it.
 */
export function AccessRequestDock() {
  const client = useClient();
  const [requests, setRequests] = useState<AccessRequest[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A device without admin gets 403 for the list: stop asking instead of polling forever.
  const [allowed, setAllowed] = useState(() => mayUse("admin") !== false);

  const refresh = useCallback(async () => {
    try {
      setRequests((await client.accessRequests()).requests);
    } catch (cause) {
      if (cause instanceof ApiError && (cause.status === 403 || cause.status === 404)) setAllowed(false);
    }
  }, [client]);

  useEffect(() => {
    if (!allowed) return;
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [allowed, refresh]);

  async function decide(request: AccessRequest, approve: boolean) {
    setBusy(request.id);
    setError(null);
    try {
      if (approve) await client.approveAccessRequest(request.id);
      else await client.denyAccessRequest(request.id);
      setRequests((current) => current.filter((candidate) => candidate.id !== request.id));
    } catch (cause) {
      setError(errorMessage(cause));
      void refresh();
    } finally {
      setBusy(null);
    }
  }

  if (requests.length === 0) return null;
  return (
    <section className="access-dock" aria-label={t("shell.access.label")}>
      {requests.slice(0, 3).map((request) => (
        <article key={request.id} className="access-card surface-overlay">
          <span className="access-card-icon" aria-hidden="true">
            <ShieldCheck size={18} strokeWidth={ICON_STROKE} />
          </span>
          <div className="access-card-copy">
            <strong>{request.name ? t("shell.access.title", { name: request.name }) : t("shell.access.titleUnnamed")}</strong>
            <span>{t("shell.access.body", { scopes: describe(request) })}</span>
            {error ? (
              <span className="access-card-error" role="alert">
                {error}
              </span>
            ) : null}
          </div>
          <div className="access-card-actions">
            <Button variant="primary" disabled={busy !== null} onClick={() => void decide(request, true)}>
              {t("shell.access.allow")}
            </Button>
            <Button variant="ghost" disabled={busy !== null} onClick={() => void decide(request, false)}>
              {t("shell.access.decline")}
            </Button>
          </div>
        </article>
      ))}
    </section>
  );
}
