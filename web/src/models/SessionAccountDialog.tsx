import { useEffect, useState } from "react";
import type { ModelAccountRef, ModelCatalog, ProviderPoolList } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { ErrorNote } from "../pages/blocks";
import { Button, Dialog, DialogActions, useToast } from "../ui";
import { accountsOf, refKey } from "./accounts";
import { providerLabel } from "./providers";

/**
 * Prefer one account for a conversation's next requests. It is a preference, not a lock: the
 * daemon still moves to another account in the same rotation when this one hits a limit.
 */
export function SessionAccountDialog({ open, sessionId, onClose }: { open: boolean; sessionId: string; onClose: () => void }) {
  const client = useClient();
  const toast = useToast();
  const [data, setData] = useState<{ catalog: ModelCatalog; pools: ProviderPoolList } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setError(null);
    Promise.all([client.modelCatalog(), client.listPools()])
      .then(([catalog, pools]) => !cancelled && setData({ catalog, pools }))
      .catch((cause) => !cancelled && setError(errorMessage(cause)));
    return () => {
      cancelled = true;
    };
  }, [client, open]);

  async function choose(account: ModelAccountRef | null, label: string) {
    setBusy(true);
    setError(null);
    try {
      await client.setSessionAccount(sessionId, account);
      toast.notify(account ? `Next messages prefer ${label}.` : "Next messages pick an account automatically.");
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  const groups = (data?.catalog.providers ?? []).flatMap((provider) => {
    const pool = data?.pools.pools.find((candidate) => candidate.provider === provider.provider)?.accounts ?? [];
    const rows = accountsOf(provider, pool, [], []).filter((row) => row.poolIndex !== null);
    return rows.length ? [{ provider: provider.provider, rows }] : [];
  });

  return (
    <Dialog open={open} onClose={onClose} title="Account for this conversation" description="openside picks an account for each request. Prefer one to keep this conversation on it; it still switches when that account hits a limit.">
      <div className="dialog-form">
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        {data && groups.length === 0 ? <p className="settings-text">No provider has an account in its rotation yet.</p> : null}
        {groups.map((group) => (
          <section key={group.provider} aria-label={providerLabel(group.provider)}>
            <h4 className="rotation-title">{providerLabel(group.provider)}</h4>
            <ul className="session-accounts">
              {group.rows.map((row) => (
                <li key={refKey(row.ref)}>
                  <span>{row.label}</span>
                  <Button size="compact" disabled={busy || !row.ready} aria-label={`Prefer ${row.label}`} onClick={() => void choose(row.ref, row.label)}>
                    Prefer
                  </Button>
                </li>
              ))}
            </ul>
          </section>
        ))}
        <DialogActions>
          <Button disabled={busy} onClick={() => void choose(null, "")}>
            Pick automatically
          </Button>
          <Button variant="primary" onClick={onClose}>
            Close
          </Button>
        </DialogActions>
      </div>
    </Dialog>
  );
}
