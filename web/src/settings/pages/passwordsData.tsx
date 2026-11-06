import { Copy, Download, FileUp } from "lucide-react";
import { useEffect, useRef, useState, type ChangeEvent, type FormEvent, type ReactNode } from "react";
import { ApiError } from "../../api/client";
import type { ImportCategoryReport, VaultBiometrics, VaultConfirmRequest, VaultRecoveryKey, VaultSettings, VaultStatus } from "../../api/types";
import { useClient } from "../../context";
import { t, tMaybe } from "../../i18n";
import { ErrorNote } from "../../pages/blocks";
import { FailureNote, toFailure, type Failure } from "../../pages/Failure";
import { formatDateTime } from "../../pages/format";
import { LocalOnlyNotice } from "../../remote/LocalOnlyNotice";
import { Button, copyText, Dialog, DialogActions, ICON_STROKE, InfoBanner, useToast } from "../../ui";
import { SECRET_INPUT } from "./passwordsVault";

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/**
 * What the daemon is shown as a fresh check: the passphrase, this computer's fingerprint, or only
 * the confirmation. "unavailable" is a vault that requires the fingerprint on a computer that
 * can't check one right now; offering a plain confirmation there would only be refused.
 */
type ReauthMode = "passphrase" | "biometric" | "unavailable" | "confirm";

function reauthMode(vault: VaultContext, biometrics: VaultBiometrics | null): ReauthMode | null {
  if (vault.status.backend === "passphrase") return "passphrase";
  if (biometrics === null || vault.settings === null) return null;
  if (!vault.settings.biometric_unlock) return "confirm";
  return biometrics.available ? "biometric" : "unavailable";
}

function biometricsUnavailable(biometrics: VaultBiometrics | null): string {
  const reason = biometrics?.reason;
  // Reason codes from a newer daemon fall back to the generic explanation.
  return (reason && tMaybe(`passwords.biometrics.reason.${reason}`)) || t("api.error.biometric_unavailable");
}

/** Everything the vault dialogs need to know about the vault and where this page runs. */
export interface VaultContext {
  status: VaultStatus;
  settings: VaultSettings | null;
  /** This page is on another device, so local-only actions are explained instead of offered. */
  remote: boolean;
}

function saveFile(name: string, text: string, type: string) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * Export, recovery key and clear ask for fresh authentication. A passphrase vault takes the
 * passphrase (optional after an unlock in the last five minutes); a keyring vault with biometric
 * unlock asks this computer's fingerprint check, and says why when the computer can't do one;
 * any other keyring vault needs only the confirmation. The passphrase leaves page state on every
 * attempt.
 */
function ReauthForm({
  vault,
  what,
  action,
  danger,
  onCancel,
  run,
  children,
}: {
  vault: VaultContext;
  what: string;
  action: string;
  danger?: boolean;
  onCancel: () => void;
  /** Sends the request; the returned step applies the answer and runs only if the dialog is still open. */
  run: (request: VaultConfirmRequest) => Promise<() => void>;
  children: ReactNode;
}) {
  const client = useClient();
  const [biometrics, setBiometrics] = useState<VaultBiometrics | null>(null);
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const keyring = vault.status.backend !== "passphrase";
  // The form unmounts when its dialog closes; an answer arriving after that must not download, show or toast anything.
  const open = useRef(false);
  useEffect(() => {
    open.current = true;
    return () => {
      open.current = false;
    };
  }, []);

  useEffect(() => {
    if (!keyring || vault.remote) return;
    let alive = true;
    client.vaultBiometrics().then(
      (next) => alive && setBiometrics(next),
      () => alive && setBiometrics({ available: false }),
    );
    return () => {
      alive = false;
    };
  }, [client, keyring, vault.remote]);

  const mode = reauthMode(vault, biometrics);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!mode || mode === "unavailable") return;
    const request: VaultConfirmRequest = { confirm: true, passphrase: mode === "passphrase" && passphrase ? passphrase : null, biometric: mode === "biometric" };
    setPassphrase("");
    setBusy(true);
    setFailure(null);
    try {
      const apply = await run(request);
      if (open.current) apply();
    } catch (cause) {
      if (!open.current) return;
      const needed: Partial<Record<ReauthMode, string>> = {
        passphrase: t("passwords.reauth.neededPassphrase"),
        biometric: t("passwords.reauth.neededBiometric"),
        unavailable: biometricsUnavailable(biometrics),
      };
      const code = cause instanceof ApiError ? cause.code : null;
      // The shared too_many_attempts text is about pairing; here it is wrong passphrases or fingerprint checks.
      const message = code === "reauth_required" ? needed[mode] : code === "too_many_attempts" ? t("passwords.reauth.tooManyAttempts") : undefined;
      setFailure(message ? { message, localOnly: false } : toFailure(cause));
    } finally {
      if (open.current) setBusy(false);
    }
  }

  if (vault.remote || failure?.localOnly) return <LocalOnlyNotice what={what} onChanged={() => setFailure(null)} />;

  return (
    <form className="passwords-form" onSubmit={(event) => void submit(event)} aria-label={action}>
      {children}
      {mode === "passphrase" ? (
        <label className="field">
          <span>{t("passwords.reauth.passphrase")}</span>
          <input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} {...SECRET_INPUT} />
          <small className="passwords-hint">{t("passwords.reauth.passphraseHint")}</small>
        </label>
      ) : mode === "biometric" ? (
        <p className="passwords-hint">{t("passwords.reauth.biometric")}</p>
      ) : mode === "unavailable" ? (
        <p className="personal-error" role="alert">
          {biometricsUnavailable(biometrics)}
        </p>
      ) : null}
      <FailureNote failure={failure} what={what} onRetry={() => setFailure(null)} />
      <DialogActions>
        <Button variant="ghost" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant={danger ? "danger" : "primary"} disabled={busy || mode === null || mode === "unavailable"}>
          {action}
        </Button>
      </DialogActions>
    </form>
  );
}

/** Export › the sign-ins as a CSV download, after a warning that the file is readable by anyone. */
export function ExportDialog({ open, onClose, vault }: { open: boolean; onClose: () => void; vault: VaultContext }) {
  const client = useClient();
  const toast = useToast();
  return (
    <Dialog open={open} onClose={onClose} title={t("passwords.export.dialogTitle")}>
      {open ? (
        <ReauthForm
          vault={vault}
          what={t("passwords.export.what")}
          action={t("passwords.export.action")}
          onCancel={onClose}
          run={async (request) => {
            // The CSV goes straight into the download; no state keeps a copy of it.
            const { filename, content, exported, skipped } = await client.exportVault(request);
            return () => {
              saveFile(filename, content, "text/csv");
              toast.notify(skipped ? t("passwords.export.skipped", { exported, count: skipped }) : t("passwords.export.done", { count: exported }), "success");
              onClose();
            };
          }}
        >
          <InfoBanner tone="info" title={t("passwords.export.warningTitle")}>
            {t("passwords.export.warning")}
          </InfoBanner>
        </ReauthForm>
      ) : null}
    </Dialog>
  );
}

function RecoveryKeyShown({ recovery, onDone }: { recovery: VaultRecoveryKey; onDone: () => void }) {
  const toast = useToast();
  return (
    <div className="passwords-form">
      <InfoBanner tone="info">{t("passwords.recoveryKey.once")}</InfoBanner>
      <code className="passwords-recovery-key" aria-label={t("passwords.recoveryKey.label")}>
        {recovery.recovery_key}
      </code>
      <span className="personal-actions">
        <Button
          onClick={() =>
            void copyText(recovery.recovery_key).then((copied) =>
              toast.notify(t(copied ? "passwords.recoveryKey.copied" : "passwords.recoveryKey.copyFailed"), copied ? "success" : "error"),
            )
          }
        >
          <Copy {...ICON} />
          {t("common.copy")}
        </Button>
        <Button
          onClick={() =>
            saveFile("openside-recovery-key.txt", `${t("passwords.recoveryKey.file", { date: formatDateTime(recovery.created_at) })}\n\n${recovery.recovery_key}\n`, "text/plain")
          }
        >
          <Download {...ICON} />
          {t("passwords.recoveryKey.download")}
        </Button>
      </span>
      <DialogActions>
        <Button variant="primary" onClick={onDone}>
          {t("common.done")}
        </Button>
      </DialogActions>
    </div>
  );
}

/**
 * Recovery key › create or replace it. The daemon keeps nothing it could show again, so the key
 * lives only in this dialog: opening or closing it discards the key, and a key that arrives after
 * the dialog closed is dropped (the form that asked for it is gone) rather than shown later.
 */
export function RecoveryKeyDialog({ open, onClose, vault, onCreated }: { open: boolean; onClose: () => void; vault: VaultContext; onCreated: () => void }) {
  const client = useClient();
  const [recovery, setRecovery] = useState<VaultRecoveryKey | null>(null);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    setRecovery(null);
  }
  const close = () => {
    setRecovery(null);
    onClose();
  };
  const replacing = vault.settings?.recovery.configured === true;
  return (
    <Dialog open={open} onClose={close} title={t("passwords.recoveryKey.title")}>
      {!open ? null : recovery ? (
        <RecoveryKeyShown recovery={recovery} onDone={close} />
      ) : (
        <ReauthForm
          vault={vault}
          what={t("passwords.recoveryKey.what")}
          action={t(replacing ? "passwords.recovery.regenerate" : "passwords.recoveryKey.generate")}
          onCancel={close}
          run={async (request) => {
            const created = await client.createVaultRecoveryKey(request);
            // The old key stopped working either way, so the row refreshes even if nobody sees the new one.
            onCreated();
            return () => setRecovery(created);
          }}
        >
          <p className="settings-text">{t(replacing ? "passwords.recoveryKey.replace" : "passwords.recoveryKey.create")}</p>
        </ReauthForm>
      )}
    </Dialog>
  );
}

/** Clear vault data › nothing is sent until the person presses Clear. */
export function ClearDialog({ open, onClose, vault, onCleared }: { open: boolean; onClose: () => void; vault: VaultContext; onCleared: () => void }) {
  const client = useClient();
  const toast = useToast();
  return (
    <Dialog open={open} onClose={onClose} title={t("passwords.clear.dialogTitle")}>
      {open ? (
        <ReauthForm
          vault={vault}
          what={t("passwords.clear.what")}
          action={t("passwords.clear.action")}
          danger
          onCancel={onClose}
          run={async (request) => {
            const { removed } = await client.clearVault(request);
            // The data is gone either way, so the page refreshes even after the dialog closed.
            onCleared();
            return () => {
              toast.notify(t("passwords.clear.done", { count: removed }), "success");
              onClose();
            };
          }}
        >
          <p className="settings-text">{t("passwords.clear.warning")}</p>
        </ReauthForm>
      ) : null}
    </Dialog>
  );
}

/** Browsers write UTF-8; some password managers (and Excel) save UTF-16 with a byte-order mark. */
export async function readCsv(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const encoding = bytes[0] === 0xff && bytes[1] === 0xfe ? "utf-16le" : bytes[0] === 0xfe && bytes[1] === 0xff ? "utf-16be" : "utf-8";
  return new TextDecoder(encoding).decode(bytes);
}

function ImportForm({ fromConnect, onClose, onImported }: { fromConnect: boolean; onClose: () => void; onImported: () => void }) {
  const client = useClient();
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ImportCategoryReport | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);

  async function choose(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    // Reset so choosing the same file again still fires a change.
    event.target.value = "";
    if (!file) return;
    setBusy(true);
    setFailure(null);
    setResult(null);
    try {
      setResult(await client.importPasswordsCsv(await readCsv(file)));
      onImported();
    } catch (cause) {
      setFailure(toFailure(cause));
    } finally {
      setBusy(false);
    }
  }

  if (failure?.localOnly) return <LocalOnlyNotice what={t("passwords.import.what")} onChanged={() => setFailure(null)} />;

  return (
    <div className="passwords-form">
      {fromConnect ? <InfoBanner tone="info">{t("passwords.connect.note")}</InfoBanner> : null}
      <p className="settings-text">{t("passwords.import.explain")}</p>
      <input ref={input} type="file" accept=".csv,text/csv" hidden onChange={(event) => void choose(event)} />
      {result ? (
        <p className="settings-text" role="status">
          {t("passwords.import.result", { imported: result.imported, skipped: result.skipped })}
          {result.note ? ` ${result.note}` : null}
        </p>
      ) : null}
      {failure ? <ErrorNote>{failure.message}</ErrorNote> : null}
      <DialogActions>
        <Button variant="ghost" onClick={onClose}>
          {t(result ? "common.done" : "common.cancel")}
        </Button>
        <Button variant="primary" disabled={busy} onClick={() => input.current?.click()}>
          <FileUp {...ICON} />
          {busy ? t("passwords.import.reading") : t("passwords.import.choose")}
        </Button>
      </DialogActions>
    </div>
  );
}

/**
 * Import › a CSV from a browser or another password manager, read here and sent once. Also the
 * nearest thing openside has to connecting an external manager (`fromConnect` says so).
 */
export function ImportDialog({
  open,
  onClose,
  remote,
  fromConnect,
  onImported,
}: {
  open: boolean;
  onClose: () => void;
  remote: boolean;
  fromConnect: boolean;
  onImported: () => void;
}) {
  return (
    <Dialog open={open} onClose={onClose} title={t("passwords.import.dialogTitle")}>
      {!open ? null : remote ? (
        <LocalOnlyNotice what={t("passwords.import.what")} onChanged={onClose} />
      ) : (
        <ImportForm fromConnect={fromConnect} onClose={onClose} onImported={onImported} />
      )}
    </Dialog>
  );
}
