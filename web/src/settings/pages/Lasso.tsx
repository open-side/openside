import { Code, Ellipsis, Image, Languages, Plus, TextAlignStart, Zap } from "lucide-react";
import { useCallback, useEffect, useId, useRef, useState, type FormEvent } from "react";
import { messageExtension } from "../../desktopNotify";
import { t } from "../../i18n";
import {
  DEFAULT_LASSO,
  LASSO_HOTKEYS,
  MAX_LABEL_CHARS,
  MAX_PROMPT_CHARS,
  MAX_SHORTCUTS,
  isBuiltin,
  parseLassoSettings,
  type LassoHotkey,
  type LassoReply,
  type LassoRequest,
  type LassoSettings,
  type LassoShortcut,
} from "../../lassoProtocol";
import { keyLabel } from "../../shortcuts";
import {
  Button,
  Dialog,
  DialogActions,
  HeroCard,
  IconButton,
  ICON_STROKE,
  InfoBanner,
  MenuItem,
  PopoverMenu,
  Select,
  SettingsCard,
  SettingsRow,
  SettingsSection,
  Switch,
  useToast,
} from "../../ui";
import "./personal.css";
import "./lasso.css";

type Kind = "text" | "region";
type Ready = Extract<LassoReply, { ok: true }>;

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

function isLassoReply(value: unknown): value is LassoReply {
  if (typeof value !== "object" || value === null || !("ok" in value)) return false;
  const reply = value as Record<string, unknown>;
  if (reply.ok === false) return typeof reply.message === "string";
  return reply.ok === true && parseLassoSettings(reply.settings) !== null && typeof reply.access === "boolean" && typeof reply.command === "string";
}

async function askLasso(request: LassoRequest): Promise<Ready | null> {
  const reply = await messageExtension(request, isLassoReply);
  return reply?.ok ? { ...reply, settings: parseLassoSettings(reply.settings)! } : null;
}

/** The modifier as this device writes it; `keyLabel` has no Control glyph, so Apple's ⌃ is added here. */
function hotkeyKey(hotkey: LassoHotkey): string {
  if (hotkey === "control") return keyLabel(["Alt"]) === "Alt" ? "Ctrl" : "⌃";
  return keyLabel([hotkey === "alt" ? "Alt" : "Shift"]);
}

function shortcutLabel(shortcut: LassoShortcut): string {
  if (shortcut.label) return shortcut.label;
  return isBuiltin(shortcut.id) ? t(`settings.lasso.builtin.${shortcut.id}.label`) : shortcut.id;
}

function shortcutPrompt(shortcut: LassoShortcut): string {
  if (shortcut.prompt) return shortcut.prompt;
  return isBuiltin(shortcut.id) ? t(`settings.lasso.builtin.${shortcut.id}.prompt`) : "";
}

function ShortcutIcon({ id }: { id: string }) {
  if (id === "summarize") return <TextAlignStart {...ICON} />;
  if (id === "translate") return <Languages {...ICON} />;
  if (id === "copy-code") return <Code {...ICON} />;
  if (id === "search-image") return <Image {...ICON} />;
  return <Zap {...ICON} />;
}

/** openside's own picture: a lasso drawn around part of a photo, with the shortcut card beside it. */
function LassoPicture() {
  const id = useId();
  const sky = `${id}-sky`;
  const photo = `${id}-photo`;
  return (
    <svg viewBox="0 0 288 165" role="presentation" preserveAspectRatio="xMidYMid slice">
      <defs>
        <linearGradient id={sky} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#e0f2fe" />
          <stop offset="0.55" stopColor="#38bdf8" />
          <stop offset="1" stopColor="#0284c7" />
        </linearGradient>
        <linearGradient id={photo} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#fde68a" />
          <stop offset="1" stopColor="#f59e0b" />
        </linearGradient>
      </defs>
      <rect width="288" height="165" fill={`url(#${sky})`} />
      <rect x="38" y="16" width="118" height="112" rx="6" fill={`url(#${photo})`} />
      <path d="M38 104 l34 -36 l26 24 l18 -14 l40 40 v4 a6 6 0 0 1 -6 6 h-106 a6 6 0 0 1 -6 -6 z" fill="#b45309" fillOpacity="0.55" />
      <circle cx="122" cy="40" r="11" fill="#ffffff" fillOpacity="0.8" />
      <ellipse cx="95" cy="68" rx="52" ry="44" fill="none" stroke="#ffffff" strokeWidth="2.5" />
      <ellipse cx="95" cy="68" rx="52" ry="44" fill="none" stroke="#0ea5e9" strokeWidth="1.2" strokeDasharray="5 4" />
      <g>
        <rect x="128" y="70" width="140" height="80" rx="10" fill="#0f172a" fillOpacity="0.9" />
        <g fill="#ffffff">
          <rect x="140" y="82" width="8" height="8" rx="2" fillOpacity="0.7" />
          <rect x="154" y="84" width="50" height="4" rx="2" fillOpacity="0.75" />
          <circle cx="144" cy="102" r="4" fillOpacity="0.7" />
          <rect x="154" y="100" width="70" height="4" rx="2" fillOpacity="0.75" />
          <rect x="140" y="128" width="64" height="4" rx="2" fillOpacity="0.3" />
          <circle cx="254" cy="130" r="7" />
        </g>
        <path d="M254 134 v-8 m-3 3 l3 -3 l3 3" stroke="#0f172a" strokeWidth="1.5" fill="none" strokeLinecap="round" strokeLinejoin="round" />
      </g>
    </svg>
  );
}

interface Editing {
  kind: Kind;
  /** `null` while adding. */
  id: string | null;
  label: string;
  prompt: string;
}

function ShortcutForm({ editing, onCancel, onSave }: { editing: Editing; onCancel: () => void; onSave: (label: string, prompt: string) => void }) {
  const [label, setLabel] = useState(editing.label);
  const [prompt, setPrompt] = useState(editing.prompt);
  const name = label.trim();
  const instruction = prompt.trim();
  // The protocol counts characters, not UTF-16 units, so maxLength alone is not the limit.
  const valid = name !== "" && instruction !== "" && [...name].length <= MAX_LABEL_CHARS && [...instruction].length <= MAX_PROMPT_CHARS;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (valid) onSave(name, instruction);
  };
  return (
    <form className="dialog-form" onSubmit={submit}>
      <label className="field">
        <span>{t("settings.lasso.dialog.name")}</span>
        <input value={label} maxLength={MAX_LABEL_CHARS * 2} onChange={(event) => setLabel(event.target.value)} required />
      </label>
      <label className="field">
        <span>{t("settings.lasso.dialog.instruction")}</span>
        <textarea
          rows={5}
          value={prompt}
          maxLength={MAX_PROMPT_CHARS * 2}
          placeholder={t("settings.lasso.dialog.instructionPlaceholder")}
          onChange={(event) => setPrompt(event.target.value)}
          required
        />
      </label>
      <DialogActions>
        <Button variant="ghost" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" disabled={!valid}>
          {t("common.save")}
        </Button>
      </DialogActions>
    </form>
  );
}

function ShortcutMenu({ name, disabled, onEdit, onRemove }: { name: string; disabled: boolean; onEdit: () => void; onRemove: () => void }) {
  const label = t("settings.lasso.shortcut.options", { name });
  return (
    <PopoverMenu
      label={label}
      trigger={
        <IconButton label={label} size="small" disabled={disabled}>
          <Ellipsis {...ICON} />
        </IconButton>
      }
    >
      <MenuItem onClick={onEdit}>{t("settings.lasso.shortcut.edit")}</MenuItem>
      <MenuItem onClick={onRemove}>{t("settings.lasso.shortcut.remove")}</MenuItem>
    </PopoverMenu>
  );
}

function ShortcutsCard({
  shortcuts,
  disabled,
  onEdit,
  onRemove,
  onAdd,
}: {
  shortcuts: readonly LassoShortcut[];
  disabled: boolean;
  onEdit: (shortcut: LassoShortcut) => void;
  onRemove: (shortcut: LassoShortcut) => void;
  onAdd: () => void;
}) {
  return (
    <SettingsCard className="lasso-shortcuts">
      {shortcuts.map((shortcut) => {
        const name = shortcutLabel(shortcut);
        return (
          <SettingsRow
            key={shortcut.id}
            leading={<ShortcutIcon id={shortcut.id} />}
            title={name}
            control={<ShortcutMenu name={name} disabled={disabled} onEdit={() => onEdit(shortcut)} onRemove={() => onRemove(shortcut)} />}
          />
        );
      })}
      <button type="button" className="ui-row ui-row--single ui-row--action lasso-add" disabled={disabled || shortcuts.length >= MAX_SHORTCUTS} onClick={onAdd}>
        <span className="ui-row-leading" aria-hidden="true">
          <Plus {...ICON} />
        </span>
        <span className="ui-row-copy">
          <span className="ui-row-title">{t("settings.lasso.shortcut.add")}</span>
        </span>
      </button>
    </SettingsCard>
  );
}

function newShortcutId(): string {
  return `custom-${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;
}

/**
 * Settings › Lasso. The selection dot, the lasso and their hotkey run in the openside extension, which
 * keeps these settings; this page reads and writes them over `externally_connectable`. Without an
 * answering extension the page shows the defaults, disabled, and says where to change them.
 */
export function LassoSettingsPage() {
  const toast = useToast();
  const [state, setState] = useState<Ready | null | undefined>(undefined);
  const [editing, setEditing] = useState<Editing | null>(null);
  // Writes land in order; a reply for an older write must not overwrite a newer choice.
  const latest = useRef(0);

  const load = useCallback(async () => {
    const turn = ++latest.current;
    const reply = await askLasso({ type: "openside.lasso.get" });
    if (turn === latest.current) setState(reply);
  }, []);

  useEffect(() => {
    void load();
    // Granting site access happens on the extension's page in another tab; read again on return.
    const onFocus = () => void load();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [load]);

  const ready = state ?? null;
  const settings: LassoSettings = ready?.settings ?? DEFAULT_LASSO;
  const disabled = ready === null;

  const save = async (next: LassoSettings) => {
    if (!ready) return;
    const previous = ready;
    const turn = ++latest.current;
    setState({ ...ready, settings: next });
    const reply = await askLasso({ type: "openside.lasso.set", settings: next });
    if (turn !== latest.current) return;
    if (reply) {
      setState(reply);
    } else {
      setState(previous);
      toast.notify(t("settings.lasso.saveFailed"), "error");
    }
  };

  const setShortcuts = (kind: Kind, shortcuts: LassoShortcut[]) => void save({ ...settings, [kind]: { ...settings[kind], shortcuts } });

  const saveEditing = (label: string, prompt: string) => {
    if (!editing) return;
    const list = settings[editing.kind].shortcuts;
    const shortcuts = editing.id === null ? [...list, { id: newShortcutId(), label, prompt }] : list.map((entry) => (entry.id === editing.id ? { ...entry, label, prompt } : entry));
    setShortcuts(editing.kind, shortcuts);
    setEditing(null);
  };

  const shortcutsCard = (kind: Kind) => (
    <ShortcutsCard
      shortcuts={settings[kind].shortcuts}
      disabled={disabled}
      onEdit={(shortcut) => setEditing({ kind, id: shortcut.id, label: shortcutLabel(shortcut), prompt: shortcutPrompt(shortcut) })}
      onRemove={(shortcut) =>
        setShortcuts(
          kind,
          settings[kind].shortcuts.filter((entry) => entry.id !== shortcut.id),
        )
      }
      onAdd={() => setEditing({ kind, id: null, label: "", prompt: "" })}
    />
  );

  const hotkeyOptions = LASSO_HOTKEYS.map((hotkey) => {
    const key = hotkeyKey(hotkey);
    return { value: hotkey, label: `${key} + ${key}` };
  });
  const blocked = ready && !ready.access && (settings.text.enabled || settings.region.enabled) ? ready : null;

  return (
    <div className="personal-page lasso-page" data-section="lasso">
      <HeroCard title={t("settings.lasso.heroTitle")} description={t("settings.lasso.heroDescription")} illustration={<LassoPicture />} />
      {state === null && <InfoBanner>{t("settings.lasso.unavailable")}</InfoBanner>}
      {blocked && (
        <InfoBanner
          action={
            <Button size="small" onClick={() => void askLasso({ type: "openside.lasso.access" }).then((reply) => reply && setState(reply))}>
              {t("settings.lasso.access.allow")}
            </Button>
          }
        >
          {t("settings.lasso.access.note")}{" "}
          {blocked.command ? t("settings.lasso.access.fallback", { keys: blocked.command }) : t("settings.lasso.access.fallbackNoKey")}
        </InfoBanner>
      )}
      <SettingsSection label={t("settings.lasso.text.section")}>
        <SettingsCard>
          <SettingsRow
            title={t("settings.lasso.text.enable.title")}
            description={t("settings.lasso.text.enable.description")}
            control={(ids) => (
              <Switch
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                disabled={disabled}
                checked={settings.text.enabled}
                onChange={(enabled) => void save({ ...settings, text: { ...settings.text, enabled } })}
              />
            )}
          />
        </SettingsCard>
        {shortcutsCard("text")}
      </SettingsSection>
      <SettingsSection label={t("settings.lasso.region.section")}>
        <SettingsCard>
          <SettingsRow
            title={t("settings.lasso.region.enable.title")}
            control={(ids) => (
              <Switch
                aria-labelledby={ids.labelId}
                disabled={disabled}
                checked={settings.region.enabled}
                onChange={(enabled) => void save({ ...settings, region: { ...settings.region, enabled } })}
              />
            )}
          />
          <SettingsRow
            title={t("settings.lasso.hotkey.title")}
            description={t("settings.lasso.hotkey.description", { key: hotkeyKey(settings.region.hotkey) })}
            control={(ids) => (
              <Select
                variant="inline"
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                disabled={disabled}
                value={settings.region.hotkey}
                options={hotkeyOptions}
                onChange={(value) => {
                  const hotkey = LASSO_HOTKEYS.find((entry) => entry === value);
                  if (hotkey) void save({ ...settings, region: { ...settings.region, hotkey } });
                }}
              />
            )}
          />
        </SettingsCard>
        {shortcutsCard("region")}
      </SettingsSection>
      <Dialog
        open={editing !== null}
        onClose={() => setEditing(null)}
        title={editing?.id === null ? t("settings.lasso.dialog.addTitle") : t("settings.lasso.dialog.editTitle")}
      >
        {editing && <ShortcutForm key={`${editing.kind}:${editing.id ?? "new"}`} editing={editing} onCancel={() => setEditing(null)} onSave={saveEditing} />}
      </Dialog>
    </div>
  );
}
