import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";

export function errorMessage(cause: unknown): string {
  return cause instanceof ApiError ? cause.message : "Request failed";
}

export interface Resource<T> {
  data: T | null;
  error: string | null;
  setError: (error: string | null) => void;
  reload: () => Promise<void>;
}

/** Loads `load` on mount and whenever its identity changes; pass a `useCallback` function. */
export function useResource<T>(load: () => Promise<T>): Resource<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setData(await load());
      setError(null);
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }, [load]);

  useEffect(() => {
    let cancelled = false;
    load().then(
      (value) => {
        if (cancelled) return;
        setData(value);
        setError(null);
      },
      (cause: unknown) => {
        if (!cancelled) setError(errorMessage(cause));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [load]);

  return { data, error, setError, reload };
}
