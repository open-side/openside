import { Check, Globe } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { ApiError } from "../api/client";
import type { ImportBatchReport, ImportCategory, ImportJob, ImportProfileList, VaultStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { formatNumber, t } from "../i18n";
import { EmptyState, ErrorNote, Loading } from "../pages/blocks";
import { ScopeNotice } from "../pages/ScopeNotice";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { Button, cx, ICON_STROKE, Spinner } from "../ui";
import { ALL_KINDS, brandOf, describeCounts, outcomeNotes, phrase, profileCounts, profileTitle, progressOf, requestFor, sumCounts, totalOf, type Counts } from "./importModel";
import { pairExtension } from "../desktopNotify";
import { deliverImport, requestImportPermissions } from "../import/delivery";
import { Frame, PrimaryButton } from "./Frame";
import type { StepNav, StepProps } from "./summary";
import { VaultGate } from "./VaultGate";
import "./import.css";

export const POLL_MS = 700;

const EVERYTHING = { profiles: null, what: null };
// Stable empties keep the memos below from recomputing while profiles are still loading.
const NO_PROFILES: ImportProfileList["profiles"] = [];
const NONE_CHOSEN: ReadonlySet<string> = new Set();

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
      <ul className="onb-counts" aria-label={t("onboarding.import.outcomeLabel")}>
        {ALL_KINDS.filter((id) => totals[id]).map((id) => (
          <li key={id}>
            <span>{t(`onboarding.kind.${id}`)}</span>
            <span className="onb-counts-value">{formatNumber(totals[id]!)}</span>
          </li>
        ))}
        {totalOf(totals) === 0 ? <li className="onb-counts-value">{t("onboarding.import.outcomeNone")}</li> : null}
      </ul>
      {notes.length ? (
        <ul className="onb-notes" aria-label={t("onboarding.import.notesLabel")}>
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
        <Spinner size={14} /> {state.total > 0 ? t("onboarding.delivery.workingProgress", { done: state.done, total: state.total }) : t("onboarding.delivery.working")}
      </p>
    );
  }
  if (state.state === "failed") {
    return (
      <>
        <ErrorNote>{state.message}</ErrorNote>
        {state.action === "permission" ? (
          <Button variant="secondary" onClick={onGrant}>
            {t("onboarding.delivery.grant")}
          </Button>
        ) : null}
        {state.action === "pairing" ? (
          <Button variant="secondary" onClick={onPair}>
            {t("onboarding.delivery.pair")}
          </Button>
        ) : null}
      </>
    );
  }
  if (state.extensionMissing) {
    return <p className="onb-note">{t("onboarding.delivery.extensionMissing")}</p>;
  }
  if (state.delivered === 0 && state.pending === 0) return null;
  if (state.pending > 0) return <p className="onb-note">{t("onboarding.delivery.pending")}</p>;
  return (
    <p className="onb-note">
      {t("onboarding.delivery.done")}
      {state.skipped ? ` ${t("onboarding.delivery.skipped", { count: state.skipped })}` : ""}
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
        <span className="onb-kinds-count">{formatNumber(rows.length)}</span>
      </summary>
      <ul aria-label={title}>
        {shown.map((row) => (
          <li key={row.key}>{row.node}</li>
        ))}
      </ul>
      {rows.length > LIST_SHOWN && !all ? (
        <Button variant="ghost" onClick={() => setAll(true)}>
          {t("onboarding.import.showAll", { count: rows.length })}
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
        title={t("onboarding.import.extensionsHad")}
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
        title={t("onboarding.kind.search_engines")}
        rows={(report.search_engines ?? []).map((engine) => ({ key: `${engine.profile_id}|${engine.keyword}|${engine.name}`, node: `${engine.name} · ${engine.keyword}` }))}
      />
      <Listing
        title={t("onboarding.kind.permissions")}
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
      <p className="onb-result-title">{t("onboarding.import.restoreTitle")}</p>
      <ul aria-label={t("onboarding.import.restoreLabel")}>
        {shown.map((tab) => (
          <li key={`${tab.profile_id}|${tab.url}`}>
            <a href={tab.url} target="_blank" rel="noopener noreferrer">
              {tab.title || tab.url}
            </a>
          </li>
        ))}
      </ul>
      {tabs.length > TABS_SHOWN && !all ? (
        <Button variant="ghost" onClick={() => setAll(true)}>
          {t("onboarding.import.showAll", { count: tabs.length })}
        </Button>
      ) : null}
    </div>
  );
}

/**
 * `only` starts with just those kinds ticked (Settings › Import bookmarks); the person can still add more.
 * `actions` moves the import button out of the body: `undefined` keeps it inline (Settings' dialog), an
 * element portals it there (the guide's footer), and `null` means that element is not mounted yet.
 */
export function ImportStep({ update, only, actions }: StepProps & { only?: readonly ImportCategory[]; actions?: HTMLElement | null }) {
  const client = useClient();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [preview, setPreview] = useState<Preview>({ state: "loading" });
  const [chosen, setChosen] = useState<ReadonlySet<string> | null>(null);
  const [kinds, setKinds] = useState<ReadonlySet<ImportCategory>>(() => new Set(only ?? ALL_KINDS));
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
      else if (!reply.ok) setDelivery({ state: "failed", message: reply.message ?? t("onboarding.delivery.pairFailed"), action: "pairing" });
      else deliver(deliverImport);
    } catch (cause) {
      if (alive.current) setDelivery({ state: "failed", message: errorMessage(cause), action: "pairing" });
    }
  }, [client, deliver]);

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

  const profiles = load.state === "ready" ? load.list.profiles : NO_PROFILES;
  const selected = chosen ?? NONE_CHOSEN;
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
        setError({ message: job.error ? errorMessage(new ApiError(500, job.error)) : t("onboarding.import.jobFailed"), code: null });
        return;
      }
      setRun({ state: "done", report: job.summary, profiles: profileCount });
      deliver(deliverImport);
      const items = totalOf(job.summary.totals);
      update({ imported: t("onboarding.import.recap", { count: profileCount, items }) });
    },
    [client, update, deliver],
  );

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

  if (load.state === "loading") return <Loading label={t("onboarding.import.looking")} />;
  if (load.state === "local") return <LocalOnlyNotice what={t("onboarding.import.localOnly")} onChanged={() => void detect()} />;
  if (load.state === "forbidden") return <ScopeNotice what={t("onboarding.import.scope")} onGranted={() => void detect()} />;
  if (load.state === "failed") {
    return (
      <div className="onb-problem">
        <ErrorNote>{load.message}</ErrorNote>
        <Button onClick={() => void detect()}>
          {t("common.tryAgain")}
        </Button>
      </div>
    );
  }
  if (profiles.length === 0) {
    return (
      <EmptyState
        icon={Globe}
        title={t("onboarding.import.noneTitle")}
        detail={t("onboarding.import.noneDetail")}
        action={
          <Button onClick={() => void detect()}>
            {t("onboarding.import.lookAgain")}
          </Button>
        }
      />
    );
  }

  if (run.state === "done") {
    return (
      <div className="onb-result" role="status">
        <p className="onb-result-title">
          <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> {t("onboarding.import.importedFrom", { count: run.profiles })}
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
          <Spinner size={16} /> {t("onboarding.import.importing")}
        </p>
        <div className="onb-meter" role="progressbar" aria-label={t("onboarding.import.progressLabel")} aria-valuemin={0} aria-valuemax={total} aria-valuenow={done}>
          <span style={{ width: `${Math.max(ratio, 0.04) * 100}%` }} />
        </div>
        <p className="onb-note">
          {current ? `${profileTitle(current)} · ` : ""}
          {t("onboarding.import.progress", { done, count: total })}
        </p>
      </div>
    );
  }

  const blocked = vaultNeeded && vault !== null;
  const nothingToImport = selected.size === 0 || kinds.size === 0;
  const disabled = starting || blocked || nothingToImport;
  const startButton =
    actions === undefined ? (
      <div className="onb-actions">
        <Button variant="primary" disabled={disabled} onClick={() => void start()}>
          {starting ? (
            <>
              <Spinner size={16} /> {t("onboarding.import.starting")}
            </>
          ) : everything ? (
            t("onboarding.import.everything")
          ) : (
            t("onboarding.import.selected")
          )}
        </Button>
      </div>
    ) : actions ? (
      createPortal(
        <PrimaryButton disabled={disabled} onClick={() => void start()}>
          {starting ? t("onboarding.import.starting") : t("onboarding.import.start")}
        </PrimaryButton>,
        actions,
      )
    ) : null;
  return (
    <>
      <ul className="onb-profiles" aria-label={t("onboarding.import.browsersFound")}>
        {profiles.map((profile) => {
          // Only the kinds that will be imported are counted, so the guide's row never promises passwords it leaves out.
          const description = describeCounts(sumCounts([counts.get(profile.id)!], kinds));
          return (
            <li key={profile.id}>
              <label className="onb-profile" data-checked={selected.has(profile.id) || undefined}>
                <BrandTile profile={profile} />
                <span className="onb-profile-copy">
                  <strong>{profileTitle(profile)}</strong>
                  {profile.email ? <span>{profile.email}</span> : null}
                  <span className={cx("onb-profile-counts", description === "" && "onb-profile-counts--quiet")}>
                    {preview.state === "loading" ? t("onboarding.import.counting") : preview.state === "failed" ? t("onboarding.import.countFailed") : description || t("onboarding.import.nothingInProfile")}
                  </span>
                  {profile.warnings.map((warning) => (
                    <span key={warning} className="onb-profile-warning">
                      {warning}
                    </span>
                  ))}
                </span>
                <input
                  type="checkbox"
                  checked={selected.has(profile.id)}
                  disabled={starting}
                  onChange={() => toggleProfile(profile.id)}
                  aria-label={t("onboarding.import.profileToggle", { name: profileTitle(profile) })}
                />
              </label>
            </li>
          );
        })}
      </ul>
      <details className="onb-kinds">
        <summary>{t("onboarding.import.chooseKinds")}</summary>
        <div className="onb-kinds-grid">
          {ALL_KINDS.map((id) => (
            <label key={id} className="onb-check">
              <input type="checkbox" checked={kinds.has(id)} disabled={starting} onChange={() => toggleKind(id)} />
              <span>{t(`onboarding.kind.${id}`)}</span>
              {picked[id] ? <span className="onb-kinds-count">{formatNumber(picked[id]!)}</span> : null}
            </label>
          ))}
        </div>
      </details>
      <p className="onb-note">
        {[wantsPasswords ? t("onboarding.import.passwordsVault") : null, t("onboarding.import.readOnly"), kinds.has("cookies") || wantsPasswords ? t("onboarding.import.osPrompt") : null].filter(Boolean).join(" ")}
      </p>
      {blocked && vault ? (
        <>
          <VaultGate status={vault} onReady={() => void refreshAfterVault()} />
          <div className="onb-actions">
            <Button variant="ghost" onClick={() => toggleKind("passwords")}>
              {t("onboarding.import.withoutPasswords")}
            </Button>
          </div>
        </>
      ) : null}
      {error ? (
        <div className="onb-problem">
          <ErrorNote>{error.message}</ErrorNote>
          {error.code === "browser_unavailable" && kinds.has("cookies") ? (
            <Button onClick={() => toggleKind("cookies")}>
              {t("onboarding.import.withoutCookies")}
            </Button>
          ) : null}
          {error.code === "unknown_profile" ? (
            <Button onClick={() => void detect()}>
              {t("onboarding.import.lookAgain")}
            </Button>
          ) : null}
        </div>
      ) : null}
      {startButton}
      {nothingToImport ? <p className="onb-note">{selected.size === 0 ? t("onboarding.import.pickBrowser") : t("onboarding.import.pickKind")}</p> : null}
      {everything && preview.state === "ready" ? (
        <p className="onb-note" aria-live="polite">
          {totalOf(picked) > 0
            ? t("onboarding.import.estimate", { count: totalOf(picked), list: ALL_KINDS.filter((id) => picked[id]).map((id) => phrase(id, picked[id]!)).join(", ") })
            : t("onboarding.import.nothingNew")}
        </p>
      ) : null}
    </>
  );
}

/** Passwords have their own step in the guide, so the browser import starts without them. */
const BROWSER_KINDS = ALL_KINDS.filter((id) => id !== "passwords");

/** The guide's "Import from another browser" step: ImportStep inside the frame, its button in the footer. */
export function BrowserImportStep({ next, back, summary, update }: StepNav) {
  const [slot, setSlot] = useState<HTMLDivElement | null>(null);
  const imported = summary.imported !== null;
  return (
    <Frame
      step="import"
      layout="centered"
      title={t("onboarding.import.title")}
      lead={t("onboarding.import.lead")}
      onBack={back}
      skip={imported ? null : { label: t("onboarding.frame.skip"), onClick: next }}
      primary={imported ? <PrimaryButton onClick={next}>{t("common.continue")}</PrimaryButton> : undefined}
      primarySlot={setSlot}
    >
      <div className="onb-import">
        {/* Once something was imported the footer says Continue; another import stays possible from the body. */}
        <ImportStep update={update} only={BROWSER_KINDS} actions={imported ? undefined : slot} />
      </div>
    </Frame>
  );
}
