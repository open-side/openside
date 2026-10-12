import { useEffect, useState } from "react";
import type { ApiError, Client } from "../api/client";
import { openSessionStream, type StreamStatus } from "../api/sse";
import { emptySessionView, reduceSession, type SessionView } from "../state/session";

export interface SessionViewHandle {
  view: SessionView;
  connection: StreamStatus;
  error: ApiError | null;
}

/** Live session view rebuilt from the full event log, then followed with resume. */
export function useSessionView(client: Client, sessionId: string): SessionViewHandle {
  const [view, setView] = useState<SessionView>(emptySessionView);
  const [connection, setConnection] = useState<StreamStatus>("connecting");
  const [error, setError] = useState<ApiError | null>(null);

  useEffect(() => {
    setView(emptySessionView);
    setError(null);
    const stream = openSessionStream({
      client,
      sessionId,
      onEvent: (event) => setView((current) => reduceSession(current, event)),
      onStatus: setConnection,
      onError: setError,
    });
    return () => stream.close();
  }, [client, sessionId]);

  return { view, connection, error };
}
