import { LockKeyhole } from "lucide-react";
import { useId, useState } from "react";
import { loadLocalSecret } from "../auth";
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
      setError("That does not look like the output of openside remote local-access.");
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
    toast.notify((await copyText("openside remote local-access --json")) ? "Command copied" : "Could not copy; select the command instead", "success");
  }

  return (
    <div className="remote-local" role="note">
      <p className="remote-local-title">
        <LockKeyhole size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        {stale ? "Reopen openside from this computer" : `To ${what}, use openside on the computer that runs it`}
      </p>
      <p className="remote-note">
        {stale ? (
          <>This tab lost its link to the computer, usually because openside restarted. Reopen openside with <code>openside-browser</code> to continue.</>
        ) : (
          <>
            This is only possible from the computer itself, so a lost phone can never open your computer to the internet. Open openside there with{" "}
            <code>openside-browser</code>.
          </>
        )}
      </p>
      <button type="button" className="row-disclosure" aria-expanded={open} onClick={() => setOpen(!open)}>
        I already have a local access link
      </button>
      {open && (
        <div className="remote-local-form">
          <p className="remote-note">
            Run <code>openside remote local-access --json</code> on the computer and paste what it prints.
          </p>
          <div className="remote-copy-row">
            <Button size="compact" variant="ghost" aria-label="Copy the local access command" onClick={() => void copyCommand()}>
              Copy command
            </Button>
          </div>
          <label htmlFor={fieldId}>Local access output</label>
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
              Use this
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
