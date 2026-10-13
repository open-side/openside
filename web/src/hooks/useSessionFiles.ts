import { useEffect, useState } from "react";
import type { Client } from "../api/client";
import { isOutputArtifact } from "../api/artifacts";
import type { Artifact, Visual } from "../api/types";
import type { SessionView } from "../state/session";

/** Signed preview URLs live 10 minutes; refresh a little earlier. */
const REFRESH_MS = 9 * 60 * 1000;

export interface SessionFiles {
  artifacts: Artifact[];
  /** Visual blocks keyed by the `assistant.final` sequence that contains them. */
  visuals: Record<number, Visual[]>;
}

/**
 * Artifact and visual listings with fresh signed preview URLs. The event log says *when*
 * something changed (`artifact.saved`, attachments, `assistant.final`); the listing routes
 * supply the preview URLs, which cannot be derived from events.
 */
export function useSessionFiles(client: Client, sessionId: string, view: SessionView): SessionFiles {
  const [files, setFiles] = useState<SessionFiles>({ artifacts: [], visuals: {} });
  const finals = view.items.filter((item) => item.type === "assistant" && item.finalSeq !== undefined).length;
  const attachments = view.items.reduce((count, item) => count + (item.type === "user" ? (item.attachments?.length ?? 0) : 0), 0);
  const saved = view.artifacts.map((artifact) => `${artifact.path}:${artifact.size}`).join("|");
  const [tick, setTick] = useState(0);

  useEffect(() => {
    const timer = setInterval(() => setTick((value) => value + 1), REFRESH_MS);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([client.listArtifacts(sessionId), client.listVisuals(sessionId)])
      .then(([artifacts, visuals]) => {
        if (cancelled) return;
        const bySeq: Record<number, Visual[]> = {};
        for (const visual of visuals.visuals) (bySeq[visual.seq] ??= []).push(visual);
        setFiles({ artifacts: artifacts.artifacts.filter(isOutputArtifact), visuals: bySeq });
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [client, sessionId, finals, attachments, saved, tick]);

  return files;
}
