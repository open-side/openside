import { useCallback, useEffect, useId, useState } from "react";
import type { NetworkPolicy, PolicyProblem } from "../../api/types";
import { useClient } from "../../context";
import { useResource } from "../../hooks/useResource";
import { t } from "../../i18n";
import { tNodes } from "../../i18n/react";
import { ErrorNote } from "../../pages/blocks";
import { FailureNote, toFailure, type Failure } from "../../pages/Failure";
import { RestartNotice } from "../../pages/RestartNotice";
import { LocalOnlyNotice } from "../../remote/LocalOnlyNotice";
import { Button, useToast } from "../../ui";

/** One entry per non-empty line, trimmed: how every openside list editor reads a text box. */
export const lines = (text: string) =>
  text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

/** Problems for one list, in the words of the entry they point at. */
export function ProblemList({ problems, list, entries, id }: { problems: PolicyProblem[]; list: PolicyProblem["list"]; entries: string[]; id?: string }) {
  const mine = problems.filter((problem) => problem.list === list);
  if (mine.length === 0) return null;
  return (
    <ul id={id} className="policy-problems" role="alert">
      {mine.map((problem) => (
        <li key={`${problem.index}-${problem.message}`}>
          {tNodes("settings.policy.problem", {
            entry: entries[problem.index] ? <code>{entries[problem.index]}</code> : t("settings.policy.entry", { index: problem.index + 1 }),
            message: problem.message,
          })}
        </li>
      ))}
    </ul>
  );
}

/**
 * Security & Privacy › Network access: the hosts openside may reach besides model providers
 * (`network.allow`). The daemon enforces the list it started with, so a change asks for a restart.
 * Saving needs local authority, so a browser elsewhere is told where to make the change up front.
 */
export function NetworkAccess() {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const load = useCallback(() => client.networkPolicy(), [client]);
  const resource = useResource(load);
  const [saved, setSaved] = useState<NetworkPolicy | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [problems, setProblems] = useState<PolicyProblem[]>([]);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const [remote, setRemote] = useState(false);
  const checkLocal = useCallback(
    // The settings document says whether this caller lacks local authority; unknown stays editable and a refused save explains itself.
    () => client.settingsConfig().then((document) => setRemote(document.remote), () => undefined),
    [client],
  );
  useEffect(() => {
    void checkLocal();
  }, [checkLocal]);
  const current = saved ?? resource.data;
  const value = text ?? current?.allow.join("\n") ?? "";
  const dirty = text !== null && JSON.stringify(lines(text)) !== JSON.stringify(current?.allow ?? []);
  const mine = problems.filter((problem) => problem.list === "network_allow");
  const localOnly = remote || failure?.localOnly === true;
  async function save() {
    setBusy(true);
    setFailure(null);
    const allow = lines(value);
    try {
      const checked = await client.validatePolicy({ network_allow: allow });
      setProblems(checked.problems);
      if (!checked.valid) return;
      setSaved(await client.setNetworkPolicy(allow));
      setText(null);
      toast.notify(t("settings.policy.network.saved"));
    } catch (cause) {
      setFailure(toFailure(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="manage-body" data-section="network">
      <p className="settings-text">{t("settings.policy.network.description")}</p>
      {!current ? (
        resource.error ? <ErrorNote>{resource.error}</ErrorNote> : null
      ) : (
        <div className="policy-form">
          <div className="field">
            <label htmlFor={`${id}-allow`}>
              <span>{t("settings.policy.network.allowed")}</span>
            </label>
            <textarea
              id={`${id}-allow`}
              rows={4}
              value={value}
              placeholder="huggingface.co"
              spellCheck={false}
              readOnly={remote}
              aria-invalid={mine.length > 0 || undefined}
              aria-describedby={mine.length > 0 ? `${id}-problems ${id}-hint` : `${id}-hint`}
              onChange={(event) => setText(event.target.value)}
            />
            <p className="settings-text" id={`${id}-hint`}>
              {tNodes("settings.policy.network.hint", { host: <code>host</code>, wildcard: <code>*.domain</code>, hostPort: <code>host:port</code>, url: <code>https://host</code> })}
            </p>
            <ProblemList id={`${id}-problems`} problems={problems} list="network_allow" entries={lines(value)} />
          </div>
        </div>
      )}
      {localOnly ? (
        <LocalOnlyNotice
          what={t("settings.policy.network.changeWhat")}
          onChanged={() => {
            setFailure(null);
            void checkLocal();
          }}
        />
      ) : (
        <FailureNote failure={failure} what={t("settings.policy.network.changeWhat")} onRetry={() => void save()} />
      )}
      {dirty ? (
        <div className="config-save">
          <span className="config-save-status" role="status">
            {t("settings.unsavedChanges")}
          </span>
          <Button
            variant="ghost"
            onClick={() => {
              setText(null);
              setProblems([]);
              setFailure(null);
            }}
          >
            {t("settings.discard")}
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => void save()}>
            {t("settings.policy.network.save")}
          </Button>
        </div>
      ) : null}
      {current?.restart_required ? (
        <RestartNotice
          onRestarted={() => {
            setSaved(null);
            void resource.reload();
          }}
        >
          {t("settings.policy.network.restartNeeded")}
        </RestartNotice>
      ) : null}
    </div>
  );
}
