import { Ellipsis, Monitor, Moon, SlidersHorizontal, Sun } from "lucide-react";
import { useEffect, useState, type KeyboardEvent } from "react";
import { Link, useNavigate } from "react-router";
import { applyLook, loadLook, type Look, type TabStyle, type Transparency } from "../../appearance";
import { t } from "../../i18n";
import { RECENT_CHAT_CHOICES, useSidebarPrefs, type SidebarPrefs } from "../../shell";
import {
  BINDINGS_EVENT,
  chordFromEvent,
  chordKeys,
  chordProblem,
  keyLabel,
  saveBinding,
  shortcutKeys,
  loadBindings,
  type Chord,
  type ChordProblem,
  type Rebindable,
} from "../../shortcuts";
import { applyTheme, loadTheme, type ThemePreference } from "../../theme";
import {
  Button,
  Dialog,
  DialogActions,
  Dropdown,
  ICON_STROKE,
  IconButton,
  KbdChip,
  MenuItem,
  PopoverMenu,
  Select,
  SettingsCard,
  SettingsRow,
  SettingsSection,
  ThumbnailPicker,
} from "../../ui";
import { settingsPath } from "../nav";
import { MINI_POPUP_KEYS } from "./MiniPopup";
import { useBrowserPage, useBrowserSettings, type BrowserSettings } from "./personalShared";
import "./personal.css";

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;
/** The Chromium zoom levels the reference offers (67–500%); the daemon accepts any Chromium level. */
const ZOOM_LEVELS = [67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500];
const REBINDABLE_ROWS: readonly Rebindable[] = ["sidebar", "search", "focus-composer", "home"];
/** The patched browser's own split accelerators (browser/patches); fixed, so never rebindable here. */
const BROWSER_ROWS = [
  { row: "splitSide", keys: ["Mod", "Shift", "\\"] },
  { row: "splitStacked", keys: ["Mod", "Shift", "-"] },
] as const;

function Thumb({ serif, tinted }: { serif?: boolean; tinted?: boolean }) {
  return (
    <svg viewBox="0 0 70 43" role="presentation">
      <rect width="70" height="43" fill="var(--sidebar-bg)" />
      {tinted ? <rect width="70" height="43" fill="rgb(176 137 104 / 0.18)" /> : null}
      <rect x="20" y="5" width="46" height="34" rx="3" fill="var(--surface-raised)" />
      <circle cx="6" cy="6" r="1.6" fill="var(--faint)" />
      <circle cx="11" cy="6" r="1.6" fill="var(--faint)" />
      <rect x="4" y="12" width="12" height="2" rx="1" fill="var(--faint)" />
      <rect x="4" y="17" width="10" height="2" rx="1" fill="var(--faint)" />
      <text x="26" y="20" fontSize="9" fill="var(--text)" fontFamily={serif ? "Georgia, serif" : "var(--font-sans)"}>
        Aa
      </text>
      <rect x="26" y="26" width="30" height="2" rx="1" fill="var(--border)" />
      <rect x="26" y="31" width="22" height="2" rx="1" fill="var(--border)" />
    </svg>
  );
}

function LookCard({ browser }: { browser: BrowserSettings }) {
  const zoom = browser.prefs?.default_zoom_percent;
  const [theme, setTheme] = useState<ThemePreference>(loadTheme);
  const [look, setLook] = useState<Look>(loadLook);
  const changeLook = (patch: Partial<Look>) => {
    const next = { ...look, ...patch };
    applyLook(next);
    setLook(next);
  };

  return (
    <SettingsCard>
      <SettingsRow
        title={t("personal.appearance.theme.title")}
        description={t("personal.appearance.theme.description")}
        control={(ids) => (
          <Select
            variant="inline"
            aria-labelledby={ids.labelId}
            aria-describedby={ids.descriptionId}
            value={theme}
            options={[
              { value: "system", label: t("settings.appearance.mode.system"), icon: <Monitor {...ICON} /> },
              { value: "light", label: t("settings.appearance.mode.light"), icon: <Sun {...ICON} /> },
              { value: "dark", label: t("settings.appearance.mode.dark"), icon: <Moon {...ICON} /> },
            ]}
            onChange={(value) => {
              const next = value as ThemePreference;
              applyTheme(next);
              setTheme(next);
            }}
          />
        )}
      />
      <SettingsRow
        title={t("personal.appearance.zoom.title")}
        description={browser.note ?? undefined}
        control={(ids) => (
          <Select
            variant="inline"
            aria-labelledby={ids.labelId}
            aria-describedby={ids.descriptionId}
            disabled={zoom == null}
            value={zoom == null ? "" : String(zoom)}
            placeholder="—"
            options={ZOOM_LEVELS.map((level) => ({ value: String(level), label: `${level}%` }))}
            onChange={(value) => browser.update({ default_zoom_percent: Number(value) })}
          />
        )}
      />
      <SettingsRow
        title={t("personal.appearance.tabStyle.title")}
        control={(ids) => (
          <ThumbnailPicker<TabStyle>
            aria-labelledby={ids.labelId}
            value={look.tabStyle}
            onChange={(tabStyle) => changeLook({ tabStyle })}
            options={[
              { value: "sans", label: t("personal.appearance.tabStyle.sans"), thumbnail: <Thumb /> },
              { value: "serif", label: t("personal.appearance.tabStyle.serif"), thumbnail: <Thumb serif /> },
            ]}
          />
        )}
      />
      <SettingsRow
        title={t("personal.appearance.transparency.title")}
        description={t("personal.appearance.transparency.description")}
        control={(ids) => (
          <ThumbnailPicker<Transparency>
            aria-labelledby={ids.labelId}
            value={look.transparency}
            onChange={(transparency) => changeLook({ transparency })}
            options={[
              { value: "tinted", label: t("personal.appearance.transparency.tinted"), thumbnail: <Thumb tinted /> },
              { value: "transparent", label: t("personal.appearance.transparency.transparent"), thumbnail: <Thumb /> },
            ]}
          />
        )}
      />
    </SettingsCard>
  );
}

function SidebarCard() {
  const [prefs, update] = useSidebarPrefs();
  return (
    <SettingsCard>
      <SettingsRow
        title={t("personal.appearance.newTabPosition.title")}
        description={t("personal.appearance.newTabPosition.description")}
        control={(ids) => (
          <Select
            variant="inline"
            aria-labelledby={ids.labelId}
            aria-describedby={ids.descriptionId}
            value={prefs.newTabPosition}
            options={[
              { value: "top", label: t("personal.appearance.newTabPosition.top") },
              { value: "bottom", label: t("personal.appearance.newTabPosition.bottom") },
            ]}
            onChange={(value) => update({ newTabPosition: value as SidebarPrefs["newTabPosition"] })}
          />
        )}
      />
      <SettingsRow
        title={t("personal.appearance.sidebar.title")}
        description={t("personal.appearance.sidebar.description")}
        control={(ids) => (
          <Dropdown
            multiple
            variant="inline"
            aria-labelledby={ids.labelId}
            aria-describedby={ids.descriptionId}
            value={prefs.bookmarks ? ["chats", "bookmarks"] : ["chats"]}
            // Chats always show: they are how a person gets back to a conversation.
            options={[
              { value: "chats", label: t("personal.appearance.sidebar.chats"), disabled: true },
              { value: "bookmarks", label: t("personal.appearance.sidebar.bookmarks") },
            ]}
            onChange={(values) => update({ bookmarks: values.includes("bookmarks"), chats: true })}
          />
        )}
      />
      <SettingsRow
        title={t("personal.appearance.recentChats.title")}
        description={t("personal.appearance.recentChats.description")}
        control={(ids) => (
          <Select
            variant="inline"
            aria-labelledby={ids.labelId}
            aria-describedby={ids.descriptionId}
            value={String(prefs.recentChats)}
            options={RECENT_CHAT_CHOICES.map((count) => ({ value: String(count), label: t("personal.appearance.recentChats.option", { count }) }))}
            onChange={(value) => update({ recentChats: Number(value) as SidebarPrefs["recentChats"] })}
          />
        )}
      />
    </SettingsCard>
  );
}

function TabsSection() {
  const [prefs, update] = useSidebarPrefs();
  const openPage = useBrowserPage();
  const [tidyOpen, setTidyOpen] = useState(false);
  return (
    <SettingsSection label={t("personal.appearance.tabs")}>
      <SettingsCard>
        <SettingsRow
          title={t("personal.appearance.switcher.title")}
          description={t("personal.appearance.switcher.description")}
          control={(ids) => (
            <Select
              variant="inline"
              aria-labelledby={ids.labelId}
              aria-describedby={ids.descriptionId}
              value={prefs.tabSwitcherOrder}
              options={[
                { value: "recent", label: t("personal.appearance.switcher.recent") },
                { value: "tabs", label: t("personal.appearance.switcher.tabs") },
              ]}
              onChange={(value) => update({ tabSwitcherOrder: value as SidebarPrefs["tabSwitcherOrder"] })}
            />
          )}
        />
        <SettingsRow
          title={t("personal.appearance.tidy.title")}
          description={t("personal.appearance.tidy.description")}
          control={
            <Button onClick={() => setTidyOpen(true)}>
              <SlidersHorizontal {...ICON} />
              {t("personal.configure")}
            </Button>
          }
        />
        <SettingsRow title={t("personal.appearance.advanced")} external onClick={() => openPage("chrome://settings/appearance")} />
      </SettingsCard>
      <Dialog open={tidyOpen} onClose={() => setTidyOpen(false)} title={t("personal.appearance.tidy.title")} description={t("personal.appearance.tidy.dialogDescription")}>
        <DialogActions>
          <Link className="ui-button ui-button--secondary" to={settingsPath("agents")} onClick={() => setTidyOpen(false)}>
            {t("personal.appearance.tidy.agentSettings")}
          </Link>
          <Button variant="primary" onClick={() => setTidyOpen(false)}>
            {t("common.done")}
          </Button>
        </DialogActions>
      </Dialog>
    </SettingsSection>
  );
}

/** Re-renders when any shortcut changes, here or in another tab. */
function useBindingsVersion(): number {
  const [version, setVersion] = useState(0);
  useEffect(() => {
    const bump = () => setVersion((value) => value + 1);
    window.addEventListener(BINDINGS_EVENT, bump);
    return () => window.removeEventListener(BINDINGS_EVENT, bump);
  }, []);
  return version;
}

const shortcutName = (name: Rebindable | "settings") => (name === "settings" ? t("personal.appearance.shortcut.settings") : t(`personal.appearance.shortcut.${name}.title`));

function problemText(problem: ChordProblem): string {
  return problem.kind === "reserved" ? t("personal.appearance.shortcut.reserved") : t("personal.appearance.shortcut.taken", { name: shortcutName(problem.by) });
}

function ChangeShortcut({ shortcut, onClose }: { shortcut: Rebindable | null; onClose: () => void }) {
  const [chord, setChord] = useState<Chord | null>(null);
  useEffect(() => setChord(null), [shortcut]);
  const problem = shortcut && chord ? chordProblem(shortcut, chord) : null;
  const capture = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = chordFromEvent(event.nativeEvent);
    if (!next) return;
    event.preventDefault();
    event.stopPropagation();
    setChord(next);
  };
  return (
    <Dialog
      open={shortcut !== null}
      onClose={onClose}
      title={shortcut ? t("personal.appearance.shortcut.changeTitle", { name: shortcutName(shortcut) }) : ""}
      description={t("personal.appearance.shortcut.changeDescription", { mod: keyLabel(["Mod"]) })}
    >
      <div className="personal-capture" tabIndex={0} role="textbox" aria-readonly="true" aria-label={shortcut ? shortcutName(shortcut) : undefined} onKeyDown={capture} autoFocus>
        {chord ? <KbdChip keys={[keyLabel(chordKeys(chord))]} /> : <span className="personal-note">{t("personal.appearance.shortcut.waiting")}</span>}
      </div>
      {problem ? (
        <p className="personal-error" role="alert">
          {problemText(problem)}
        </p>
      ) : null}
      <DialogActions>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button
          variant="primary"
          disabled={!chord || problem !== null}
          onClick={() => {
            if (shortcut && chord) saveBinding(shortcut, chord);
            onClose();
          }}
        >
          {t("common.save")}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

function ShortcutMenu({ name, onChange, onReset, disabled }: { name: string; onChange?: () => void; onReset?: () => void; disabled?: boolean }) {
  const label = t("personal.appearance.shortcut.options", { name });
  return (
    <PopoverMenu
      label={label}
      trigger={
        <IconButton label={label} outlined size="small">
          <Ellipsis {...ICON} />
        </IconButton>
      }
    >
      <MenuItem disabled={disabled} onClick={onChange}>
        {t("personal.appearance.shortcut.change")}
      </MenuItem>
      <MenuItem disabled={disabled || !onReset} onClick={onReset}>
        {t("personal.appearance.shortcut.reset")}
      </MenuItem>
    </PopoverMenu>
  );
}

function ShortcutsSection({ browser }: { browser: BrowserSettings }) {
  useBindingsVersion();
  const openPage = useBrowserPage();
  const navigate = useNavigate();
  const [editing, setEditing] = useState<Rebindable | null>(null);
  const bindings = loadBindings();
  const copyUrl = t("personal.appearance.shortcut.copyUrl.title");
  const miniPopup = t("personal.appearance.shortcut.miniPopup.title");
  const miniKeys = keyLabel(MINI_POPUP_KEYS);
  const notSet = <KbdChip keys={["—"]} unset label={t("personal.appearance.shortcut.notSet")} />;
  return (
    <SettingsSection label={t("personal.appearance.shortcuts")}>
      <SettingsCard>
        {REBINDABLE_ROWS.map((shortcut) => {
          const name = shortcutName(shortcut);
          const keys = keyLabel(shortcutKeys(shortcut));
          return (
            <SettingsRow
              key={shortcut}
              title={name}
              description={t(`personal.appearance.shortcut.${shortcut}.description`)}
              control={
                <span className="personal-shortcut">
                  <KbdChip keys={[keys]} label={keys} />
                  <ShortcutMenu name={name} onChange={() => setEditing(shortcut)} onReset={bindings[shortcut] ? () => saveBinding(shortcut, null) : undefined} />
                </span>
              }
            />
          );
        })}
        <SettingsRow
          title={copyUrl}
          description={t("personal.appearance.shortcut.copyUrl.description")}
          control={
            <span className="personal-shortcut">
              {notSet}
              <ShortcutMenu name={copyUrl} disabled />
            </span>
          }
        />
        {BROWSER_ROWS.map(({ row, keys: chord }) => {
          const name = t(`personal.appearance.shortcut.${row}.title`);
          const keys = keyLabel(chord);
          return (
            <SettingsRow
              key={row}
              title={name}
              description={t(`personal.appearance.shortcut.${row}.description`)}
              control={
                <span className="personal-shortcut">
                  <KbdChip keys={[keys]} label={keys} />
                  <ShortcutMenu name={name} disabled />
                </span>
              }
            />
          );
        })}
        <SettingsRow
          title={miniPopup}
          description={t("personal.appearance.shortcut.miniPopup.description")}
          control={
            <span className="personal-shortcut">
              {browser.prefs?.mini_popup ? <KbdChip keys={[miniKeys]} label={miniKeys} /> : notSet}
              <ShortcutMenu name={miniPopup} onChange={() => navigate(settingsPath("mini-popup"))} />
            </span>
          }
        />
      </SettingsCard>
      <SettingsCard>
        <SettingsRow
          title={t("personal.appearance.shortcut.extensions.title")}
          description={t("personal.appearance.shortcut.extensions.description")}
          external
          onClick={() => openPage("chrome://extensions/shortcuts")}
        />
      </SettingsCard>
      <ChangeShortcut shortcut={editing} onClose={() => setEditing(null)} />
    </SettingsSection>
  );
}

/** Settings › Appearance: theme, zoom, the sidebar's look and sections, tabs and keyboard shortcuts. */
export function AppearanceSettings() {
  // One read of the browser profile for the zoom row and the Mini popup row.
  const browser = useBrowserSettings();
  return (
    <div className="personal-page" data-section="appearance">
      <SettingsSection>
        <LookCard browser={browser} />
        <SidebarCard />
      </SettingsSection>
      <TabsSection />
      <ShortcutsSection browser={browser} />
    </div>
  );
}
