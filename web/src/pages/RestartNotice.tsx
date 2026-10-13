import { Check, RotateCw } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { Button, ICON_STROKE, Spinner, useToast } from "../ui";
import { ErrorNote, useConfirm } from "./blocks";

export type RestartPhase = "idle" | "restarting" | "done";

const POLL_MS = 1_000;
// A slow disk or a service manager that waits for open connections can take a while; past this the
// computer needs a look rather than another second of spinner.
const GIVE_UP_MS = 90_000;

export interface DaemonRestart {
  phase: RestartPhase;
  error: string | null;
  /** The request needs the computer's own authority and this browser does not have it. */
  localOnly: boolean;
  /** Ask first; the restart stops running conversations, which resume from their saved history. */
  ask: () => void;
  /** The confirmation dialog; render it once next to the button. */
  dialog: ReactNode;
  retry: () => void;
}

/**
 * Restart the daemon and reconnect: the request answers at once, so the restart is finished when
 * `GET /v1/daemon` reports another `boot_id`. Requests fail while the process is down; that is
 * expected and only the deadline ends the wait.
 */
export function useDaemonRestart({ onRestarted, pollMs = POLL_MS }: { onRestarted?: () => void; pollMs?: number } = {}): DaemonRestart {
  const client = useClient();
  const toast = useToast();
  const [phase, setPhase] = useState<RestartPhase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [localOnly, setLocalOnly] = useState(false);
  const [confirm, ask] = useConfirm();
  const alive = useRef(true);
  const done = useRef(onRestarted);
  done.current = onRestarted;

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const run = useCallback(async () => {
    setPhase("restarting");
    setError(null);
    setLocalOnly(false);
    let previous: string;
    try {
      previous = (await client.restartDaemon()).boot_id;
    } catch (cause) {
      if (!alive.current) return;
      setPhase("idle");
      setLocalOnly(cause instanceof ApiError && cause.code === "local_access_required");
      setError(errorMessage(cause));
      return;
    }
    const deadline = Date.now() + GIVE_UP_MS;
    while (alive.current && Date.now() < deadline) {
      const wait = Promise.withResolvers<void>();
      setTimeout(wait.resolve, pollMs);
      await wait.promise;
      if (!alive.current) return;
      try {
        if ((await client.daemonStatus()).boot_id !== previous) {
          setPhase("done");
          toast.notify("openside restarted.");
          done.current?.();
          return;
        }
      } catch {
        // The daemon is down between the old process ending and the new one listening.
      }
    }
    if (alive.current) {
      setPhase("idle");
      setError("openside did not come back in time. Check the computer that runs it.");
    }
  }, [client, pollMs, toast]);

  const request = useCallback(
    () =>
      ask({
        title: "Restart openside?",
        description: "Conversations that are running stop and pick up again from their saved history. This page reconnects by itself.",
        confirmLabel: "Restart",
        onConfirm: () => void run(),
      }),
    [ask, run],
  );

  return { phase, error, localOnly, ask: request, dialog: confirm, retry: () => void run() };
}

/** Progress, result and errors of a restart started from anywhere. */
export function RestartStatus({ restart }: { restart: DaemonRestart }) {
  return (
    <>
      {restart.phase === "restarting" ? (
        <p className="restart-progress" role="status">
          <Spinner size={14} /> Restarting openside. This page reconnects when it is back.
        </p>
      ) : null}
      {restart.phase === "done" ? (
        <p className="restart-progress" role="status">
          <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> openside restarted.
        </p>
      ) : null}
      {restart.localOnly ? (
        <LocalOnlyNotice what="restart openside" onChanged={restart.retry} />
      ) : restart.error ? (
        <ErrorNote>{restart.error}</ErrorNote>
      ) : null}
      {restart.dialog}
    </>
  );
}

/**
 * "Saved. This applies after a restart." with the button that does it. `onRestarted` should reload
 * whatever told the person to restart, so the notice disappears once it no longer applies.
 */
export function RestartNotice({ children, onRestarted }: { children: ReactNode; onRestarted?: () => void }) {
  const restart = useDaemonRestart({ onRestarted });
  return (
    <div className="restart-notice">
      <div className="restart-notice-row" role="status">
        <p>{children}</p>
        {restart.phase === "idle" ? (
          <Button size="compact" onClick={restart.ask}>
            <RotateCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
            Restart daemon
          </Button>
        ) : null}
      </div>
      <RestartStatus restart={restart} />
    </div>
  );
}
