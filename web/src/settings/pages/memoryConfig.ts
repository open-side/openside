import { useCallback, useEffect, useState } from "react";
import type { Retention, SettingsDocument } from "../../api/types";
import { useClient } from "../../context";
import { errorMessage } from "../../hooks/useResource";

/** The `[memory]` keys these pages change; both are open to a paired phone (`remote_editable`). */
export interface MemoryConfig {
  history?: boolean;
  default_retention?: Retention;
}

export interface MemoryConfigState {
  /** `[memory]` as stored, or null until the document loads. */
  memory: MemoryConfig | null;
  /** The stored config differs from what the running daemon loaded. */
  restartRequired: boolean;
  busy: boolean;
  error: string | null;
  set: <K extends keyof MemoryConfig>(key: K, value: NonNullable<MemoryConfig[K]>) => Promise<void>;
  reload: () => Promise<void>;
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

/**
 * Settings › Memory's config rows. `PUT /v1/settings/config` takes the whole document, so a
 * change sends back everything as loaded with only the one `[memory]` key replaced; the daemon
 * applies it on its next start.
 */
export function useMemoryConfig(): MemoryConfigState {
  const client = useClient();
  const [doc, setDoc] = useState<SettingsDocument | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setDoc(await client.settingsConfig());
      setError(null);
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }, [client]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const set = useCallback(
    async <K extends keyof MemoryConfig>(key: K, value: NonNullable<MemoryConfig[K]>) => {
      if (!doc) return;
      const config = record(doc.config);
      setBusy(true);
      setError(null);
      try {
        setDoc(await client.updateSettingsConfig({ ...config, memory: { ...record(config.memory), [key]: value } }));
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        setBusy(false);
      }
    },
    [client, doc],
  );

  return {
    memory: doc ? (record(record(doc.config).memory) as MemoryConfig) : null,
    restartRequired: doc?.restart_required ?? false,
    busy,
    error,
    set,
    reload,
  };
}
