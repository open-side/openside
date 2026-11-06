import { Asterisk, BookOpen, FolderPlus, Plus, RotateCw, ShieldCheck, Volume2, X } from "lucide-react";
import { useCallback, useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import type { About, NotificationKind, PolicyCapability, PolicyEffect, PolicyMode, PolicyRule, PolicySettings, Preferences, SettingsDocument } from "../../api/types";
import { COMPLETION_SOUNDS, announcePreferences, loadCompletionSound, playCompletionSound, saveCompletionSound, type CompletionSound } from "../../completionSound";
import { useClient } from "../../context";
import { errorMessage } from "../../hooks/useResource";
import { t, type MessageKey } from "../../i18n";
import { RestartNotice, RestartStatus, useDaemonRestart } from "../../pages/RestartNotice";
import {
  Button,
  Dialog,
  DialogActions,
  Dropdown,
  IconButton,
  ICON_STROKE,
  MenuItem,
  PopoverMenu,
  Select,
  SettingsCard,
  SettingsRow,
  SettingsSection,
  Switch,
  useToast,
} from "../../ui";
import { ComputerUseSettings } from "../ComputerUse";
import { DeviceNotificationsRow } from "./DeviceNotifications";
import "./agents.css";

const KINDS: readonly { kind: NotificationKind; label: MessageKey; detail: MessageKey }[] = [
  { kind: "needs_attention", label: "agentSettings.notifications.needsAttention", detail: "agentSettings.notifications.needsAttention.detail" },
  { kind: "task_completed", label: "agentSettings.notifications.taskCompleted", detail: "agentSettings.notifications.taskCompleted.detail" },
  { kind: "routine_completed", label: "agentSettings.notifications.routineCompleted", detail: "agentSettings.notifications.routineCompleted.detail" },
];

const MODES: readonly { mode: PolicyMode; label: MessageKey; icon: ReactNode }[] = [
  { mode: "read-only", label: "agentSettings.access.readOnly", icon: <BookOpen strokeWidth={ICON_STROKE} /> },
  { mode: "guard", label: "agentSettings.access.guard", icon: <ShieldCheck strokeWidth={ICON_STROKE} /> },
  { mode: "full", label: "agentSettings.access.full", icon: <Asterisk strokeWidth={ICON_STROKE} /> },
];

// The daemon accepts exactly these; 0 (never) is reachable only from the config file, so it is
// listed only while it is the stored value.
const CLEANUP_MINUTES = [5, 10, 15, 30, 60, 120] as const;

const CAPABILITIES: readonly PolicyCapability[] = ["exec", "browse", "fetch", "fs_read", "fs_write", "vault_use", "sensitive", "privacy_read", "person_browser", "computer_use"];

// The daemon refuses any allow or ask rule for `sensitive`; such actions can only be denied outright.
const offeredFor = (effect: PolicyEffect | null, capability: PolicyCapability) => capability !== "sensitive" || effect === "deny";

const EFFECTS: readonly { effect: PolicyEffect; title: MessageKey; description: MessageKey; empty: MessageKey }[] = [
  { effect: "allow", title: "agentSettings.tools.allow", description: "agentSettings.tools.allow.description", empty: "agentSettings.tools.allow.empty" },
  { effect: "ask", title: "agentSettings.tools.ask", description: "agentSettings.tools.ask.description", empty: "agentSettings.tools.ask.empty" },
  { effect: "deny", title: "agentSettings.tools.deny", description: "agentSettings.tools.deny.description", empty: "agentSettings.tools.deny.empty" },
];

const SOUND_LABEL: Record<CompletionSound, MessageKey> = {
  chime: "agentSettings.sound.chime",
  pop: "agentSettings.sound.pop",
  bell: "agentSettings.sound.bell",
};

function cleanupLabel(minutes: number): string {
  if (minutes === 0) return t("agentSettings.cleanup.never");
  if (minutes === 60) return t("agentSettings.cleanup.hour");
  if (minutes % 60 === 0) return t("agentSettings.cleanup.hours", { count: minutes / 60 });
  return t("agentSettings.cleanup.minutes", { count: minutes });
}

interface PreferenceStore {
  /** Null until the first load. */
  preferences: Preferences | null;
  update: (patch: Partial<Preferences>) => Promise<void>;
}

interface PolicyStore {
  /** Null until the first load. */
  policy: PolicySettings | null;
  /** Resolves false when validation or the save failed; the store has rolled back and said why. */
  save: (change: (current: PolicySettings) => PolicySettings) => Promise<boolean>;
}

/** Optimistic preference writes: the control moves at once and snaps back if the daemon refuses. */
function usePreferences(): PreferenceStore {
  const client = useClient();
  const toast = useToast();
  const [preferences, setPreferences] = useState<Preferences | null>(null);
  const latest = useRef(preferences);
  latest.current = preferences;

  useEffect(() => {
    let alive = true;
    client.preferences().then(
      (loaded) => alive && setPreferences(loaded),
      (cause: unknown) => alive && toast.notify(errorMessage(cause), "error"),
    );
    return () => {
      alive = false;
    };
  }, [client, toast]);

  const update = useCallback(
    async (patch: Partial<Preferences>) => {
      const before = latest.current;
      if (!before) return;
      setPreferences({ ...before, ...patch });
      try {
        const saved = await client.updatePreferences(patch);
        setPreferences(saved);
        announcePreferences(saved);
      } catch (cause) {
        setPreferences(before);
        toast.notify(errorMessage(cause), "error");
      }
    },
    [client, toast],
  );
  return { preferences, update };
}

/**
 * The live policy behind Access permission, File permissions and Tool permissions. Every save sends
 * the whole document back, origin lists included (Settings › Security owns those), after the
 * daemon's own validation.
 */
function usePolicy(): PolicyStore {
  const client = useClient();
  const toast = useToast();
  const [policy, setPolicy] = useState<PolicySettings | null>(null);
  const latest = useRef(policy);
  latest.current = policy;

  useEffect(() => {
    let alive = true;
    client.policy().then(
      (loaded) => alive && setPolicy(loaded),
      (cause: unknown) => alive && toast.notify(errorMessage(cause), "error"),
    );
    return () => {
      alive = false;
    };
  }, [client, toast]);

  const save = useCallback(
    async (change: (current: PolicySettings) => PolicySettings): Promise<boolean> => {
      const before = latest.current;
      if (!before) return false;
      const next = change(before);
      setPolicy(next);
      try {
        const check = await client.validatePolicy({ policy: next });
        if (!check.valid) {
          setPolicy(before);
          toast.notify(t("agentSettings.policy.saveFailed", { reason: check.problems.map((problem) => problem.message).join(" ") }), "error");
          return false;
        }
        setPolicy(await client.setPolicy(next));
        return true;
      } catch (cause) {
        setPolicy(before);
        toast.notify(t("agentSettings.policy.saveFailed", { reason: errorMessage(cause) }), "error");
        return false;
      }
    },
    [client, toast],
  );
  return { policy, save };
}

/** Settings › Agents (vm2/agents-0/1): how chats behave, then what agents may touch. */
export function AgentsSettings() {
  const preferences = usePreferences();
  const policy = usePolicy();
  return (
    <div className="agents-page">
      <SettingsSection label={t("agentSettings.chat")}>
        <SettingsCard>
          <NotificationsRow store={preferences} />
          <SoundRow store={preferences} />
          <DeviceNotificationsRow />
        </SettingsCard>
        <SettingsCard>
          <AccessRow store={policy} />
          <FollowUpRow store={preferences} />
          <TabSwitchRow />
          <CleanupRow store={preferences} />
        </SettingsCard>
      </SettingsSection>
      <ComputerUseSettings />
      <SandboxSection />
      <FilePermissions store={policy} />
      <ToolPermissions store={policy} />
      <AgentRuntime />
    </div>
  );
}

function NotificationsRow({ store: { preferences, update } }: { store: PreferenceStore }) {
  const chosen = preferences?.notifications ?? [];
  const summary =
    chosen.length === KINDS.length ? t("agentSettings.notifications.everything") : chosen.length === 0 ? t("agentSettings.notifications.off") : undefined;
  return (
    <SettingsRow
      title={t("agentSettings.notifications.title")}
      description={t("agentSettings.notifications.description")}
      control={({ labelId, descriptionId }) => (
        <Dropdown
          multiple
          variant="inline"
          aria-labelledby={labelId}
          aria-describedby={descriptionId}
          disabled={!preferences}
          summary={summary}
          value={chosen}
          options={KINDS.map(({ kind, label, detail }) => ({ value: kind, label: t(label), detail: t(detail) }))}
          // Keep the daemon's order so a save never reads as a change of its own.
          onChange={(values) => void update({ notifications: KINDS.map(({ kind }) => kind).filter((kind) => values.includes(kind)) })}
        />
      )}
    />
  );
}

function SoundRow({ store: { preferences, update } }: { store: PreferenceStore }) {
  const [sound, setSound] = useState<CompletionSound>(loadCompletionSound);
  return (
    <div className="agents-sound">
      <SettingsRow
        title={t("agentSettings.sound.title")}
        description={t("agentSettings.sound.description")}
        control={({ labelId, descriptionId }) => (
          <Switch
            aria-labelledby={labelId}
            aria-describedby={descriptionId}
            disabled={!preferences}
            checked={preferences?.completion_sound ?? false}
            onChange={(on) => void update({ completion_sound: on })}
          />
        )}
      />
      <div className="agents-sound-actions">
        <Button size="small" onClick={() => playCompletionSound(sound)}>
          <Volume2 strokeWidth={ICON_STROKE} aria-hidden="true" />
          {t("agentSettings.sound.preview")}
        </Button>
        <PopoverMenu
          label={t("agentSettings.sound.changeLabel")}
          trigger={<Button size="small">{t("agentSettings.sound.change")}</Button>}
        >
          {COMPLETION_SOUNDS.map((choice) => (
            <MenuItem
              key={choice}
              checked={choice === sound}
              onClick={() => {
                saveCompletionSound(choice);
                setSound(choice);
                playCompletionSound(choice);
              }}
            >
              {t(SOUND_LABEL[choice])}
            </MenuItem>
          ))}
        </PopoverMenu>
      </div>
    </div>
  );
}

function AccessRow({ store: { policy, save } }: { store: PolicyStore }) {
  return (
    <SettingsRow
      title={t("agentSettings.access.title")}
      description={t("agentSettings.access.description")}
      control={({ labelId, descriptionId }) => (
        <Select
          variant="inline"
          aria-labelledby={labelId}
          aria-describedby={descriptionId}
          disabled={!policy}
          value={policy?.default_mode ?? "guard"}
          options={MODES.map(({ mode, label, icon }) => ({ value: mode, label: t(label), icon }))}
          onChange={(mode) => {
            const chosen = MODES.find((entry) => entry.mode === mode)?.mode;
            if (chosen) void save((current) => ({ ...current, default_mode: chosen }));
          }}
        />
      )}
    />
  );
}

function FollowUpRow({ store: { preferences, update } }: { store: PreferenceStore }) {
  return (
    <SettingsRow
      title={t("agentSettings.followUp.title")}
      description={t("agentSettings.followUp.description")}
      control={({ labelId, descriptionId }) => (
        <Select
          variant="inline"
          aria-labelledby={labelId}
          aria-describedby={descriptionId}
          disabled={!preferences}
          value={preferences?.follow_up ?? "queue"}
          options={[
            { value: "queue", label: t("agentSettings.followUp.queue") },
            { value: "steer", label: t("agentSettings.followUp.steer") },
          ]}
          onChange={(mode) => void update({ follow_up: mode === "steer" ? "steer" : "queue" })}
        />
      )}
    />
  );
}

/**
 * The side panel keeps one chat per tab (extension storage `sessions:<tabId>`); following one chat
 * across tabs has no engine yet, so that choice is shown but cannot be picked (D6).
 */
function TabSwitchRow() {
  return (
    <SettingsRow
      title={t("agentSettings.tabSwitch.title")}
      description={t("agentSettings.tabSwitch.description")}
      control={({ labelId, descriptionId }) => (
        <Select
          variant="inline"
          aria-labelledby={labelId}
          aria-describedby={descriptionId}
          value="new"
          options={[
            { value: "new", label: t("agentSettings.tabSwitch.newChat") },
            { value: "keep", label: t("agentSettings.tabSwitch.keepChat"), disabled: true },
          ]}
          onChange={() => undefined}
        />
      )}
    />
  );
}

function CleanupRow({ store: { preferences, update } }: { store: PreferenceStore }) {
  const current = preferences?.agent_tab_cleanup_minutes ?? 15;
  const choices: number[] = CLEANUP_MINUTES.some((minutes) => minutes === current) ? [...CLEANUP_MINUTES] : [current, ...CLEANUP_MINUTES];
  return (
    <SettingsRow
      title={t("agentSettings.cleanup.title")}
      description={t("agentSettings.cleanup.description")}
      control={({ labelId, descriptionId }) => (
        <Select
          variant="inline"
          aria-labelledby={labelId}
          aria-describedby={descriptionId}
          disabled={!preferences}
          value={String(current)}
          options={choices.map((minutes) => ({ value: String(minutes), label: cleanupLabel(minutes) }))}
          onChange={(value) => void update({ agent_tab_cleanup_minutes: Number(value) })}
        />
      )}
    />
  );
}

function sandboxRequired(config: unknown): boolean {
  if (!config || typeof config !== "object" || !("sandbox" in config)) return false;
  const sandbox = config.sandbox;
  return Boolean(sandbox && typeof sandbox === "object" && "required" in sandbox && sandbox.required === true);
}

function withSandbox(config: unknown, required: boolean): Record<string, unknown> {
  const base: Record<string, unknown> = config && typeof config === "object" ? { ...config } : {};
  const sandbox = base.sandbox && typeof base.sandbox === "object" ? base.sandbox : {};
  return { ...base, sandbox: { ...sandbox, required } };
}

/**
 * `sandbox.required` in the configuration file: on, commands run only inside the sandbox and are
 * refused where none is available. It needs this computer's authority and applies on restart.
 */
function SandboxSection() {
  const client = useClient();
  const toast = useToast();
  const [document, setDocument] = useState<SettingsDocument | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    client.settingsConfig().then(
      (loaded) => alive && setDocument(loaded),
      (cause: unknown) => alive && toast.notify(errorMessage(cause), "error"),
    );
    return () => {
      alive = false;
    };
  }, [client, toast]);

  const remote = document?.remote === true && !document.remote_editable.some((key) => key === "sandbox." || key === "sandbox.required");
  async function change(required: boolean) {
    if (!document) return;
    setBusy(true);
    try {
      setDocument(await client.updateSettingsConfig(withSandbox(document.config, required)));
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsSection label={t("agentSettings.sandbox.title")}>
      <SettingsCard>
        <SettingsRow
          title={t("agentSettings.sandbox.row")}
          description={remote ? `${t("agentSettings.sandbox.description")} ${t("agentSettings.sandbox.remote")}` : t("agentSettings.sandbox.description")}
          control={({ labelId, descriptionId }) => (
            <Switch
              aria-labelledby={labelId}
              aria-describedby={descriptionId}
              disabled={!document || remote || busy}
              checked={sandboxRequired(document?.config)}
              onChange={(on) => void change(on)}
            />
          )}
        />
      </SettingsCard>
      {document?.restart_required ? <RestartNotice>{t("agentSettings.sandbox.restart")}</RestartNotice> : null}
    </SettingsSection>
  );
}

// A folder rule as the policy stores it: everything under the folder.
const folderOf = (pattern: string) => pattern.replace(/\/\*\*$/, "");
const isFolderRule = (rule: PolicyRule) => rule.effect === "allow" && (rule.capability === "fs_read" || rule.capability === "fs_write");

function FilePermissions({ store }: { store: PolicyStore }) {
  const [adding, setAdding] = useState<"fs_read" | "fs_write" | null>(null);
  const rules = store.policy?.rules ?? [];
  const list = (capability: "fs_read" | "fs_write") => rules.filter((rule) => isFolderRule(rule) && rule.capability === capability);
  const remove = (target: PolicyRule) => void store.save((current) => ({ ...current, rules: current.rules.filter((rule) => rule !== target && !sameRule(rule, target)) }));

  const block = (capability: "fs_read" | "fs_write") => {
    const view = capability === "fs_read";
    return (
      <>
        <SettingsRow
          title={t(view ? "agentSettings.files.view.title" : "agentSettings.files.edit.title")}
          description={t(view ? "agentSettings.files.view.description" : "agentSettings.files.edit.description")}
          control={
            <Button disabled={!store.policy} onClick={() => setAdding(capability)}>
              <FolderPlus strokeWidth={ICON_STROKE} aria-hidden="true" />
              {t("agentSettings.files.add")}
            </Button>
          }
        />
        <div className="agents-folder">
          <span className="agents-folder-name">{t("agentSettings.files.chatFolder")}</span>
          <span className="agents-folder-path">{t("agentSettings.files.chatFolder.detail")}</span>
          <span className="agents-default">{t("agentSettings.files.default")}</span>
        </div>
        {list(capability).map((rule) => (
          <div className="agents-folder" key={`${rule.capability}:${rule.pattern}`}>
            <span className="agents-folder-path" title={rule.pattern}>
              {folderOf(rule.pattern)}
            </span>
            <IconButton size="small" label={t("agentSettings.files.remove", { folder: folderOf(rule.pattern) })} onClick={() => remove(rule)}>
              <X strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
          </div>
        ))}
      </>
    );
  };

  return (
    <SettingsSection label={t("agentSettings.files.title")}>
      <SettingsCard>
        {block("fs_read")}
        {block("fs_write")}
      </SettingsCard>
      <FolderDialog
        capability={adding}
        onClose={() => setAdding(null)}
        onAdd={async (folder) => {
          const capability = adding;
          if (!capability) return false;
          return store.save((current) => ({ ...current, rules: [...current.rules, { effect: "allow", capability, pattern: `${folder.trim().replace(/\/+$/, "")}/**` }] }));
        }}
      />
    </SettingsSection>
  );
}

const sameRule = (a: PolicyRule, b: PolicyRule) => a.effect === b.effect && a.capability === b.capability && a.pattern === b.pattern;

function FolderDialog({ capability, onClose, onAdd }: { capability: "fs_read" | "fs_write" | null; onClose: () => void; onAdd: (folder: string) => Promise<boolean> }) {
  const [folder, setFolder] = useState("");
  const [busy, setBusy] = useState(false);
  const fieldId = useId();
  useEffect(() => {
    if (capability) setFolder("");
  }, [capability]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    const added = await onAdd(folder);
    setBusy(false);
    if (added) onClose();
  }

  return (
    <Dialog
      open={capability !== null}
      onClose={onClose}
      title={t(capability === "fs_write" ? "agentSettings.files.dialog.edit" : "agentSettings.files.dialog.view")}
      description={t("agentSettings.files.dialog.description")}
    >
      <form className="agents-form" onSubmit={(event) => void submit(event)}>
        <label htmlFor={fieldId}>
          {t("agentSettings.files.dialog.folder")}
          <input id={fieldId} value={folder} placeholder="/home/me/projects" spellCheck={false} autoCapitalize="none" onChange={(event) => setFolder(event.target.value)} />
        </label>
        <DialogActions>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button type="submit" variant="primary" disabled={busy || !folder.trim()}>
            {busy ? t("common.saving") : t("agentSettings.files.add")}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

const capabilityLabel = (capability: PolicyCapability) => t(`agentSettings.capability.${capability}`);

function ToolPermissions({ store }: { store: PolicyStore }) {
  const [adding, setAdding] = useState<PolicyEffect | null>(null);
  const rules = (store.policy?.rules ?? []).filter((rule) => !isFolderRule(rule));
  return (
    <SettingsSection label={t("agentSettings.tools.title")} info={t("agentSettings.tools.info")}>
      <SettingsCard>
        {EFFECTS.map(({ effect, title, description, empty }) => {
          const matching = rules.filter((rule) => rule.effect === effect);
          return (
            <div className="agents-rules" key={effect} role="group" aria-label={t(title)}>
              <span className="agents-rules-effect">{t(title)}</span>
              <span className="agents-rules-description">{t(description)}</span>
              <Button variant="ghost" disabled={!store.policy} aria-label={t("agentSettings.tools.addRuleFor", { effect: t(title) })} onClick={() => setAdding(effect)}>
                <Plus strokeWidth={ICON_STROKE} aria-hidden="true" />
                {t("agentSettings.tools.addRule")}
              </Button>
              <ul className="agents-rules-list">
                {matching.length === 0 ? (
                  <li className="agents-rules-empty">{t(empty)}</li>
                ) : (
                  matching.map((rule) => {
                    const name = `${capabilityLabel(rule.capability)} ${rule.pattern}`;
                    return (
                      <li className="agents-rule" key={`${rule.capability}:${rule.pattern}`}>
                        <span>{capabilityLabel(rule.capability)}</span>
                        <span className="agents-rule-pattern">{rule.pattern}</span>
                        <IconButton
                          size="small"
                          label={t("agentSettings.tools.removeRule", { rule: name })}
                          onClick={() => void store.save((current) => ({ ...current, rules: current.rules.filter((other) => !sameRule(other, rule)) }))}
                        >
                          <X strokeWidth={ICON_STROKE} aria-hidden="true" />
                        </IconButton>
                      </li>
                    );
                  })
                )}
              </ul>
            </div>
          );
        })}
      </SettingsCard>
      <RuleDialog
        effect={adding}
        onClose={() => setAdding(null)}
        onAdd={(rule) => store.save((current) => ({ ...current, rules: [...current.rules, rule] }))}
      />
    </SettingsSection>
  );
}

function RuleDialog({ effect, onClose, onAdd }: { effect: PolicyEffect | null; onClose: () => void; onAdd: (rule: PolicyRule) => Promise<boolean> }) {
  const [capability, setCapability] = useState<PolicyCapability>("exec");
  const [pattern, setPattern] = useState("**");
  const [busy, setBusy] = useState(false);
  const patternId = useId();
  const actionId = useId();
  useEffect(() => {
    if (!effect) return;
    setCapability("exec");
    setPattern("**");
  }, [effect]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!effect) return;
    setBusy(true);
    const added = await onAdd({ effect, capability, pattern: pattern.trim() });
    setBusy(false);
    if (added) onClose();
  }

  const title = EFFECTS.find((entry) => entry.effect === effect)?.title;
  return (
    <Dialog open={effect !== null} onClose={onClose} title={t("agentSettings.tools.addRuleFor", { effect: title ? t(title) : "" })}>
      <form className="agents-form" onSubmit={(event) => void submit(event)}>
        <label id={actionId}>{t("agentSettings.tools.dialog.action")}</label>
        <Select
          aria-labelledby={actionId}
          value={capability}
          options={CAPABILITIES.filter((value) => offeredFor(effect, value)).map((value) => ({ value, label: capabilityLabel(value) }))}
          onChange={(value) => setCapability(CAPABILITIES.find((entry) => entry === value) ?? "exec")}
        />
        <label htmlFor={patternId}>
          {t("agentSettings.tools.dialog.pattern")}
          <input id={patternId} value={pattern} spellCheck={false} autoCapitalize="none" onChange={(event) => setPattern(event.target.value)} />
        </label>
        <p className="agents-note">{t("agentSettings.tools.dialog.patternHint")}</p>
        <DialogActions>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button type="submit" variant="primary" disabled={busy || !pattern.trim()}>
            {busy ? t("common.saving") : t("agentSettings.tools.addRule")}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

/**
 * The runtime ships with openside and the daemon starts it on demand. There is no reinstall
 * endpoint, so Aside's Reinstall becomes its nearest working behaviour: restart the background
 * service, which starts a fresh runtime process (D6).
 */
function AgentRuntime() {
  const client = useClient();
  const [about, setAbout] = useState<About | null>(null);
  const restart = useDaemonRestart();

  useEffect(() => {
    let alive = true;
    client.about().then(
      (loaded) => alive && setAbout(loaded),
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, [client]);

  return (
    <SettingsSection label={t("agentSettings.runtime.title")} info={t("agentSettings.runtime.info")}>
      <SettingsCard>
        <SettingsRow
          title={t("agentSettings.runtime.version.title")}
          description={about?.runtime_version ? t("agentSettings.runtime.version.description", { version: about.runtime_version }) : t("agentSettings.runtime.version.pending")}
        />
        <SettingsRow
          title={t("agentSettings.runtime.restart.title")}
          description={t("agentSettings.runtime.restart.description")}
          control={
            <Button variant="primary" disabled={restart.phase === "restarting"} onClick={restart.ask}>
              <RotateCw strokeWidth={ICON_STROKE} aria-hidden="true" />
              {t("agentSettings.runtime.restart.button")}
            </Button>
          }
        />
      </SettingsCard>
      <RestartStatus restart={restart} />
    </SettingsSection>
  );
}
