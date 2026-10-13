import { Check, LockKeyhole } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";
import { saveScopes } from "../auth";
import { useClient } from "../context";
import { Button, ICON_STROKE, Spinner } from "../ui";
import "./pages.css";

const POLL_MS = 3_000;

type Phase = "checking" | "idle" | "asking" | "waiting" | "granted" | "declined" | "unsupported" | "failed";

/**
 * A 403: this device was paired without the `admin` scope the screen needs. The person asks for it
 * here and approves it on the computer running openside, where the request appears.
 */
export function ScopeNotice({ what, onGranted }: { what: string; onGranted?: () => void }) {
  const client = useClient();
  const [phase, setPhase] = useState<Phase>("checking");
  const granted = useRef(onGranted);
  granted.current = onGranted;
  const wasWaiting = useRef(false);

  const check = useCallback(async () => {
    try {
      const me = await client.myDevice();
      if (me.scopes.includes("admin")) {
        saveScopes(me.scopes);
        setPhase("granted");
        granted.current?.();
      } else if (me.request) {
        wasWaiting.current = true;
        setPhase("waiting");
      } else {
        setPhase(wasWaiting.current ? "declined" : "idle");
        wasWaiting.current = false;
      }
    } catch (cause) {
      setPhase(cause instanceof ApiError && cause.status === 404 ? "unsupported" : "failed");
    }
  }, [client]);

  useEffect(() => {
    void check();
  }, [check]);

  useEffect(() => {
    if (phase !== "waiting") return;
    const timer = window.setInterval(() => void check(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [phase, check]);

  async function ask() {
    setPhase("asking");
    try {
      await client.requestAccess(["admin"]);
      wasWaiting.current = true;
      setPhase("waiting");
    } catch (cause) {
      setPhase(cause instanceof ApiError && cause.status === 404 ? "unsupported" : "failed");
    }
  }

  return (
    <div className="empty-state scope-notice" role="region" aria-label="Access needed">
      <span className="empty-state-icon" aria-hidden="true">
        {phase === "granted" ? <Check size={22} strokeWidth={ICON_STROKE} /> : <LockKeyhole size={22} strokeWidth={ICON_STROKE} />}
      </span>
      <strong>{phase === "granted" ? "This device now has access" : `This device can’t manage ${what} yet`}</strong>
      {phase === "waiting" ? (
        <p role="status">
          <Spinner size={14} /> Waiting for you to approve it on the computer running openside. The request is shown there.
        </p>
      ) : phase === "granted" ? (
        <p role="status">You can manage {what} from this device.</p>
      ) : phase === "unsupported" ? (
        <p>
          This version of openside cannot grant access from here. Give this device the admin permission on the computer running openside, under
          Workspace → Devices.
        </p>
      ) : (
        <>
          <p>
            {phase === "declined"
              ? "The request was declined or timed out. You can ask again."
              : phase === "failed"
                ? "Could not reach openside. Try again."
                : `Managing ${what} needs permission from the computer running openside. Ask for it here and approve it there.`}
          </p>
          {phase === "checking" ? null : (
            <Button size="compact" variant="primary" disabled={phase === "asking"} onClick={() => void ask()}>
              {phase === "asking" ? "Asking…" : "Ask for access"}
            </Button>
          )}
        </>
      )}
    </div>
  );
}
