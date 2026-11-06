import { Hand, History, SlidersHorizontal, Trash2 } from "lucide-react";
import { useCallback, useEffect, useId, useState } from "react";
import { ApiError } from "../../api/client";
import type { BrowsingDataType, PolicyProblem, PolicySettings } from "../../api/types";
import { useClient } from "../../context";
import { errorMessage } from "../../hooks/useResource";
import { t, type MessageKey } from "../../i18n";
import { ConnectionsPage } from "../../pages/ConnectionsPage";
import { lines, NetworkAccess, ProblemList } from "./NetworkAccess";
import { Button, Dialog, DialogActions, ICON_STROKE, Select, SettingsCard, SettingsRow, SettingsSection, Switch, useToast } from "../../ui";
import { useBrowserPage, useBrowserSettings } from "./personalShared";
import "./personal.css";

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

function FiltersDialog({ open, onClose, policy, onSaved }: { open: boolean; onClose: () => void; policy: PolicySettings | null; onSaved: (next: PolicySettings) => void }) {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const [text, setText] = useState("");
  const [problems, setProblems] = useState<PolicyProblem[]>([]);
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!open) return;
    setText(policy?.deny_origins.join("\n") ?? "");
    setProblems([]);
    setFailure(null);
  }, [open, policy]);

  const save = async () => {
    if (!policy) return;
    const next = { ...policy, deny_origins: lines(text) };
    setBusy(true);
    setFailure(null);
    try {
      const checked = await client.validatePolicy({ policy: next });
      const mine = checked.problems.filter((problem) => problem.list === "deny_origins");
      setProblems(mine);
      if (!checked.valid) {
        if (mine.length === 0) setFailure(checked.problems.map((problem) => problem.message).join(" "));
        return;
      }
      onSaved(await client.setPolicy(next));
      toast.notify(t("personal.security.filters.saved"));
      onClose();
    } catch (cause) {
      setFailure(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onClose={onClose} title={t("personal.security.filters.dialogTitle")} description={t("personal.security.filters.dialogDescription")}>
      <label className="personal-field" htmlFor={`${id}-deny`}>
        {t("personal.security.filters.label")}
      </label>
      <textarea
        id={`${id}-deny`}
        className="personal-textarea"
        rows={6}
        value={text}
        placeholder="https://*.example.com"
        spellCheck={false}
        aria-invalid={problems.length > 0 || undefined}
        onChange={(event) => setText(event.target.value)}
      />
      <ProblemList problems={problems} list="deny_origins" entries={lines(text)} />
      {failure ? (
        <p className="personal-error" role="alert">
          {failure}
        </p>
      ) : null}
      <DialogActions>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button variant="primary" disabled={busy || !policy} onClick={() => void save()}>
          {t("common.save")}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

const DATA_TYPES: readonly { type: BrowsingDataType; label: MessageKey }[] = [
  { type: "history", label: "personal.security.clearData.history" },
  { type: "cookies", label: "personal.security.clearData.cookies" },
  { type: "cache", label: "personal.security.clearData.cache" },
  { type: "downloads", label: "personal.security.clearData.downloads" },
  { type: "passwords", label: "personal.security.clearData.passwords" },
  { type: "form_data", label: "personal.security.clearData.formData" },
];
const HOUR = 3_600_000;
const RANGES: readonly { value: string; label: MessageKey; ms: number }[] = [
  { value: "hour", label: "personal.security.clearData.lastHour", ms: HOUR },
  { value: "day", label: "personal.security.clearData.lastDay", ms: 24 * HOUR },
  { value: "week", label: "personal.security.clearData.lastWeek", ms: 7 * 24 * HOUR },
  { value: "month", label: "personal.security.clearData.lastMonth", ms: 28 * 24 * HOUR },
  { value: "all", label: "personal.security.clearData.allTime", ms: 0 },
];
// Passwords and form data are opt-in: losing them costs far more than a stale cache.
const DEFAULT_TYPES: ReadonlySet<BrowsingDataType> = new Set(["history", "cookies", "cache"]);

function ClearDataDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const [types, setTypes] = useState<ReadonlySet<BrowsingDataType>>(DEFAULT_TYPES);
  const [range, setRange] = useState("hour");
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!open) return;
    setTypes(DEFAULT_TYPES);
    setRange("hour");
    setFailure(null);
  }, [open]);

  const toggle = (type: BrowsingDataType, on: boolean) =>
    setTypes((current) => {
      const next = new Set(current);
      if (on) next.add(type);
      else next.delete(type);
      return next;
    });

  const clear = async () => {
    const span = RANGES.find((option) => option.value === range)?.ms ?? 0;
    setBusy(true);
    setFailure(null);
    try {
      await client.clearBrowsingData({ types: DATA_TYPES.map((item) => item.type).filter((type) => types.has(type)), since_ms: span ? Date.now() - span : 0 });
      toast.notify(t("personal.security.clearData.done"), "success");
      onClose();
    } catch (cause) {
      setFailure(cause instanceof ApiError && cause.code === "person_browser_not_connected" ? t("personal.security.clearData.notConnected") : errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onClose={onClose} title={t("personal.security.clearData.title")}>
      <label className="personal-field" htmlFor={`${id}-range`}>
        {t("personal.security.clearData.range")}
      </label>
      <Select id={`${id}-range`} variant="field" value={range} options={RANGES.map((option) => ({ value: option.value, label: t(option.label) }))} onChange={setRange} />
      <fieldset className="personal-checks">
        <legend className="personal-field">{t("personal.security.clearData.types")}</legend>
        {DATA_TYPES.map((item) => (
          <label key={item.type} className="personal-check">
            <input type="checkbox" checked={types.has(item.type)} onChange={(event) => toggle(item.type, event.target.checked)} />
            <span>{t(item.label)}</span>
          </label>
        ))}
      </fieldset>
      {failure ? (
        <p className="personal-error" role="alert">
          {failure}
        </p>
      ) : null}
      <DialogActions>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button variant="danger" disabled={busy || types.size === 0} onClick={() => void clear()}>
          {t("personal.security.clearData.action")}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

function AdblockSection() {
  const client = useClient();
  const [policy, setPolicy] = useState<PolicySettings | null>(null);
  const [open, setOpen] = useState(false);
  const load = useCallback(() => client.policy().then(setPolicy, () => undefined), [client]);
  useEffect(() => {
    void load();
  }, [load]);
  const count = policy?.deny_origins.length ?? 0;
  return (
    <SettingsSection label={t("personal.security.adblock")}>
      <SettingsCard>
        <SettingsRow
          leading={<Hand {...ICON} />}
          title={t("personal.security.adblock.title")}
          description={t("personal.security.adblock.description")}
          control={(ids) => <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} disabled checked={false} onChange={() => undefined} />}
        />
        <SettingsRow
          leading={<SlidersHorizontal {...ICON} />}
          title={t("personal.security.filters.title")}
          description={policy ? t("personal.security.filters.count", { count }) : t("settings.status.checking")}
          control={
            <Button disabled={!policy} onClick={() => setOpen(true)}>
              <SlidersHorizontal {...ICON} />
              {t("personal.configure")}
            </Button>
          }
        />
      </SettingsCard>
      <FiltersDialog open={open} onClose={() => setOpen(false)} policy={policy} onSaved={setPolicy} />
    </SettingsSection>
  );
}

function PrivacySection() {
  const browser = useBrowserSettings();
  const openPage = useBrowserPage();
  const [sheet, setSheet] = useState<"sites" | "network" | null>(null);
  const cookies = browser.prefs?.third_party_cookies;
  return (
    <SettingsSection label={t("personal.security.privacy")}>
      <SettingsCard>
        <SettingsRow
          title={t("personal.security.cookies.title")}
          description={browser.note ?? t("personal.security.cookies.description")}
          control={(ids) => (
            <Select
              variant="inline"
              aria-labelledby={ids.labelId}
              aria-describedby={ids.descriptionId}
              disabled={!cookies}
              value={cookies ?? ""}
              placeholder="—"
              options={[
                { value: "allow", label: t("personal.security.cookies.allow") },
                { value: "block", label: t("personal.security.cookies.block") },
              ]}
              onChange={(value) => browser.update({ third_party_cookies: value === "block" ? "block" : "allow" })}
            />
          )}
        />
        <SettingsRow
          title={t("personal.security.telemetry.title")}
          description={t("personal.security.telemetry.description")}
          control={(ids) => <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} disabled checked={false} onChange={() => undefined} />}
        />
      </SettingsCard>
      <SettingsCard>
        <SettingsRow title={t("personal.security.adPrivacy.title")} description={t("personal.security.adPrivacy.description")} external onClick={() => openPage("chrome://settings/adPrivacy")} />
        <SettingsRow title={t("personal.security.siteSettings.title")} description={t("personal.security.siteSettings.description")} external onClick={() => openPage("chrome://settings/content")} />
        <SettingsRow title={t("personal.security.advanced.title")} description={t("personal.security.advanced.description")} external onClick={() => openPage("chrome://settings/security")} />
      </SettingsCard>
      <SettingsCard>
        <SettingsRow title={t("personal.security.sites.title")} description={t("personal.security.sites.description")} onClick={() => setSheet("sites")} />
        <SettingsRow title={t("personal.security.network.title")} description={t("personal.security.network.description")} onClick={() => setSheet("network")} />
      </SettingsCard>
      <Dialog open={sheet === "sites"} onClose={() => setSheet(null)} size="wide" title={t("personal.security.sites.title")}>
        <ConnectionsPage audit={false} />
      </Dialog>
      <Dialog open={sheet === "network"} onClose={() => setSheet(null)} size="wide" title={t("personal.security.network.title")}>
        <NetworkAccess />
      </Dialog>
    </SettingsSection>
  );
}

/** Settings › Security & Privacy: browsing history, blocking, cookies and where agents may go. */
export function SecuritySettings() {
  const openPage = useBrowserPage();
  const [clearing, setClearing] = useState(false);
  return (
    <div className="personal-page" data-section="security">
      <SettingsSection label={t("personal.security.history")}>
        <SettingsCard>
          <SettingsRow
            leading={<Trash2 {...ICON} />}
            title={t("personal.security.deleteHistory.title")}
            description={t("personal.security.deleteHistory.description")}
            control={
              <Button variant="danger" size="small" onClick={() => setClearing(true)}>
                {t("personal.security.deleteHistory.action")}
              </Button>
            }
          />
          <SettingsRow leading={<History {...ICON} />} title={t("personal.security.viewHistory.title")} external onClick={() => openPage("chrome://history")} />
        </SettingsCard>
        <ClearDataDialog open={clearing} onClose={() => setClearing(false)} />
      </SettingsSection>
      <AdblockSection />
      <PrivacySection />
    </div>
  );
}
