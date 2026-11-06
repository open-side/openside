import { LockKeyhole } from "lucide-react";
import { useId, useState } from "react";
import { loadLocalSecret } from "../auth";
import { t } from "../i18n";
import { tNodes } from "../i18n/react";
import { applyLocalAccess, parseLocalAccess } from "../localAccess";
import { Button, copyText, ICON_STROKE, useToast } from "../ui";
import "./remote.css";

/**
 * Remote access is changed only from the computer running openside, so a phone's token (even an
 * admin one) cannot open the daemon to the internet. This explains where to do it and takes the
 * local-access proof by hand when the desktop launcher did not pass it.
 */
export function LocalOnlyNotice({ what, onChanged }: { what: string; onChanged: () => void }) {
  const toast = useToast();
  const fieldId = useId();
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  // A secret that is stored but refused means the daemon restarted since this tab was opened.
  const stale = loadLocalSecret() !== null;

  function submit() {
    const access = parseLocalAccess(value);
    if (!access) {
      setError(t("connections.localOnly.notParsed"));
      return;
    }
    const tokenChanged = applyLocalAccess(access);
    setValue("");
    setError(null);
    // The app reads its token once at start; a new token needs a fresh start to take effect.
    if (tokenChanged) window.location.reload();
    else onChanged();
  }

  async function copyCommand() {
    toast.notify((await copyText("openside remote local-access --json")) ? t("connections.localOnly.commandCopied") : t("connections.localOnly.copyFailed"), "success");
  }

  return (
    <div className="remote-local" role="note">
      <p className="remote-local-title">
        <LockKeyhole size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        {stale ? t("connections.localOnly.staleTitle") : t("connections.localOnly.title", { what })}
      </p>
      <p className="remote-note">
        {tNodes(stale ? "connections.localOnly.staleBody" : "connections.localOnly.body", { command: <code>openside-browser</code> })}
      </p>
      <button type="button" className="row-disclosure" aria-expanded={open} onClick={() => setOpen(!open)}>
        {t("connections.localOnly.haveLink")}
      </button>
      {open && (
        <div className="remote-local-form">
          <p className="remote-note">
            {tNodes("connections.localOnly.runCommand", { command: <code>openside remote local-access --json</code> })}
          </p>
          <div className="remote-copy-row">
            <Button variant="ghost" aria-label={t("connections.localOnly.copyCommandLabel")} onClick={() => void copyCommand()}>
              {t("connections.localOnly.copyCommand")}
            </Button>
          </div>
          <label htmlFor={fieldId}>{t("connections.localOnly.outputLabel")}</label>
          <textarea
            id={fieldId}
            rows={3}
            value={value}
            spellCheck={false}
            autoComplete="off"
            onChange={(event) => setValue(event.target.value)}
          />
          {error && (
            <p role="alert" className="error-note">
              {error}
            </p>
          )}
          <div className="remote-actions">
            <Button variant="secondary" disabled={!value.trim()} onClick={submit}>
              {t("connections.localOnly.useThis")}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
