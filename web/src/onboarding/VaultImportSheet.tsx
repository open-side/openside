import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { ChevronDown, ChevronLeft, ChevronRight, KeyRound } from "lucide-react";
import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { ApiError } from "../api/client";
import type { ImportProfile, VaultStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { t } from "../i18n";
import { ErrorNote, Loading } from "../pages/blocks";
import { ScopeNotice } from "../pages/ScopeNotice";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { readCsv } from "../settings/pages/passwordsData";
import { Button, ICON_STROKE, Spinner } from "../ui";
import { PrimaryButton } from "./Frame";
import { POLL_MS } from "./ImportStep";
import { brandOf, outcomeNotes, profileTitle } from "./importModel";
import { passwordSources, sourceForProfile, type PasswordSource } from "./passwordSources";
import { VaultGate } from "./VaultGate";
import "./import.css";

type Choice = { kind: "profile"; profile: ImportProfile } | { kind: "source"; source: PasswordSource };

type View = "pick" | "method" | "export" | "direct" | "vault";

type Detected = { state: "loading" } | { state: "ready"; profiles: ImportProfile[] } | { state: "failed"; message: string };

/** The vault view remembers where it came from; the import it interrupted waits in a ref, so unlocking carries on with it. */
type VaultStop = { status: VaultStatus; back: View };

interface Action {
  label: string;
  run: () => void;
  disabled?: boolean;
  busy?: boolean;
}

/** Every import endpoint is local-only and admin-scoped; either refusal replaces the sheet's body with the notice that resolves it. */
function refusal(cause: unknown): "local" | "forbidden" | null {
  if (!(cause instanceof ApiError)) return null;
  if (cause.code === "local_access_required") return "local";
  return cause.status === 403 ? "forbidden" : null;
}

function sleep(ms: number) {
  const pause = Promise.withResolvers<void>();
  setTimeout(pause.resolve, ms);
  return pause.promise;
}

function Tile({ name, tint }: { name: string; tint: string }) {
  return (
    <span className="onb-brand-tile" style={{ background: tint }} aria-hidden="true">
      {name.slice(0, 1).toUpperCase()}
    </span>
  );
}

function sameChoice(current: Choice | null, other: Choice) {
  if (current?.kind === "profile" && other.kind === "profile") return current.profile.id === other.profile.id;
  if (current?.kind === "source" && other.kind === "source") return current.source.id === other.source.id;
  return false;
}

function SourceRow({ choice, current, name, tint, onSelect }: { choice: Choice; current: Choice | null; name: string; tint: string; onSelect: (choice: Choice) => void }) {
  const checked = sameChoice(current, choice);
  return (
    <label className="onb-sheet-row" data-checked={checked || undefined}>
      <input className="sr-only" type="radio" name="onb-vault-source" checked={checked} onChange={() => onSelect(choice)} />
      <Tile name={name} tint={tint} />
      <span className="onb-sheet-row-name">{name}</span>
    </label>
  );
}

/** Aside's export illustration redrawn: the exporting app's menu bar, its menu open, and the item to choose highlighted. */
function MenuIllustration({ source }: { source: PasswordSource }) {
  const [top, ...items] = source.menu;
  return (
    <div className="onb-sheet-art" aria-hidden="true">
      <div className="onb-sheet-menubar">
        <strong>{source.name}</strong>
        <span data-active="">{top}</span>
      </div>
      <ul className="onb-sheet-menu">
        {items.map((item, index) => (
          <li key={item} data-active={index === items.length - 1 || undefined}>
            <span>{item}</span>
            {index < items.length - 1 ? <ChevronRight size={12} strokeWidth={ICON_STROKE} /> : null}
          </li>
        ))}
      </ul>
    </div>
  );
}

function MethodOption({ checked, onSelect, title, detail, note }: { checked: boolean; onSelect: () => void; title: string; detail: string; note: string }) {
  return (
    <label className="onb-sheet-option" data-checked={checked || undefined}>
      <span className="onb-sheet-option-copy">
        <strong>{title}</strong>
        <span>{detail}</span>
        <small>{note}</small>
      </span>
      <input type="radio" name="onb-vault-method" checked={checked} onChange={onSelect} />
    </label>
  );
}

function SheetBody({ onClose, onImported, onBusy }: { onClose: () => void; onImported: (count: number, note: string | null) => void; onBusy: (busy: boolean) => void }) {
  const client = useClient();
  const alive = useRef(true);
  // Set synchronously, so a second click or Enter that lands before React re-renders cannot start another import.
  const running = useRef(false);
  // The CSV is plaintext passwords: it lives only here, never in state or a closure, and only while a vault stop may retry it.
  const csvText = useRef<string | null>(null);
  const pendingRetry = useRef<(() => Promise<void>) | null>(null);
  const picker = useRef<HTMLInputElement>(null);
  const detectedLabel = useId();
  const [detected, setDetected] = useState<Detected>({ state: "loading" });
  const [refused, setRefused] = useState<"local" | "forbidden" | null>(null);
  const [sources] = useState(passwordSources);
  const [view, setView] = useState<View>("pick");
  const [choice, setChoice] = useState<Choice | null>(null);
  const [others, setOthers] = useState(false);
  const [method, setMethod] = useState<"direct" | "csv">("direct");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [vault, setVault] = useState<VaultStop | null>(null);
  // VaultGate's own set-up or unlock request; leaving mid-request would drop the import it is about to resume.
  const [gateBusy, setGateBusy] = useState(false);
  const blocked = busy || gateBusy;

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      csvText.current = null;
      pendingRetry.current = null;
    };
  }, []);

  useEffect(() => onBusy(blocked), [blocked, onBusy]);

  const detect = useCallback(async () => {
    setDetected({ state: "loading" });
    setRefused(null);
    try {
      const list = await client.importProfiles();
      if (!alive.current) return;
      const profiles = list.profiles.filter((profile) => profile.available.passwords);
      setDetected({ state: "ready", profiles });
      // With no browser to offer, the managers are the only way forward, so they start open.
      if (profiles.length === 0) setOthers(true);
    } catch (cause) {
      if (!alive.current) return;
      const kind = refusal(cause);
      if (kind) setRefused(kind);
      else {
        setDetected({ state: "failed", message: errorMessage(cause) });
        setOthers(true);
      }
    }
  }, [client]);

  useEffect(() => {
    void detect();
  }, [detect]);

  const name = choice ? (choice.kind === "profile" ? brandOf(choice.profile).label : choice.source.name) : "";
  const crumb = choice ? (choice.kind === "profile" ? profileTitle(choice.profile) : choice.source.name) : "";
  const csvSource = choice ? (choice.kind === "source" ? choice.source : sourceForProfile(choice.profile)) : null;

  function finish(count: number, note: string | null) {
    onBusy(false);
    onImported(count, note);
  }

  function begin() {
    // A vault request that settles after the sheet unmounted must not start an import nobody sees.
    if (running.current || !alive.current) return false;
    running.current = true;
    setBusy(true);
    setError(null);
    return true;
  }

  function end() {
    running.current = false;
    if (alive.current) setBusy(false);
  }

  /** Resolves true when the failure parked the import at the vault view, where `retry` waits for set-up or unlocking. */
  async function fail(cause: unknown, back: View, retry: () => Promise<void>): Promise<boolean> {
    if (cause instanceof ApiError && (cause.code === "vault_setup_required" || cause.code === "vault_locked")) {
      const status = await client.vaultStatus().catch(() => null);
      if (!alive.current) return false;
      if (status) {
        pendingRetry.current = retry;
        setVault({ status, back });
        setView("vault");
        return true;
      }
    }
    const kind = refusal(cause);
    if (kind) setRefused(kind);
    else setError(errorMessage(cause));
    return false;
  }

  async function importDirect(profile: ImportProfile) {
    if (!begin()) return;
    setView("direct");
    try {
      let job = await client.runImport({ profiles: [profile.id], what: ["passwords"] });
      while (job.state === "running") {
        await sleep(POLL_MS);
        if (!alive.current) return;
        job = await client.importJob(job.id);
      }
      if (!alive.current) return;
      if (job.state === "failed" || !job.summary) {
        // A failed job reports a stable code; the vault codes route to the vault view like a refused request.
        if (job.error) throw new ApiError(500, job.error);
        setView("method");
        setError(t("onboarding.import.jobFailed"));
        return;
      }
      finish(job.summary.totals.passwords ?? 0, outcomeNotes(job.summary).join(" ") || null);
    } catch (cause) {
      if (!alive.current) return;
      setView("method");
      await fail(cause, "method", () => importDirect(profile));
    } finally {
      end();
    }
  }

  // The file is read once; a vault stop retries with the text held in `csvText` instead of asking for the file again.
  async function postCsv() {
    const csv = csvText.current;
    if (csv === null || !alive.current) return;
    setView("export");
    try {
      const result = await client.importPasswordsCsv(csv);
      csvText.current = null;
      if (alive.current) finish(result.imported, result.note ?? null);
    } catch (cause) {
      if (!alive.current) return;
      if (!(await fail(cause, "export", retryCsv))) csvText.current = null;
    }
  }

  async function retryCsv() {
    if (!begin()) return;
    try {
      await postCsv();
    } finally {
      end();
    }
  }

  async function chooseFile(file: File) {
    if (!begin()) return;
    try {
      let csv: string;
      try {
        csv = await readCsv(file);
      } catch {
        if (alive.current) setError(t("onboarding.sheet.readFailed"));
        return;
      }
      // A read that finishes after the sheet closed must not leave the text behind the unmount's cleanup.
      if (!alive.current) return;
      csvText.current = csv;
      await postCsv();
    } finally {
      end();
    }
  }

  function select(next: Choice) {
    setChoice(next);
    setError(null);
  }

  function go(next: View) {
    setError(null);
    setView(next);
  }

  // A running direct import has no way back: the daemon finishes it either way.
  const back: (() => void) | null =
    view === "method"
      ? () => go("pick")
      : view === "export"
        ? () => go(choice?.kind === "profile" ? "method" : "pick")
        : view === "vault" && vault
          ? () => {
              // Going back abandons the interrupted import, and with it any CSV text it held.
              pendingRetry.current = null;
              csvText.current = null;
              go(vault.back);
              setVault(null);
            }
          : null;

  let primary: Action | null = null;
  if (refused === null && view === "pick") {
    primary = { label: t("common.next"), disabled: choice === null, run: () => choice && go(choice.kind === "profile" ? "method" : "export") };
  } else if (refused === null && view === "method" && choice?.kind === "profile") {
    const profile = choice.profile;
    primary = { label: t("common.next"), disabled: busy, busy, run: () => (method === "direct" ? void importDirect(profile) : go("export")) };
  } else if (refused === null && view === "export") {
    primary = { label: busy ? t("onboarding.sheet.importing") : t("onboarding.sheet.chooseFile"), disabled: busy, busy, run: () => picker.current?.click() };
  }

  // Aside's ↵ glyph means Enter moves on; controls that use Enter themselves (buttons, fields, the vault form) keep it.
  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key !== "Enter" || event.shiftKey || event.altKey || event.metaKey || event.ctrlKey || event.nativeEvent.isComposing) return;
    if ((event.target as Element).closest("button, a, textarea, summary, form, input:not([type='radio'])")) return;
    if (running.current || !primary || primary.disabled) return;
    event.preventDefault();
    primary.run();
  }

  let title: string;
  let lead: ReactNode = null;
  let center = "";
  let body: ReactNode;
  if (view === "method" && choice?.kind === "profile") {
    title = t("onboarding.sheet.methodTitle", { name });
    lead = t("onboarding.sheet.methodLead", { name });
    center = crumb;
    body = (
      <div className="onb-sheet-options" role="radiogroup" aria-label={title}>
        <MethodOption checked={method === "direct"} onSelect={() => setMethod("direct")} title={t("onboarding.sheet.direct")} detail={t("onboarding.sheet.directDetail", { name })} note={t("onboarding.sheet.directNote")} />
        <MethodOption checked={method === "csv"} onSelect={() => setMethod("csv")} title={t("onboarding.sheet.csv")} detail={t("onboarding.sheet.csvDetail", { name })} note={t("onboarding.sheet.csvNote")} />
      </div>
    );
  } else if (view === "export" && csvSource) {
    title = t("onboarding.sheet.exportTitle", { name: csvSource.name });
    lead = t(csvSource.steps, { name: csvSource.name });
    center = t("onboarding.sheet.csvCrumb", { name: crumb });
    body = (
      <>
        <MenuIllustration source={csvSource} />
        <input
          ref={picker}
          type="file"
          accept=".csv,text/csv"
          hidden
          aria-label={t("onboarding.sheet.fileLabel")}
          onChange={(event) => {
            const file = event.target.files?.[0];
            // Reset so choosing the same file again still fires a change.
            event.target.value = "";
            if (file) void chooseFile(file);
          }}
        />
      </>
    );
  } else if (view === "direct") {
    title = t("onboarding.sheet.directTitle", { name });
    lead = t("onboarding.sheet.directNote");
    center = t("onboarding.sheet.directCrumb", { name: crumb });
    body = <Loading label={t("onboarding.sheet.importing")} />;
  } else if (view === "vault" && vault) {
    const { state, backend } = vault.status;
    title = state === "unavailable" || backend === null ? t("onboarding.vault.unavailableTitle") : state === "uninitialized" ? t("onboarding.vault.setupTitle") : t("onboarding.vault.unlockTitle");
    center = crumb;
    body = (
      <VaultGate
        bare
        status={vault.status}
        onBusy={setGateBusy}
        onReady={() => {
          if (!alive.current) return;
          // Taken once: a second ready signal must not start the interrupted import again.
          const retry = pendingRetry.current;
          pendingRetry.current = null;
          setVault(null);
          if (retry) void retry();
        }}
      />
    );
  } else {
    title = t("onboarding.sheet.pickTitle");
    lead = t("onboarding.sheet.pickLead");
    const profiles = detected.state === "ready" ? detected.profiles : [];
    body = (
      <div className="onb-sheet-pick">
        <p className="onb-sheet-label" id={detectedLabel}>
          {t("onboarding.sheet.detected")}
        </p>
        {detected.state === "loading" ? (
          <Loading label={t("onboarding.import.looking")} />
        ) : detected.state === "failed" ? (
          <ErrorNote>{detected.message}</ErrorNote>
        ) : profiles.length === 0 ? (
          <p className="onb-note">{t("onboarding.sheet.noneDetected")}</p>
        ) : (
          <div className="onb-sheet-rows" role="radiogroup" aria-labelledby={detectedLabel}>
            {profiles.map((profile) => (
              <SourceRow key={profile.id} choice={{ kind: "profile", profile }} current={choice} name={profileTitle(profile)} tint={brandOf(profile).tint} onSelect={select} />
            ))}
          </div>
        )}
        <button type="button" className="onb-sheet-disclosure" aria-expanded={others} onClick={() => setOthers((value) => !value)}>
          {t("onboarding.sheet.others")}
          <ChevronDown size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
        {others ? (
          <div className="onb-sheet-rows" role="radiogroup" aria-label={t("onboarding.sheet.others")}>
            {sources.map((source) => (
              <SourceRow key={source.id} choice={{ kind: "source", source }} current={choice} name={source.name} tint={source.tint} onSelect={select} />
            ))}
          </div>
        ) : null}
      </div>
    );
  }

  if (refused) {
    body =
      refused === "local" ? (
        <LocalOnlyNotice what={t("onboarding.sheet.localOnly")} onChanged={() => void detect()} />
      ) : (
        <ScopeNotice what={t("onboarding.sheet.scope")} onGranted={() => void detect()} />
      );
  }

  const notNow =
    view === "pick" || view === "vault" || refused ? (
      <Button className="onb-sheet-quiet" disabled={blocked} onClick={onClose}>
        {t("onboarding.sheet.notNow")}
      </Button>
    ) : null;

  return (
    <div className="onb-sheet-frame" onKeyDown={onKeyDown}>
      <header className="onb-sheet-top">
        <span className="onb-sheet-crumb">
          <KeyRound size={13} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {t("onboarding.sheet.vault")}
        </span>
        {center ? <span className="onb-sheet-path">{center}</span> : null}
      </header>
      <div className="onb-sheet-body thin-scroll" data-view={view}>
        <div className="onb-sheet-head">
          <BaseDialog.Title className="onb-sheet-title">{title}</BaseDialog.Title>
          {lead ? <BaseDialog.Description className="onb-sheet-lead">{lead}</BaseDialog.Description> : null}
        </div>
        {body}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
      </div>
      <footer className="onb-sheet-footer">
        {back && !refused ? (
          <Button className="onb-sheet-quiet onb-sheet-back" disabled={blocked} onClick={back}>
            <ChevronLeft size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
            {t("common.back")}
          </Button>
        ) : null}
        <span className="onb-sheet-end">
          {notNow}
          {primary ? (
            <PrimaryButton disabled={primary.disabled} onClick={primary.run}>
              {primary.busy ? <Spinner size={14} /> : null}
              {primary.label}
            </PrimaryButton>
          ) : null}
        </span>
      </footer>
    </div>
  );
}

/**
 * The passwords step's import sheet, drawn after Aside's vault sheet: pick a detected browser or a
 * password manager, then import directly or from an exported CSV file. Set-up or unlocking of the
 * vault happens inside the sheet, and the interrupted import carries on afterwards. `container` is
 * the guide window: like Aside's window-modal sheet, only that window is dimmed and blurred.
 */
export function VaultImportSheet({ open, onClose, onImported, container }: { open: boolean; onClose: () => void; onImported: (count: number, note: string | null) => void; container: HTMLElement | null }) {
  // A running import is not abandoned by Escape or an outside click; the daemon would finish it unseen.
  const [busy, setBusy] = useState(false);
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next && !busy) onClose();
      }}
    >
      <BaseDialog.Portal className="onb-sheet-portal" container={container}>
        <BaseDialog.Backdrop className="onb-sheet-backdrop" />
        <BaseDialog.Viewport className="onb-sheet-viewport">
          <BaseDialog.Popup className="onb-sheet surface-overlay">
            <SheetBody onClose={onClose} onImported={onImported} onBusy={setBusy} />
          </BaseDialog.Popup>
        </BaseDialog.Viewport>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
