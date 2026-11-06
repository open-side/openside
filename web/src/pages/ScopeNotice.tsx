import { Check, LockKeyhole } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";
import { saveScopes } from "../auth";
import { useClient } from "../context";
import { t } from "../i18n";
import { Button, ICON_STROKE, Spinner } from "../ui";
import "./pages.css";
import "./signin.css";

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
    <div className="empty-state scope-notice" role="region" aria-label={t("settings.scope.region")}>
      <span className="empty-state-icon" aria-hidden="true">
        {phase === "granted" ? <Check size={22} strokeWidth={ICON_STROKE} /> : <LockKeyhole size={22} strokeWidth={ICON_STROKE} />}
      </span>
      <strong>{phase === "granted" ? t("settings.scope.granted") : t("settings.scope.cannotManage", { what })}</strong>
      {phase === "waiting" ? (
        <p role="status">
          <Spinner size={14} /> {t("settings.scope.waiting")}
        </p>
      ) : phase === "granted" ? (
        <p role="status">{t("settings.scope.grantedDetail", { what })}</p>
      ) : phase === "unsupported" ? (
        <p>{t("settings.scope.unsupported")}</p>
      ) : (
        <>
          <p>
            {phase === "declined"
              ? t("settings.scope.declined")
              : phase === "failed"
                ? t("settings.scope.failed")
                : t("settings.scope.needed", { what })}
          </p>
          {phase === "checking" ? null : (
            <Button variant="primary" disabled={phase === "asking"} onClick={() => void ask()}>
              {phase === "asking" ? t("settings.scope.asking") : t("settings.scope.ask")}
            </Button>
          )}
        </>
      )}
    </div>
  );
}
