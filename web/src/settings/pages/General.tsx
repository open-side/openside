import { ArrowUp, SlidersHorizontal, X } from "lucide-react";
import { useCallback, useEffect, useId, useState, type FormEvent } from "react";
import type { About, Health, UpdateCheck } from "../../api/types";
import { saveNewTabMode, loadNewTabMode, type NewTabMode } from "../../appearance";
import { useClient } from "../../context";
import { loadLocalePreference, saveLocalePreference, t, type LocalePreference } from "../../i18n";
import { useLocale } from "../../i18n/react";
import { ImportStep } from "../../onboarding/ImportStep";
import { Button, Dialog, DialogActions, ICON_STROKE, IconButton, Select, SettingsCard, SettingsRow, SettingsSection, Switch, type SelectOption } from "../../ui";
import { REPOSITORY } from "../nav";
import { Avatar, useBrowserSettings } from "./personalShared";
import "./personal.css";

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;
/** aside-relay re-checks the connection every five seconds while Settings is open. */
const HEALTH_INTERVAL_MS = 5_000;
// The daemon accepts BCP 47 tags; this catches typos before the browser refuses them.
const LANGUAGE_TAG = /^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$/;

function languageName(tag: string, locale: string): string {
  try {
    return new Intl.DisplayNames([locale], { type: "language" }).of(tag) ?? tag;
  } catch {
    return tag;
  }
}

/** The person's name (Settings › Account) and the device this browser is paired as. */
function usePerson(): { name: string | null; device: string | null } {
  const client = useClient();
  const [person, setPerson] = useState<{ name: string | null; device: string | null }>({ name: null, device: null });
  useEffect(() => {
    let cancelled = false;
    void Promise.allSettled([client.preferences(), client.myDevice()]).then(([preferences, device]) => {
      if (cancelled) return;
      setPerson({
        name: preferences.status === "fulfilled" ? (preferences.value.name ?? null) : null,
        device: device.status === "fulfilled" ? device.value.name || null : null,
      });
    });
    return () => {
      cancelled = true;
    };
  }, [client]);
  return person;
}

function AccountCard({ onSignOut }: { onSignOut: () => void }) {
  const { name, device } = usePerson();
  const shown = name ?? device ?? t("personal.general.account.unnamed");
  return (
    <SettingsCard className="personal-account">
      <SettingsRow
        leading={<Avatar name={shown} size={40} />}
        title={shown}
        description={device ? t("personal.general.account.device", { device }) : t("personal.general.account.local")}
        control={<Button onClick={onSignOut}>{t("settings.access.signOut")}</Button>}
      />
    </SettingsCard>
  );
}

function LanguagesDialog({ open, onClose, languages, disabledNote, onSave }: { open: boolean; onClose: () => void; languages: readonly string[]; disabledNote: string | null; onSave: (next: string[]) => void }) {
  const id = useId();
  const locale = useLocale();
  const [display, setDisplay] = useState<LocalePreference>(loadLocalePreference);
  const [list, setList] = useState<string[]>([...languages]);
  const [draft, setDraft] = useState("");
  const [invalid, setInvalid] = useState(false);
  useEffect(() => {
    if (open) setList([...languages]);
  }, [open, languages]);

  const add = (event: FormEvent) => {
    event.preventDefault();
    const tag = draft.trim();
    if (!LANGUAGE_TAG.test(tag)) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    setDraft("");
    if (!list.some((entry) => entry.toLowerCase() === tag.toLowerCase())) setList([...list, tag]);
  };
  const move = (index: number) => setList([...list.slice(0, index - 1), list[index]!, list[index - 1]!, ...list.slice(index + 1)]);
  const changed = list.join() !== languages.join();
  // Language names stay in their own language so a person can find theirs whatever is showing.
  const displayOptions: SelectOption[] = [
    { value: "system", label: t("common.language.system") },
    { value: "en", label: "English" },
    { value: "ko", label: "한국어" },
  ];

  return (
    <Dialog open={open} onClose={onClose} title={t("personal.general.languages.dialogTitle")}>
      <SettingsCard>
        <SettingsRow
          title={t("common.language.label")}
          description={t("common.language.description")}
          control={(ids) => (
            <Select
              variant="inline"
              aria-labelledby={ids.labelId}
              aria-describedby={ids.descriptionId}
              value={display}
              options={displayOptions}
              onChange={(value) => {
                const next = value as LocalePreference;
                setDisplay(next);
                saveLocalePreference(next);
              }}
            />
          )}
        />
      </SettingsCard>
      <section className="personal-languages" aria-labelledby={`${id}-websites`}>
        <h3 className="personal-subheading" id={`${id}-websites`}>
          {t("personal.general.languages.websites")}
        </h3>
        <p className="personal-note">{disabledNote ?? t("personal.general.languages.websitesDescription")}</p>
        <ol className="personal-language-list">
          {list.map((tag, index) => {
            const language = languageName(tag, locale);
            return (
              <li key={tag}>
                <span>
                  {language}
                  {language !== tag ? <span className="personal-note"> {tag}</span> : null}
                </span>
                <IconButton size="small" label={t("personal.general.languages.moveUp", { language })} disabled={index === 0 || disabledNote !== null} onClick={() => move(index)}>
                  <ArrowUp {...ICON} />
                </IconButton>
                <IconButton
                  size="small"
                  label={t("personal.general.languages.remove", { language })}
                  disabled={list.length === 1 || disabledNote !== null}
                  onClick={() => setList(list.filter((entry) => entry !== tag))}
                >
                  <X {...ICON} />
                </IconButton>
              </li>
            );
          })}
        </ol>
        <form className="personal-inline-form" onSubmit={add}>
          <label className="sr-only" htmlFor={`${id}-add`}>
            {t("personal.general.languages.add")}
          </label>
          <input
            id={`${id}-add`}
            value={draft}
            disabled={disabledNote !== null}
            placeholder={t("personal.general.languages.addPlaceholder")}
            spellCheck={false}
            aria-invalid={invalid || undefined}
            onChange={(event) => setDraft(event.target.value)}
          />
          <Button type="submit" disabled={!draft.trim() || disabledNote !== null}>
            {t("common.add")}
          </Button>
        </form>
        {invalid ? (
          <p className="personal-error" role="alert">
            {t("personal.general.languages.invalid")}
          </p>
        ) : null}
      </section>
      <DialogActions>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button
          variant="primary"
          disabled={!changed || disabledNote !== null}
          onClick={() => {
            onSave(list);
            onClose();
          }}
        >
          {t("common.save")}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

function PreferenceCards() {
  const browser = useBrowserSettings();
  const { prefs, note } = browser;
  const locale = useLocale();
  const [mode, setMode] = useState<NewTabMode>(loadNewTabMode);
  const [languagesOpen, setLanguagesOpen] = useState(false);
  const engines: SelectOption[] = prefs?.search_engine.options.map((engine) => ({ value: String(engine.id), label: engine.name })) ?? [];
  const engine = prefs?.search_engine.current;
  const languages = prefs?.accept_languages ?? [];

  return (
    <SettingsSection label={t("personal.general.preferences")}>
      <SettingsCard>
        <SettingsRow
          title={t("personal.general.searchEngine.title")}
          description={note ?? t("personal.general.searchEngine.description")}
          control={(ids) => (
            <Select
              variant="inline"
              aria-labelledby={ids.labelId}
              aria-describedby={ids.descriptionId}
              disabled={!prefs}
              value={engine == null ? "" : String(engine)}
              placeholder="—"
              options={engines}
              onChange={(value) => browser.update({ search_engine: Number(value) })}
            />
          )}
        />
        <SettingsRow
          title={t("personal.general.newTabMode.title")}
          description={t("personal.general.newTabMode.description")}
          control={(ids) => (
            <Select
              variant="inline"
              aria-labelledby={ids.labelId}
              aria-describedby={ids.descriptionId}
              value={mode}
              options={[
                { value: "search", label: t("personal.general.newTabMode.search") },
                { value: "ask", label: t("personal.general.newTabMode.ask") },
              ]}
              onChange={(value) => {
                const next = value as NewTabMode;
                setMode(next);
                saveNewTabMode(next);
              }}
            />
          )}
        />
      </SettingsCard>
      <SettingsCard>
        <SettingsRow
          title={t("personal.general.pip.title")}
          description={note ?? t("personal.general.pip.description")}
          control={(ids) => (
            <Switch
              aria-labelledby={ids.labelId}
              aria-describedby={ids.descriptionId}
              disabled={prefs?.auto_picture_in_picture == null}
              checked={prefs?.auto_picture_in_picture === true}
              onChange={(checked) => browser.update({ auto_picture_in_picture: checked })}
            />
          )}
        />
        <SettingsRow
          title={t("personal.general.screenshot.title")}
          description={t("personal.general.screenshot.description")}
          control={(ids) => <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} disabled checked={false} onChange={() => undefined} />}
        />
      </SettingsCard>
      <SettingsCard>
        <SettingsRow
          title={t("personal.general.languages.title")}
          description={note ?? (languages.map((tag) => languageName(tag, locale)).join(", ") || undefined)}
          control={
            <Button onClick={() => setLanguagesOpen(true)}>
              <SlidersHorizontal {...ICON} />
              {t("personal.configure")}
            </Button>
          }
        />
        <SettingsRow
          title={t("personal.general.spellCheck.title")}
          description={note ?? t("personal.general.spellCheck.description")}
          control={(ids) => (
            <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} disabled={!prefs} checked={prefs?.spell_check === true} onChange={(checked) => browser.update({ spell_check: checked })} />
          )}
        />
      </SettingsCard>
      <LanguagesDialog
        open={languagesOpen}
        onClose={() => setLanguagesOpen(false)}
        languages={languages}
        disabledNote={note}
        onSave={(next) => browser.update({ accept_languages: next })}
      />
    </SettingsSection>
  );
}

function ImportCards() {
  const [open, setOpen] = useState<"browser" | "bookmarks" | null>(null);
  return (
    <SettingsSection label={t("personal.general.import")}>
      <SettingsCard>
        <SettingsRow title={t("personal.general.import.browser.title")} description={t("personal.general.import.browser.description")} onClick={() => setOpen("browser")} />
        <SettingsRow title={t("personal.general.import.bookmarks.title")} description={t("personal.general.import.bookmarks.description")} onClick={() => setOpen("bookmarks")} />
      </SettingsCard>
      <Dialog
        open={open !== null}
        onClose={() => setOpen(null)}
        size="wide"
        title={open === "bookmarks" ? t("personal.general.import.bookmarks.title") : t("personal.general.import.browser.title")}
      >
        {open ? <ImportStep key={open} update={() => undefined} only={open === "bookmarks" ? ["bookmarks"] : undefined} /> : null}
      </Dialog>
    </SettingsSection>
  );
}

type Check = { state: "idle" } | { state: "checking" } | { state: "done"; result: UpdateCheck } | { state: "failed" };

type UserAgentData = { getHighEntropyValues?: (hints: string[]) => Promise<{ fullVersionList?: { brand: string; version: string }[] }> };
// openside's browser is a Chromium build; a stock Chrome reports its own brand next to Chromium's.
const BROWSER_BRANDS = ["Chromium", "Google Chrome"];

/** The browser's full version from User-Agent Client Hints; null where the browser does not report one. */
function useBrowserVersion(): string | null {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const data = (navigator as Navigator & { userAgentData?: UserAgentData }).userAgentData;
    data?.getHighEntropyValues?.(["fullVersionList"]).then(
      ({ fullVersionList = [] }) => {
        const found = BROWSER_BRANDS.map((brand) => fullVersionList.find((entry) => entry.brand === brand)).find(Boolean);
        if (alive && found) setVersion(found.version);
      },
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, []);
  return version;
}

function useHealth(): { health: Health | null; failed: boolean } {
  const client = useClient();
  const [state, setState] = useState<{ health: Health | null; failed: boolean }>({ health: null, failed: false });
  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    const check = async () => {
      try {
        const health = await client.health();
        if (cancelled) return;
        setState({ health, failed: false });
      } catch {
        if (cancelled) return;
        setState((current) => ({ ...current, failed: true }));
      }
      timer = window.setTimeout(() => void check(), HEALTH_INTERVAL_MS);
    };
    void check();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client]);
  return state;
}

function AboutCards() {
  const client = useClient();
  const [about, setAbout] = useState<About | null>(null);
  const [check, setCheck] = useState<Check>({ state: "idle" });
  const { health, failed } = useHealth();
  useEffect(() => {
    let cancelled = false;
    client.about().then(
      (next) => {
        if (!cancelled) setAbout(next);
      },
      () => undefined,
    );
    return () => {
      cancelled = true;
    };
  }, [client]);

  const runCheck = useCallback(async () => {
    setCheck({ state: "checking" });
    try {
      setCheck({ state: "done", result: await client.checkForUpdates() });
    } catch {
      setCheck({ state: "failed" });
    }
  }, [client]);

  const browserVersion = useBrowserVersion();
  const version = browserVersion ? t("personal.general.about.version", { version: browserVersion }) : "—";
  const appState =
    check.state === "done"
      ? check.result.update_available && check.result.latest
        ? t("personal.general.about.available", { latest: `v${check.result.latest}` })
        : t("personal.general.about.upToDate")
      : check.state === "failed"
        ? t("personal.general.about.checkFailed")
        : "";
  const update = check.state === "done" && check.result.update_available ? (check.result.url ?? about?.releases_url ?? null) : null;
  const service = failed ? t("personal.general.about.notResponding") : health ? (health.ok ? t("personal.general.about.running") : t("personal.general.about.degraded")) : "";
  const serviceDetails = [
    service,
    about ? t("personal.general.about.serviceVersion", { version: `v${about.version}` }) : null,
    about?.runtime_version ? t("personal.general.about.runtime", { version: about.runtime_version }) : null,
  ].filter(Boolean);

  return (
    <SettingsSection label={t("personal.general.about")}>
      <SettingsCard>
        <SettingsRow
          title={t("personal.general.about.app")}
          description={
            <span role="status">
              {version}
              {appState ? ` ${appState}` : null}
            </span>
          }
          control={
            update ? (
              <a className="ui-button ui-button--primary" href={update} target="_blank" rel="noopener noreferrer">
                {t("personal.general.about.download")}
              </a>
            ) : (
              <Button variant="primary" disabled={check.state === "checking"} onClick={() => void runCheck()}>
                {check.state === "checking" ? t("personal.general.about.checking") : t("personal.general.about.check")}
              </Button>
            )
          }
        />
        <SettingsRow
          title={t("personal.general.about.service")}
          description={
            <span role="status">{serviceDetails.join(" ")}</span>
          }
        />
        <SettingsRow to="/welcome" title={t("settings.general.runSetup")} />
      </SettingsCard>
      <p className="personal-footer">
        {t("personal.general.footer")}{" "}
        <a href={REPOSITORY} target="_blank" rel="noopener noreferrer">
          {t("personal.general.footer.source")}
        </a>
      </p>
    </SettingsSection>
  );
}

/** Settings › General: who is signed in, browser preferences, import and About. */
export function GeneralSettings({ onSignOut }: { onSignOut: () => void }) {
  return (
    <div className="personal-page" data-section="general">
      <AccountCard onSignOut={onSignOut} />
      <PreferenceCards />
      <ImportCards />
      <AboutCards />
    </div>
  );
}
