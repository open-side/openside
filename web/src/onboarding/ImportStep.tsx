import { Check, Globe } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ApiError } from "../api/client";
import type { ImportBatchReport, ImportCategory, ImportJob, ImportProfileList, VaultStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { EmptyState, ErrorNote, Loading } from "../pages/blocks";
import { ScopeNotice } from "../pages/ScopeNotice";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { Button, cx, ICON_STROKE, Spinner } from "../ui";
import {
  ALL_KINDS,
  brandOf,
  describeCounts,
  formatCount,
  KINDS,
  outcomeNotes,
  phrase,
  profileCounts,
  profileTitle,
  progressOf,
  requestFor,
  sumCounts,
  totalOf,
  type Counts,
} from "./importModel";
import { pairExtension } from "../desktopNotify";
import { deliverImport, requestImportPermissions } from "../import/delivery";
import type { StepProps } from "./summary";
import { VaultGate } from "./VaultGate";

export const POLL_MS = 700;

const EVERYTHING = { profiles: null, what: null };

type Load =
  | { state: "loading" }
  | { state: "ready"; list: ImportProfileList }
  | { state: "local" }
  | { state: "forbidden" }
  | { state: "failed"; message: string };

type Preview = { state: "loading" } | { state: "ready"; report: ImportBatchReport } | { state: "failed" };

type Run = { state: "idle" } | { state: "running"; job: ImportJob } | { state: "done"; report: ImportBatchReport; profiles: number };

function BrandTile({ profile }: { profile: Parameters<typeof brandOf>[0] }) {
  const brand = brandOf(profile);
  return (
    <span className="onb-brand-tile" style={{ background: brand.tint }} aria-hidden="true">
      {brand.initial}
    </span>
  );
}

function Outcome({ report }: { report: ImportBatchReport }) {
  const totals: Counts = {};
  for (const id of ALL_KINDS) if (report.totals[id]) totals[id] = report.totals[id];
  const notes = outcomeNotes(report);
  return (
    <>
      <ul className="onb-counts" aria-label="Imported">
        {KINDS.filter(({ id }) => totals[id]).map(({ id, label }) => (
          <li key={id}>
            <span>{label}</span>
            <span className="onb-counts-value">{formatCount(totals[id]!)}</span>
          </li>
        ))}
        {totalOf(totals) === 0 ? <li className="onb-counts-value">Nothing new was found to bring over.</li> : null}
      </ul>
      {notes.length ? (
        <ul className="onb-notes" aria-label="Notes">
          {notes.map((note) => (
            <li key={note}>{note}</li>
          ))}
        </ul>
      ) : null}
    </>
  );
}

type DeliveryState = { state: "idle" } | { state: "working"; done: number; total: number } | { state: "done"; delivered: number; pending: number; extensionMissing: boolean; skipped?: number } | { state: "failed"; message: string; action: "permission" | "pairing" | null };

/** Bookmarks, history and cookies reach this browser through the openside extension; a missing extension is a note, not an error. */
function Delivery({ state, onGrant, onPair }: { state: DeliveryState; onGrant: () => void; onPair: () => void }) {
  if (state.state === "idle") return null;
  if (state.state === "working") {
    return (
      <p className="onb-note" role="status">
        <Spinner size={14} /> Putting your bookmarks and history in this browser…{state.total > 0 ? ` ${state.done} of ${state.total}` : ""}
      </p>
    );
  }
  if (state.state === "failed") {
    return (
      <>
        <ErrorNote>{state.message}</ErrorNote>
        {state.action === "permission" ? (
          <Button variant="secondary" onClick={onGrant}>
            Sign in to your sites in openside
          </Button>
        ) : null}
        {state.action === "pairing" ? (
          <Button variant="secondary" onClick={onPair}>
            Connect the openside extension
          </Button>
        ) : null}
      </>
    );
  }
  if (state.extensionMissing) {
    return <p className="onb-note">Install the openside extension in the browser you use to bring your bookmarks and history into it. Everything else is already imported.</p>;
  }
  if (state.delivered === 0 && state.pending === 0) return null;
  if (state.pending > 0) return <p className="onb-note">Some of it is still waiting for this browser; it is added the next time you open openside here.</p>;
  return (
    <p className="onb-note">
      Your bookmarks and history are in this browser.
      {state.skipped ? ` ${state.skipped} ${state.skipped === 1 ? "link was" : "links were"} skipped.` : ""}
    </p>
  );
}

function deliveryAction(cause: unknown): "permission" | "pairing" | null {
  if (!(cause instanceof ApiError)) return null;
  if (cause.code === "import_delivery_permission_required") return "permission";
  return cause.code === "import_delivery_pairing_required" ? "pairing" : null;
}

const LIST_SHOWN = 8;

/** One collapsed list of what was brought over, for the kinds that are more than a number. */
function Listing({ title, rows }: { title: string; rows: { key: string; node: ReactNode }[] }) {
  const [all, setAll] = useState(false);
  if (rows.length === 0) return null;
  const shown = all ? rows : rows.slice(0, LIST_SHOWN);
  return (
    <details className="onb-kinds onb-listing">
      <summary>
        {title}
        <span className="onb-kinds-count">{formatCount(rows.length)}</span>
      </summary>
      <ul aria-label={title}>
        {shown.map((row) => (
          <li key={row.key}>{row.node}</li>
        ))}
      </ul>
      {rows.length > LIST_SHOWN && !all ? (
        <Button size="compact" variant="ghost" onClick={() => setAll(true)}>
          Show all {rows.length}
        </Button>
      ) : null}
    </details>
  );
}

/** Extensions are names and store links only: openside never installs them. */
function ImportDetails({ report }: { report: ImportBatchReport }) {
  return (
    <>
      <Listing
        title="Extensions you had"
        rows={(report.extensions ?? []).map((extension) => ({
          key: `${extension.profile_id}|${extension.id}`,
          node: extension.store_url ? (
            <a href={extension.store_url} target="_blank" rel="noopener noreferrer">
              {extension.name}
            </a>
          ) : (
            extension.name
          ),
        }))}
      />
      <Listing
        title="Search engines"
        rows={(report.search_engines ?? []).map((engine) => ({ key: `${engine.profile_id}|${engine.keyword}|${engine.name}`, node: `${engine.name} · ${engine.keyword}` }))}
      />
      <Listing
        title="Site permissions"
        rows={(report.permissions ?? []).map((entry) => ({ key: `${entry.profile_id}|${entry.origin}|${entry.permission}`, node: `${entry.origin} · ${entry.permission} · ${entry.setting}` }))}
      />
    </>
  );
}

const TABS_SHOWN = 8;

/** Tabs that were open in the source browser. They are only listed: nothing opens until the person clicks. */
function RestoreTabs({ tabs }: { tabs: NonNullable<ImportBatchReport["restore_tabs"]> }) {
  const [all, setAll] = useState(false);
  if (tabs.length === 0) return null;
  const shown = all ? tabs : tabs.slice(0, TABS_SHOWN);
  return (
    <div className="onb-restore">
      <p className="onb-result-title">Restore your tabs</p>
      <ul aria-label="Tabs to restore">
        {shown.map((tab) => (
          <li key={`${tab.profile_id}|${tab.url}`}>
            <a href={tab.url} target="_blank" rel="noopener noreferrer">
              {tab.title || tab.url}
            </a>
          </li>
        ))}
      </ul>
      {tabs.length > TABS_SHOWN && !all ? (
        <Button size="compact" variant="ghost" onClick={() => setAll(true)}>
          Show all {tabs.length}
        </Button>
      ) : null}
    </div>
  );
}

export function ImportStep({ update }: StepProps) {
  const client = useClient();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [preview, setPreview] = useState<Preview>({ state: "loading" });
  const [chosen, setChosen] = useState<ReadonlySet<string> | null>(null);
  const [kinds, setKinds] = useState<ReadonlySet<ImportCategory>>(new Set(ALL_KINDS));
  const [vault, setVault] = useState<VaultStatus | null>(null);
  const [run, setRun] = useState<Run>({ state: "idle" });
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<{ message: string; code: string | null } | null>(null);
  const alive = useRef(true);
  const [delivery, setDelivery] = useState<DeliveryState>({ state: "idle" });
  // The cookies need the person's explicit per-site approval, so a permission failure offers a button instead of retrying.
  const deliver = useCallback(
    (run: typeof deliverImport) => {
      setDelivery({ state: "working", done: 0, total: 0 });
      run(client, (done, total) => alive.current && setDelivery({ state: "working", done, total })).then(
        (result) => alive.current && setDelivery({ state: "done", ...result }),
        (cause) => alive.current && setDelivery({ state: "failed", message: errorMessage(cause), action: deliveryAction(cause) }),
      );
    },
    [client],
  );
  // An unpaired extension is paired with the same flow as notifications, then the batches are handed over.
  const pairAndDeliver = useCallback(async () => {
    setDelivery({ state: "working", done: 0, total: 0 });
    try {
      const reply = await pairExtension(client);
      if (!alive.current) return;
      if (reply === null) setDelivery({ state: "done", delivered: 0, pending: 1, extensionMissing: true });
      else if (!reply.ok) setDelivery({ state: "failed", message: reply.message ?? "The extension could not be connected. Reload it and try again.", action: "pairing" });
      else deliver(deliverImport);
    } catch (cause) {
      if (alive.current) setDelivery({ state: "failed", message: errorMessage(cause), action: "pairing" });
    }
  }, [client, deliver]);
  const picker = useRef<HTMLInputElement>(null);
  const [csv, setCsv] = useState<{ state: "idle" | "busy" } | { state: "done"; imported: number; note: string | null } | { state: "vault" }>({ state: "idle" });

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const detect = useCallback(async () => {
    setLoad({ state: "loading" });
    setPreview({ state: "loading" });
    try {
      const list = await client.importProfiles();
      if (!alive.current) return;
      setLoad({ state: "ready", list });
      setChosen(new Set(list.profiles.map((profile) => profile.id)));
      if (list.profiles.length === 0) return;
      client.previewImport(EVERYTHING).then(
        (report) => alive.current && setPreview({ state: "ready", report }),
        () => alive.current && setPreview({ state: "failed" }),
      );
    } catch (cause) {
      if (!alive.current) return;
      if (cause instanceof ApiError && cause.code === "local_access_required") setLoad({ state: "local" });
      else if (cause instanceof ApiError && cause.status === 403) setLoad({ state: "forbidden" });
      else setLoad({ state: "failed", message: errorMessage(cause) });
    }
  }, [client]);

  useEffect(() => {
    void detect();
  }, [detect]);

  const profiles = load.state === "ready" ? load.list.profiles : [];
  const selected = chosen ?? new Set<string>();
  const report = preview.state === "ready" ? preview.report : null;
  const counts = useMemo(
    () => new Map(profiles.map((profile) => [profile.id, profileCounts(report, profile.id)])),
    [profiles, report],
  );
  const picked = useMemo(
    () => sumCounts(profiles.filter((profile) => selected.has(profile.id)).map((profile) => counts.get(profile.id)!), kinds),
    [profiles, selected, counts, kinds],
  );
  const everything = profiles.length > 0 && profiles.every((profile) => selected.has(profile.id)) && ALL_KINDS.every((id) => kinds.has(id));
  const wantsPasswords = kinds.has("passwords") && (picked.passwords ?? 0) > 0;
  const vaultNeeded = wantsPasswords && report !== null && (report.vault_needed || report.vault_locked);

  useEffect(() => {
    if (!vaultNeeded) return;
    let cancelled = false;
    client.vaultStatus().then(
      (status) => !cancelled && setVault(status),
      () => undefined,
    );
    return () => {
      cancelled = true;
    };
  }, [client, vaultNeeded]);

  function toggleProfile(id: string) {
    setChosen((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
    setError(null);
  }

  function toggleKind(id: ImportCategory) {
    setKinds((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
    setError(null);
  }

  async function refreshAfterVault() {
    setVault(null);
    setPreview({ state: "loading" });
    try {
      const fresh = await client.previewImport(EVERYTHING);
      if (alive.current) setPreview({ state: "ready", report: fresh });
    } catch {
      if (alive.current) setPreview({ state: "failed" });
    }
  }

  const watch = useCallback(
    async (first: ImportJob, profileCount: number) => {
      let job = first;
      while (alive.current && job.state === "running") {
        const pause = Promise.withResolvers<void>();
        setTimeout(pause.resolve, POLL_MS);
        await pause.promise;
        if (!alive.current) return;
        try {
          job = await client.importJob(job.id);
        } catch (cause) {
          setRun({ state: "idle" });
          setError({ message: errorMessage(cause), code: cause instanceof ApiError ? cause.code : null });
          return;
        }
        if (job.state === "running") setRun({ state: "running", job });
      }
      if (!alive.current) return;
      if (job.state === "failed" || !job.summary) {
        setRun({ state: "idle" });
        // A batch keeps the profiles that finished, so this never claims that nothing was changed.
        setError({ message: job.error ? errorMessage(new ApiError(500, job.error)) : "The import did not finish. Data already imported is kept. Check the summary, then try again.", code: null });
        return;
      }
      setRun({ state: "done", report: job.summary, profiles: profileCount });
      deliver(deliverImport);
      const items = totalOf(job.summary.totals);
      update({ imported: `${profileCount} ${profileCount === 1 ? "profile" : "profiles"} · ${formatCount(items)} items` });
    },
    [client, update, deliver],
  );

  async function importCsv(file: File) {
    setCsv({ state: "busy" });
    setError(null);
    try {
      const result = await client.importPasswordsCsv(await file.text());
      setCsv({ state: "done", imported: result.imported, note: result.note ?? null });
    } catch (cause) {
      const code = cause instanceof ApiError ? cause.code : null;
      if (code === "vault_setup_required" || code === "vault_locked") {
        setVault(await client.vaultStatus().catch(() => null));
        setCsv({ state: "vault" });
      } else setCsv({ state: "idle" });
      setError({ message: errorMessage(cause), code });
    }
  }

  async function start() {
    setStarting(true);
    setError(null);
    try {
      const job = await client.runImport(requestFor(selected, profiles, kinds));
      setRun({ state: "running", job });
      await watch(job, selected.size);
    } catch (cause) {
      const code = cause instanceof ApiError ? cause.code : null;
      if (code === "vault_setup_required" || code === "vault_locked") {
        await refreshAfterVault().catch(() => undefined);
        setVault(await client.vaultStatus().catch(() => null));
      }
      setError({ message: errorMessage(cause), code });
    } finally {
      setStarting(false);
    }
  }

  if (load.state === "loading") return <Loading label="Looking for your browsers…" />;
  if (load.state === "local") return <LocalOnlyNotice what="import your browser data" onChanged={() => void detect()} />;
  if (load.state === "forbidden") return <ScopeNotice what="browser import" onGranted={() => void detect()} />;
  if (load.state === "failed") {
    return (
      <div className="onb-problem">
        <ErrorNote>{load.message}</ErrorNote>
        <Button size="compact" onClick={() => void detect()}>
          Try again
        </Button>
      </div>
    );
  }
  if (profiles.length === 0) {
    return (
      <EmptyState
        icon={Globe}
        title="No other browsers found"
        detail="openside looks for Chrome, Chromium, Brave, Edge, Vivaldi, Arc, Aside, Firefox and Safari in their usual places on this computer. Install or sign in to one and look again, or carry on and sign in to your sites later."
        action={
          <Button size="compact" onClick={() => void detect()}>
            Look again
          </Button>
        }
      />
    );
  }

  if (run.state === "done") {
    return (
      <div className="onb-result" role="status">
        <p className="onb-result-title">
          <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> Imported from {run.profiles} {run.profiles === 1 ? "profile" : "profiles"}
        </p>
        <Outcome report={run.report} />
        <ImportDetails report={run.report} />
        <Delivery state={delivery} onGrant={() => deliver(requestImportPermissions)} onPair={() => void pairAndDeliver()} />
        <RestoreTabs tabs={run.report.restore_tabs ?? []} />
      </div>
    );
  }

  if (run.state === "running") {
    const { done, total, ratio } = progressOf(run.job);
    const current = profiles.find((profile) => profile.id === run.job.current_profile);
    return (
      <div className="onb-result" role="status">
        <p className="onb-result-title">
          <Spinner size={16} /> Importing…
        </p>
        <div className="onb-meter" role="progressbar" aria-label="Import progress" aria-valuemin={0} aria-valuemax={total} aria-valuenow={done}>
          <span style={{ width: `${Math.max(ratio, 0.04) * 100}%` }} />
        </div>
        <p className="onb-note">
          {current ? `${profileTitle(current)} · ` : ""}
          {done} of {total} {total === 1 ? "profile" : "profiles"}
        </p>
      </div>
    );
  }

  const blocked = vaultNeeded && vault !== null;
  const nothingToImport = selected.size === 0 || kinds.size === 0;
  return (
    <>
      <ul className="onb-profiles" aria-label="Browsers found">
        {profiles.map((profile) => {
          const own = counts.get(profile.id)!;
          const description = describeCounts(own);
          return (
            <li key={profile.id}>
              <label className="onb-profile" data-checked={selected.has(profile.id) || undefined}>
                <input
                  type="checkbox"
                  checked={selected.has(profile.id)}
                  disabled={starting}
                  onChange={() => toggleProfile(profile.id)}
                  aria-label={`Import ${profileTitle(profile)}`}
                />
                <BrandTile profile={profile} />
                <span className="onb-profile-copy">
                  <strong>{profileTitle(profile)}</strong>
                  {profile.email ? <span>{profile.email}</span> : null}
                  <span className={cx("onb-profile-counts", description === "" && "onb-profile-counts--quiet")}>
                    {preview.state === "loading" ? "Counting…" : preview.state === "failed" ? "Could not count what is there" : description || "Nothing to bring over"}
                  </span>
                  {profile.warnings.map((warning) => (
                    <span key={warning} className="onb-profile-warning">
                      {warning}
                    </span>
                  ))}
                </span>
              </label>
            </li>
          );
        })}
      </ul>
      <details className="onb-kinds">
        <summary>Choose what to bring</summary>
        <div className="onb-kinds-grid">
          {KINDS.map(({ id, label }) => (
            <label key={id} className="onb-check">
              <input type="checkbox" checked={kinds.has(id)} disabled={starting} onChange={() => toggleKind(id)} />
              <span>{label}</span>
              {picked[id] ? <span className="onb-kinds-count">{formatCount(picked[id]!)}</span> : null}
            </label>
          ))}
        </div>
      </details>
      <p className="onb-note">
        {wantsPasswords ? "Passwords go into openside’s encrypted vault. " : ""}
        Everything is copied; your browsers are only read, never changed.
        {kinds.has("cookies") || wantsPasswords ? " Your computer may ask for access when cookies and passwords are read." : ""}
      </p>
      {blocked && vault ? (
        <>
          <VaultGate status={vault} onReady={() => void refreshAfterVault()} />
          <div className="onb-actions">
            <Button size="compact" variant="ghost" onClick={() => toggleKind("passwords")}>
              Import without passwords
            </Button>
          </div>
        </>
      ) : null}
      {error ? (
        <div className="onb-problem">
          <ErrorNote>{error.message}</ErrorNote>
          {error.code === "browser_unavailable" && kinds.has("cookies") ? (
            <Button size="compact" onClick={() => toggleKind("cookies")}>
              Leave cookies out
            </Button>
          ) : null}
          {error.code === "unknown_profile" ? (
            <Button size="compact" onClick={() => void detect()}>
              Look again
            </Button>
          ) : null}
        </div>
      ) : null}
      <div className="onb-actions">
        <Button variant="primary" disabled={starting || blocked || nothingToImport} onClick={() => void start()}>
          {starting ? (
            <>
              <Spinner size={16} /> Starting…
            </>
          ) : everything ? (
            "Import everything"
          ) : (
            "Import selected"
          )}
        </Button>
      </div>
      <div className="onb-csv">
        <input
          ref={picker}
          type="file"
          accept=".csv,text/csv"
          hidden
          aria-label="Passwords CSV file"
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            if (file) void importCsv(file);
          }}
        />
        <Button size="compact" variant="ghost" disabled={csv.state === "busy" || starting} onClick={() => picker.current?.click()}>
          {csv.state === "busy" ? "Reading file…" : "Import passwords from a CSV file"}
        </Button>
        {csv.state === "done" ? (
          <p className="onb-note" role="status">
            {csv.imported === 0 ? "No new passwords were found in that file." : `${phrase("passwords", csv.imported)} added to the vault.`} {csv.note ?? ""}
          </p>
        ) : csv.state === "vault" && vault ? (
          <VaultGate
            status={vault}
            onReady={() => {
              setVault(null);
              setCsv({ state: "idle" });
              setError(null);
            }}
          />
        ) : null}
      </div>
      {nothingToImport ? <p className="onb-note">{selected.size === 0 ? "Choose at least one browser." : "Choose at least one kind of data."}</p> : null}
      {everything && preview.state === "ready" ? (
        <p className="onb-note" aria-live="polite">
          {totalOf(picked) > 0 ? `About ${formatCount(totalOf(picked))} items: ${KINDS.filter(({ id }) => picked[id]).map(({ id }) => phrase(id, picked[id]!)).join(", ")}.` : "Nothing new was found."}
        </p>
      ) : null}
    </>
  );
}
