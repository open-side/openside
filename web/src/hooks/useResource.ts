import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";

export function errorMessage(cause: unknown): string {
  return cause instanceof ApiError ? cause.message : "Request failed";
}

export interface Resource<T> {
  data: T | null;
  error: string | null;
  /** The last load was refused with 403: this device's token lacks the scope the screen needs. */
  forbidden: boolean;
  setError: (error: string | null) => void;
  reload: () => Promise<void>;
}

/** Loads `load` on mount and whenever its identity changes; pass a `useCallback` function. */
export function useResource<T>(load: () => Promise<T>): Resource<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [forbidden, setForbidden] = useState(false);

  const reload = useCallback(async () => {
    try {
      setData(await load());
      setError(null);
      setForbidden(false);
    } catch (cause) {
      setError(errorMessage(cause));
      setForbidden(cause instanceof ApiError && cause.status === 403);
    }
  }, [load]);

  useEffect(() => {
    let cancelled = false;
    load().then(
      (value) => {
        if (cancelled) return;
        setData(value);
        setError(null);
        setForbidden(false);
      },
      (cause: unknown) => {
        if (cancelled) return;
        setError(errorMessage(cause));
        setForbidden(cause instanceof ApiError && cause.status === 403);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [load]);

  return { data, error, forbidden, setError, reload };
}
