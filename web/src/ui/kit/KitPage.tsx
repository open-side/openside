/*
 * Dev-only catalogue of the UI kit (`/__kit`, loaded by main.tsx only under `import.meta.env.DEV`).
 * Its copy is fixture data: the fixtures repeat Aside's labels so per-component crops can be
 * diffed against the Aside screenshots (tools: /tmp/openside-qa/measure/sbs.py). Nothing here
 * ships, so it stays out of the i18n catalogue.
 *
 *   /__kit                         light and dark side by side (two frames)
 *   /__kit?theme=dark              one theme
 *   /__kit?fixture=<id>&theme=…    a fixture placed at Aside's window coordinates (1920×960)
 */
import { ArrowUpRight, Archive, Copy, Monitor, Moon, Pencil, Plus, Settings2, Sun, Trash2 } from "lucide-react";
import { useState, type ReactNode } from "react";
import { MemoryRouter } from "react-router";
import {
  Badge,
  Button,
  Dropdown,
  EmptyState,
  HeroCard,
  ICON_STROKE,
  IconButton,
  InfoBanner,
  KbdChip,
  MenuGroupLabel,
  MenuItem,
  MenuSeparator,
  PopoverMenu,
  Select,
  SettingsCard,
  SettingsRow,
  SettingsSection,
  SplitSelect,
  Switch,
  ThumbnailPicker,
  type SelectOption,
} from "..";
import "./kit.css";

const params = new URLSearchParams(window.location.search);

function Thumb({ serif }: { serif?: boolean }) {
  return (
    <svg viewBox="0 0 70 43" role="presentation">
      <rect width="70" height="43" fill="var(--sidebar-bg)" />
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

function Illustration() {
  return (
    <svg viewBox="0 0 288 165" role="presentation" preserveAspectRatio="xMidYMid slice">
      <rect width="288" height="165" fill="var(--sidebar-bg)" />
      <rect x="40" y="28" width="208" height="120" rx="10" fill="var(--surface-raised)" />
      <rect x="58" y="48" width="96" height="8" rx="4" fill="var(--border)" />
      <rect x="58" y="64" width="150" height="6" rx="3" fill="var(--hover)" />
      <rect x="58" y="76" width="130" height="6" rx="3" fill="var(--hover)" />
      <rect x="58" y="112" width="60" height="18" rx="6" fill="var(--primary)" />
    </svg>
  );
}

const engines: SelectOption[] = [
  { value: "google", label: "Google" },
  { value: "duckduckgo", label: "DuckDuckGo" },
  { value: "bing", label: "Bing" },
];
const tabModes: SelectOption[] = [
  { value: "search", label: "Search" },
  { value: "ask", label: "Ask" },
];
const themes: SelectOption[] = [
  { value: "system", label: "System", icon: <Monitor strokeWidth={ICON_STROKE} /> },
  { value: "light", label: "Light", icon: <Sun strokeWidth={ICON_STROKE} /> },
  { value: "dark", label: "Dark", icon: <Moon strokeWidth={ICON_STROKE} /> },
];
const zooms: SelectOption[] = ["67%", "75%", "90%", "100%", "110%", "125%"].map((value) => ({ value, label: value }));
const positions: SelectOption[] = [
  { value: "top", label: "At top" },
  { value: "bottom", label: "At bottom" },
];
const sections: SelectOption[] = [
  { value: "chats", label: "Chats" },
  { value: "bookmarks", label: "Bookmarks" },
  { value: "tabs", label: "Tabs" },
];
const recents: SelectOption[] = ["1", "3", "5"].map((value) => ({ value, label: `${value} chats` }));
const models: SelectOption[] = ["GPT-6.1 Sol", "GPT-6.1 Terra", "GPT-6 Luna", "Claude Haiku 5.5", "Claude Sonnet 5.5", "Gemini 4 Argon", "Gemini 3.8 Flash"].map((label) => ({ value: label, label }));
const notifications: SelectOption[] = [
  { value: "everything", label: "Everything" },
  { value: "mentions", label: "Only failures" },
  { value: "off", label: "Nothing" },
];

function ThemeRow() {
  const [theme, setTheme] = useState("system");
  return (
    <SettingsRow
      title="Theme"
      description="Choose how openside looks"
      control={(ids) => <Select variant="inline" aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} value={theme} onChange={setTheme} options={themes} />}
    />
  );
}

function AppearanceCards() {
  const [zoom, setZoom] = useState("100%");
  const [tabStyle, setTabStyle] = useState("serif");
  const [transparency, setTransparency] = useState("transparent");
  const [position, setPosition] = useState("top");
  const [shown, setShown] = useState<string[]>(["chats", "bookmarks"]);
  const [recent, setRecent] = useState("3");
  return (
    <>
      <SettingsCard>
        <ThemeRow />
        <SettingsRow title="Zoom level" control={(ids) => <Select variant="inline" aria-labelledby={ids.labelId} value={zoom} onChange={setZoom} options={zooms} />} />
        <SettingsRow
          title="Tab style"
          control={(ids) => (
            <ThumbnailPicker
              aria-labelledby={ids.labelId}
              value={tabStyle}
              onChange={setTabStyle}
              options={[
                { value: "sans", label: "San Francisco", thumbnail: <Thumb /> },
                { value: "serif", label: "New York", thumbnail: <Thumb serif />, accent: "var(--badge-new-bg)" },
              ]}
            />
          )}
        />
        <SettingsRow
          title="Transparency"
          control={(ids) => (
            <ThumbnailPicker
              aria-labelledby={ids.labelId}
              value={transparency}
              onChange={setTransparency}
              options={[
                { value: "tinted", label: "Tinted", thumbnail: <Thumb /> },
                { value: "transparent", label: "Transparent", thumbnail: <Thumb />, accent: "var(--badge-new-bg)" },
              ]}
            />
          )}
        />
      </SettingsCard>
      <SettingsCard>
        <SettingsRow
          title="New tab position"
          description="Choose where new tabs are added in the sidebar."
          control={(ids) => <Select variant="inline" aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} value={position} onChange={setPosition} options={positions} />}
        />
        <SettingsRow
          title="Customize sidebar"
          description="Choose which sections appear above your tabs."
          control={(ids) => <Dropdown multiple variant="inline" aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} value={shown} onChange={setShown} options={sections} />}
        />
        <SettingsRow
          title="Recent chats in sidebar"
          description="How many recent chats to show in the sidebar."
          control={(ids) => <Select variant="inline" aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} value={recent} onChange={setRecent} options={recents} />}
        />
      </SettingsCard>
    </>
  );
}

function GeneralSection() {
  const [engine, setEngine] = useState("google");
  const [mode, setMode] = useState("search");
  const [pip, setPip] = useState(false);
  const [shot, setShot] = useState(true);
  const [spell, setSpell] = useState(true);
  return (
    <SettingsSection label="Preferences">
      <SettingsCard>
        <SettingsRow
          title="Default search engine"
          description="Used for searches from the address bar"
          control={(ids) => <Select variant="inline" aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} value={engine} onChange={setEngine} options={engines} />}
        />
        <SettingsRow
          title="New tab mode"
          description="Open new tabs in Search or Ask"
          control={(ids) => <Select variant="inline" aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} value={mode} onChange={setMode} options={tabModes} />}
        />
      </SettingsCard>
      <SettingsCard>
        <SettingsRow
          title="Auto Picture-in-Picture"
          description="Show floating player when switching from a video tab"
          control={(ids) => <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} checked={pip} onChange={setPip} />}
        />
        <SettingsRow
          title="Screenshot"
          description="Capture the current page from the right-click menu"
          control={(ids) => <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} checked={shot} onChange={setShot} />}
        />
      </SettingsCard>
      <SettingsCard>
        <SettingsRow
          title="Preferred languages"
          description="English (United States), English"
          control={
            <Button>
              <Settings2 strokeWidth={ICON_STROKE} aria-hidden="true" />
              Configure
            </Button>
          }
        />
        <SettingsRow
          title="Spell check"
          description="Check spelling as you type"
          control={(ids) => <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} checked={spell} onChange={setSpell} />}
        />
      </SettingsCard>
    </SettingsSection>
  );
}

function ModelMenu({ open }: { open?: boolean }) {
  const [model, setModel] = useState("GPT-6.1 Sol");
  return (
    <PopoverMenu
      label="Model"
      popupClassName="kit-model-menu"
      align="end"
      open={open}
      search={{ placeholder: "Search models", empty: "No models match" }}
      trigger={
        <button type="button" className="kit-chip">
          {model}
        </button>
      }
    >
      {models.map((option) => (
        <MenuItem key={option.value} checked={option.value === model} onClick={() => setModel(option.value)}>
          {option.label}
        </MenuItem>
      ))}
      <MenuSeparator />
      <MenuItem trailing={<ArrowUpRight strokeWidth={ICON_STROKE} />} onClick={() => undefined}>
        Settings
      </MenuItem>
    </PopoverMenu>
  );
}

function Frame({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="kit-frame">
      <h2 className="kit-heading">{title}</h2>
      <div className="kit-body">{children}</div>
    </section>
  );
}

function Catalogue() {
  const [on, setOn] = useState(true);
  const [off, setOff] = useState(false);
  const [engine, setEngine] = useState("google");
  const [notify, setNotify] = useState("everything");
  const [shown, setShown] = useState<string[]>([]);
  const [model, setModel] = useState("GPT-6.1 Sol");
  const [image, setImage] = useState("GPT-6 Luna");
  return (
    <main className="kit">
      <Frame title="Buttons">
        <div className="kit-line">
          <Button>Sign out</Button>
          <Button>
            <Copy strokeWidth={ICON_STROKE} aria-hidden="true" />
            Invite
          </Button>
          <Button disabled>Disabled</Button>
          <Button size="small">Change</Button>
          <Button size="small" disabled>
            Small disabled
          </Button>
        </div>
        <div className="kit-line">
          <Button variant="primary">Check for updates</Button>
          <Button variant="primary" disabled>
            Disabled
          </Button>
          <Button variant="danger">Delete</Button>
          <Button variant="danger" disabled>
            Clear
          </Button>
          <Button variant="ghost">
            <Plus strokeWidth={ICON_STROKE} aria-hidden="true" />
            Create
          </Button>
        </div>
        <div className="kit-line">
          <IconButton label="More options" outlined>
            <Pencil size={14} strokeWidth={ICON_STROKE} />
          </IconButton>
          <IconButton label="More options" outlined disabled>
            <Pencil size={14} strokeWidth={ICON_STROKE} />
          </IconButton>
        </div>
      </Frame>

      <Frame title="Toggle">
        <div className="kit-line">
          <Switch label="On" checked={on} onChange={setOn} />
          <Switch label="Off" checked={off} onChange={setOff} />
          <Switch label="Disabled on" checked disabled onChange={() => undefined} />
          <Switch label="Disabled off" checked={false} disabled onChange={() => undefined} />
        </div>
      </Frame>

      <Frame title="Select (stepper), Dropdown, SplitSelect">
        <div className="kit-line">
          <Select variant="inline" aria-label="Search engine" value={engine} onChange={setEngine} options={engines} />
          <Select variant="inline" aria-label="Theme" value="system" onChange={() => undefined} options={themes} />
          <Select variant="inline" aria-label="Profile" value="" placeholder="Select…" onChange={() => undefined} options={engines} />
          <Select variant="inline" aria-label="Disabled" value="google" disabled onChange={() => undefined} options={engines} />
        </div>
        <div className="kit-line">
          <Dropdown variant="inline" aria-label="Task notifications" value={notify} onChange={setNotify} options={notifications} />
          <Dropdown multiple variant="inline" aria-label="Add skill to" placeholder="Add skill to…" value={shown} onChange={setShown} options={sections} />
        </div>
        <div className="kit-line">
          <SplitSelect
            aria-label="Default model"
            value={model}
            onChange={setModel}
            options={models}
            more={{
              label: "More options",
              items: (
                <>
                  <MenuItem icon={<Settings2 strokeWidth={ICON_STROKE} />} onClick={() => undefined}>
                    Configure
                  </MenuItem>
                  <MenuItem icon={<Trash2 strokeWidth={ICON_STROKE} />} danger onClick={() => undefined}>
                    Reset
                  </MenuItem>
                </>
              ),
            }}
          />
          <SplitSelect aria-label="Image generation model" value={image} onChange={setImage} options={models} />
          <SplitSelect aria-label="Disabled" value={model} onChange={setModel} options={models} disabled more={{ label: "More options", items: null }} />
        </div>
        <div className="kit-line">
          <div className="kit-field">
            <Select aria-label="Field" value={engine} onChange={setEngine} options={engines} />
          </div>
        </div>
      </Frame>

      <Frame title="KbdChip, Badge">
        <div className="kit-line">
          <KbdChip keys={["⌘", "S"]} label="Command S" />
          <KbdChip keys={["⇧", "⌘", "A"]} label="Shift Command A" />
          <KbdChip keys={["⌥", "Space"]} unset label="Option Space, not set" />
          <IconButton label="More options" outlined>
            <span className="kit-dots" aria-hidden="true">
              ···
            </span>
          </IconButton>
        </div>
        <div className="kit-line">
          <Badge tone="new">New</Badge>
          <Badge>This device</Badge>
          <Badge>Default</Badge>
        </div>
      </Frame>

      <Frame title="Popover menu (row 32px)">
        <div className="kit-line">
          <ModelMenu />
          <PopoverMenu label="Chat" align="start" trigger={<Button>Open menu</Button>}>
            <MenuGroupLabel>Projects</MenuGroupLabel>
            <MenuItem icon={<Pencil strokeWidth={ICON_STROKE} />} onClick={() => undefined}>
              Rename
            </MenuItem>
            <MenuItem icon={<Copy strokeWidth={ICON_STROKE} />} trailing="⇧⌘C" onClick={() => undefined}>
              Copy session ID
            </MenuItem>
            <MenuItem icon={<Settings2 strokeWidth={ICON_STROKE} />} disabled onClick={() => undefined}>
              Disabled
            </MenuItem>
            <MenuSeparator />
            <MenuItem icon={<Archive strokeWidth={ICON_STROKE} />} onClick={() => undefined}>
              Archive
            </MenuItem>
            <MenuItem icon={<Trash2 strokeWidth={ICON_STROKE} />} danger onClick={() => undefined}>
              Delete
            </MenuItem>
          </PopoverMenu>
        </div>
      </Frame>

      <Frame title="Settings rows">
        <MemoryRouter>
          <GeneralSection />
          <SettingsSection label="Import">
            <SettingsCard>
              <SettingsRow title="Import from another browser" description="Bring history, cookies, bookmarks and passwords" to="/import" />
              <SettingsRow title="Advanced" href="https://example.com" />
              <SettingsRow title="Context Awareness" badge={<Badge tone="new">New</Badge>} onClick={() => undefined} />
            </SettingsCard>
          </SettingsSection>
          <SettingsSection label="Appearance">
            <AppearanceCards />
          </SettingsSection>
        </MemoryRouter>
      </Frame>

      <Frame title="HeroCard, EmptyState, InfoBanner">
        <HeroCard
          title="openside CLI"
          description="Browse and run web tasks from your terminal and other agents."
          action={<Button variant="primary">Install</Button>}
          illustration={<Illustration />}
        />
        <HeroCard title="Without picture" description="A hero card can stand without its illustration." />
        <EmptyState icon={<Archive strokeWidth={ICON_STROKE} />} title="No archived chats" description="Chats you archive will appear here." />
        <SettingsCard>
          <InfoBanner action={<Button size="small">Learn more</Button>}>You can keep up to 3 routines active.</InfoBanner>
          <SettingsRow title="Create" leading={<Plus size={14} strokeWidth={ICON_STROKE} />} onClick={() => undefined} />
        </SettingsCard>
        <InfoBanner tone="info" title="You might not need an MCP">
          Most sites work with the built-in browser tools. Connect a server only for tools it cannot reach.
        </InfoBanner>
      </Frame>
    </main>
  );
}

/** Aside's window coordinates minus the 30px menu bar (measure-shell §0). */
const FIXTURES: Record<string, () => ReactNode> = {
  "l41-preferences": () => (
    // Preferences label block starts 28px above the first card (y 382 − 30).
    <div className="kit-at kit-column" style={{ top: 352 - 28 }}>
      <MemoryRouter>
        <GeneralSection />
      </MemoryRouter>
    </div>
  ),
  "l42-appearance": () => (
    <div className="kit-at kit-column kit-stack" style={{ top: 194 - 30 }}>
      <AppearanceCards />
    </div>
  ),
  "l15-model": () => (
    // Menu box 1176..1369 × 434..748; the trigger sits 6px above it, right-aligned.
    <div className="kit-at" style={{ top: 434 - 30 - 6 - 22, left: 1369 - 120, width: 120 }}>
      <ModelMenu open />
    </div>
  ),
};

export function KitPage() {
  const theme = params.get("theme");
  const fixture = params.get("fixture");
  if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
  if (fixture && FIXTURES[fixture]) {
    return <div className="kit-fixture">{FIXTURES[fixture]()}</div>;
  }
  if (!theme) {
    return (
      <div className="kit-split">
        <iframe title="Light" src="/__kit?theme=light" />
        <iframe title="Dark" src="/__kit?theme=dark" />
      </div>
    );
  }
  return <Catalogue />;
}
