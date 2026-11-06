import { ChevronDown, Copy, FolderCog, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useId, useState } from "react";
import { ApiError } from "../../api/client";
import type { CliDefaults, CliProfile, ExternalAgent, ExternalAgentList, Preferences } from "../../api/types";
import { announcePreferences } from "../../completionSound";
import { useClient } from "../../context";
import { errorMessage, useResource, type Resource } from "../../hooks/useResource";
import { t, type MessageKey } from "../../i18n";
import { tNodes } from "../../i18n/react";
import { AgentFolderDialog } from "../../pages/AgentFolderDialog";
import { AgentLink } from "../../pages/AgentLink";
import { ErrorNote, useConfirm, useDialogState } from "../../pages/blocks";
import { ScopeNotice } from "../../pages/ScopeNotice";
import { LocalOnlyNotice } from "../../remote/LocalOnlyNotice";
import { HelpLink, RemoteControlSection } from "../../remote/RemoteAccess";
import {
  Button,
  copyText,
  Dialog,
  DialogActions,
  HeroCard,
  ICON_STROKE,
  MenuGroupLabel,
  MenuItem,
  MenuSeparator,
  PopoverMenu,
  Select,
  SettingsCard,
  SettingsRow,
  SettingsSection,
  SubmenuItem,
  Switch,
  useToast,
} from "../../ui";
import { REPOSITORY } from "../nav";
import "./developers.css";

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** What `openside skills install` does with no agent named: the skill for every agent found here. */
const SETUP_COMMAND = "openside skills install";
const CLI_GUIDE = `${REPOSITORY}/blob/master/docs/reference/cli.md`;
/** The `mcp.json` entry docs/specs/mcp.md gives; with no `--url` the CLI talks to this computer's socket. */
const MCP_CONFIG = JSON.stringify({ mcpServers: { openside: { command: "openside", args: ["mcp"] } } }, null, 2);

// `default` stands for no saved profile (contract C2: PATCH `profile: null`).
const DEFAULT_PROFILE = "default";
const PROFILES: readonly { value: CliProfile; label: MessageKey; detail: MessageKey }[] = [
  { value: "shared", label: "settings.developers.profile.shared", detail: "settings.developers.profile.shared.detail" },
  { value: "isolated", label: "settings.developers.profile.isolated", detail: "settings.developers.profile.isolated.detail" },
  { value: "incognito", label: "settings.developers.profile.incognito", detail: "settings.developers.profile.incognito.detail" },
];

const FAQS: readonly { id: string; question: MessageKey; answer: MessageKey }[] = [
  { id: "cli", question: "settings.developers.faq.cli.question", answer: "settings.developers.faq.cli.answer" },
  { id: "agents", question: "settings.developers.faq.agents.question", answer: "settings.developers.faq.agents.answer" },
  { id: "remote", question: "settings.developers.faq.remote.question", answer: "settings.developers.faq.remote.answer" },
  { id: "elsewhere", question: "settings.developers.faq.elsewhere.question", answer: "settings.developers.faq.elsewhere.answer" },
  { id: "mcp", question: "settings.developers.faq.mcp.question", answer: "settings.developers.faq.mcp.answer" },
];

/** openside's own picture: a terminal handing a task to openside over a cool gradient. */
function TerminalArt() {
  const id = useId();
  const glow = `${id}-glow`;
  return (
    <svg viewBox="0 0 288 165" role="presentation" preserveAspectRatio="xMidYMid slice">
      <defs>
        <linearGradient id={glow} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#5eead4" />
          <stop offset="0.55" stopColor="#38bdf8" />
          <stop offset="1" stopColor="#818cf8" />
        </linearGradient>
      </defs>
      <rect width="288" height="165" fill={`url(#${glow})`} />
      <rect x="26" y="24" width="262" height="141" rx="10" fill="#0b1120" />
      <g fill="#ffffff" fillOpacity="0.35">
        <circle cx="40" cy="37" r="3.5" />
        <circle cx="52" cy="37" r="3.5" />
        <circle cx="64" cy="37" r="3.5" />
      </g>
      <g fontFamily="ui-monospace, SFMono-Regular, Menlo, monospace" fontSize="8.5">
        <text x="40" y="66" fill="#5eead4">$</text>
        <text x="50" y="66" fill="#e2e8f0">
          openside exec "find a table for two tonight"
        </text>
        <text x="40" y="84" fill="#94a3b8">
          ● Opening the booking site in your profile…
        </text>
        <text x="40" y="99" fill="#94a3b8">
          ● Waiting for your approval on the phone
        </text>
        <text x="40" y="114" fill="#a5f3fc">
          ✓ Booked: 19:30, two people
        </text>
        <text x="40" y="136" fill="#5eead4">$</text>
      </g>
      <rect x="50" y="129" width="5" height="9" fill="#e2e8f0" fillOpacity="0.8" />
    </svg>
  );
}

/**
 * The CLI ships with openside, so there is nothing of its own to reinstall (D6). Reinstall puts a
 * fresh browser skill into every agent that already has one, or every agent found on this
 * computer when none has it yet.
 */
function CliHero({ agents }: { agents: Resource<ExternalAgentList> }) {
  const client = useClient();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const list = agents.data?.agents ?? null;

  async function reinstall(all: readonly ExternalAgent[]) {
    const installed = all.filter((agent) => agent.skill_installed && agent.skills_dir);
    const targets = installed.length > 0 ? installed : all.filter((agent) => agent.detected && agent.skills_dir);
    if (targets.length === 0) {
      toast.notify(t("settings.developers.cli.noAgents"), "error");
      return;
    }
    setBusy(true);
    const done: string[] = [];
    try {
      for (const agent of targets) {
        try {
          await client.installAgentSkill(agent.id, true);
          done.push(agent.label);
        } catch (cause) {
          toast.notify(t("settings.developers.cli.reinstallFailed", { name: agent.label, reason: errorMessage(cause) }), "error");
        }
      }
      if (done.length > 0) toast.notify(t("settings.developers.cli.reinstalled", { names: done.join(", ") }));
    } finally {
      setBusy(false);
      await agents.reload();
    }
  }

  async function copySetup() {
    const copied = await copyText(SETUP_COMMAND);
    toast.notify(t(copied ? "settings.developers.cli.setupCopied" : "settings.developers.cli.copyFailed"), copied ? "success" : "error");
  }

  return (
    <HeroCard
      title={t("settings.developers.cli.title")}
      description={t("settings.developers.cli.description")}
      illustration={<TerminalArt />}
      action={
        <span className="dev-split">
          <Button variant="primary" disabled={busy || list === null} title={t("settings.developers.cli.reinstallHint")} onClick={() => list && void reinstall(list)}>
            {busy ? t("settings.developers.cli.reinstalling") : t("settings.developers.cli.reinstall")}
          </Button>
          <PopoverMenu
            label={t("settings.developers.cli.more")}
            align="start"
            trigger={
              <Button variant="primary" className="dev-split-more" aria-label={t("settings.developers.cli.more")}>
                <ChevronDown strokeWidth={ICON_STROKE} aria-hidden="true" />
              </Button>
            }
          >
            <MenuItem icon={<Copy {...ICON} />} onClick={() => void copySetup()}>
              {t("settings.developers.cli.copySetup")}
            </MenuItem>
            <MenuItem onClick={() => void window.open(CLI_GUIDE, "_blank", "noopener,noreferrer")}>{t("settings.developers.cli.guide")}</MenuItem>
          </PopoverMenu>
        </span>
      }
    />
  );
}

type SwitchKey = "save_sessions" | "mcp_server";

interface Switches {
  /** Null until the first load. */
  preferences: Preferences | null;
  /** The switch whose save is in flight; it stays disabled until the answer lands. */
  saving: SwitchKey | null;
  save: (key: SwitchKey, value: boolean) => Promise<void>;
  /** The daemon refused the MCP switch because this browser lacks the computer's local access. */
  mcpRefused: boolean;
}

/** The two switches this page owns; each save rolls back its own field when the daemon refuses. */
function usePreferenceSwitches(): Switches {
  const client = useClient();
  const toast = useToast();
  const [preferences, setPreferences] = useState<Preferences | null>(null);
  const [saving, setSaving] = useState<SwitchKey | null>(null);
  const [mcpRefused, setMcpRefused] = useState(false);

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

  const save = useCallback(
    async (key: SwitchKey, value: boolean) => {
      setSaving(key);
      setPreferences((current) => current && { ...current, [key]: value });
      try {
        const saved = await client.updatePreferences({ [key]: value });
        setPreferences(saved);
        announcePreferences(saved);
        if (key === "mcp_server") setMcpRefused(false);
      } catch (cause) {
        setPreferences((current) => current && { ...current, [key]: !value });
        if (key === "mcp_server" && cause instanceof ApiError && cause.code === "local_access_required") setMcpRefused(true);
        else toast.notify(errorMessage(cause), "error");
      } finally {
        setSaving(null);
      }
    },
    [client, toast],
  );
  return { preferences, saving, save, mcpRefused };
}

/** Contract C2: the browser profile new CLI and MCP sessions on this computer use. */
function DefaultProfileRow({ onRefused }: { onRefused: (refused: boolean) => void }) {
  const client = useClient();
  const toast = useToast();
  const [defaults, setDefaults] = useState<CliDefaults | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    client.cliDefaults().then(
      (loaded) => alive && setDefaults(loaded),
      (cause: unknown) => alive && toast.notify(errorMessage(cause), "error"),
    );
    return () => {
      alive = false;
    };
  }, [client, toast]);

  async function choose(value: string) {
    if (!defaults) return;
    const before = defaults;
    const profile = value === DEFAULT_PROFILE ? null : (value as CliProfile);
    setDefaults({ profile: profile ?? "shared", profile_is_default: profile === null });
    setSaving(true);
    try {
      setDefaults(await client.updateCliDefaults({ profile }));
      onRefused(false);
    } catch (cause) {
      setDefaults(before);
      if (cause instanceof ApiError && cause.code === "local_access_required") onRefused(true);
      else toast.notify(errorMessage(cause), "error");
    } finally {
      setSaving(false);
    }
  }

  return (
    <SettingsRow
      title={t("settings.developers.profile.title")}
      description={t("settings.developers.profile.description")}
      control={(ids) => (
        <Select
          variant="inline"
          aria-labelledby={ids.labelId}
          aria-describedby={ids.descriptionId}
          disabled={!defaults || saving}
          value={!defaults ? "" : defaults.profile_is_default ? DEFAULT_PROFILE : defaults.profile}
          options={[
            { value: DEFAULT_PROFILE, label: t("settings.developers.profile.default") },
            ...PROFILES.map((profile) => ({ value: profile.value, label: t(profile.label), detail: t(profile.detail) })),
          ]}
          onChange={(value) => void choose(value)}
        />
      )}
    />
  );
}

function SettingsGroup({ switches }: { switches: Switches }) {
  const [profileRefused, setProfileRefused] = useState(false);
  const { preferences, saving, save } = switches;
  return (
    <SettingsSection label={t("settings.developers.settings.label")}>
      <SettingsCard>
        <SettingsRow
          title={t("settings.developers.saveSessions.title")}
          description={t("settings.developers.saveSessions.description")}
          control={(ids) => (
            <Switch
              aria-labelledby={ids.labelId}
              aria-describedby={ids.descriptionId}
              disabled={!preferences || saving === "save_sessions"}
              checked={preferences?.save_sessions === true}
              onChange={(checked) => void save("save_sessions", checked)}
            />
          )}
        />
        <DefaultProfileRow onRefused={setProfileRefused} />
      </SettingsCard>
      {profileRefused && <LocalOnlyNotice what={t("settings.developers.profile.localOnly")} onChanged={() => setProfileRefused(false)} />}
    </SettingsSection>
  );
}

/** "Add skill to…": every coding agent with its install state, plus adding, moving or forgetting one. */
function SkillMenu({ agents }: { agents: Resource<ExternalAgentList> }) {
  const client = useClient();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const folder = useDialogState<{ agent?: ExternalAgent }>();
  const [confirm, ask] = useConfirm();
  const list = agents.data?.agents ?? null;
  const custom = list?.filter((agent) => agent.custom) ?? [];

  async function install(agent: ExternalAgent) {
    setBusy(true);
    try {
      // An installed skill is replaced with the bundled one, so a check mark never hides a stale copy.
      await client.installAgentSkill(agent.id, agent.skill_installed);
      toast.notify(
        agent.skill_installed ? t("settings.developers.cli.reinstalled", { names: agent.label }) : t("settings.developers.skills.installed", { name: agent.label }),
      );
    } catch (cause) {
      // Installed by another route while the menu was open: the reloaded list is the truth.
      if (!(cause instanceof ApiError && cause.code === "exists")) toast.notify(errorMessage(cause), "error");
    } finally {
      setBusy(false);
      await agents.reload();
    }
  }

  return (
    <>
      <PopoverMenu
        label={t("settings.developers.skills.menu")}
        align="end"
        trigger={
          <Button disabled={list === null || busy}>
            {busy ? t("agents.coding.installing") : t("settings.developers.skills.add")}
            <ChevronDown strokeWidth={ICON_STROKE} aria-hidden="true" />
          </Button>
        }
      >
        {(list ?? []).map((agent) => (
          <MenuItem
            key={agent.id}
            checked={agent.skill_installed}
            checkRole="checkbox"
            disabled={!agent.skills_dir}
            textValue={agent.label}
            description={agent.skills_dir ? t("agents.coding.skillsFolder", { path: agent.skills_dir }) : t("agents.coding.unknownFolder")}
            trailing={agent.detected ? undefined : <span className="dev-menu-note">{t("agents.coding.badge.notFound")}</span>}
            onClick={() => void install(agent)}
          >
            {agent.label}
          </MenuItem>
        ))}
        <MenuSeparator />
        <MenuItem icon={<Plus {...ICON} />} onClick={() => folder.show({})}>
          {t("settings.developers.skills.addAnother")}
        </MenuItem>
        {custom.length > 0 && (
          <>
            <MenuSeparator />
            <MenuGroupLabel>{t("settings.developers.skills.addedAgents")}</MenuGroupLabel>
            {custom.map((agent) => (
              <SubmenuItem key={agent.id} label={agent.label}>
                <MenuItem icon={<FolderCog {...ICON} />} onClick={() => folder.show({ agent })}>
                  {t("agents.coding.changeFolder")}
                </MenuItem>
                <MenuItem
                  danger
                  icon={<Trash2 {...ICON} />}
                  onClick={() =>
                    ask({
                      title: t("agents.coding.forgetTitle", { name: agent.label }),
                      description: t("agents.coding.forgetDescription"),
                      confirmLabel: t("agents.coding.forget"),
                      onConfirm: () =>
                        void client.deleteExternalAgent(agent.id).then(
                          () => agents.reload(),
                          (cause: unknown) => toast.notify(errorMessage(cause), "error"),
                        ),
                    })
                  }
                >
                  {t("agents.coding.forgetAgent")}
                </MenuItem>
              </SubmenuItem>
            ))}
          </>
        )}
      </PopoverMenu>
      {folder.value ? (
        <AgentFolderDialog
          key={folder.value.agent?.id ?? "new"}
          open={folder.open}
          agent={folder.value.agent}
          onClose={folder.close}
          onSaved={(id) => {
            folder.close();
            toast.notify(folder.value?.agent ? t("agents.coding.folderSaved") : t("agents.coding.added", { id }));
            void agents.reload();
          }}
        />
      ) : null}
      {confirm}
    </>
  );
}

function SkillAndMcp({ agents, switches }: { agents: Resource<ExternalAgentList>; switches: Switches }) {
  const toast = useToast();
  const [mcpHelp, setMcpHelp] = useState(false);
  const [agentLink, setAgentLink] = useState(false);
  const { preferences, saving, save, mcpRefused } = switches;

  async function copyConfig() {
    const copied = await copyText(MCP_CONFIG);
    toast.notify(t(copied ? "settings.developers.mcp.copied" : "settings.developers.cli.copyFailed"), copied ? "success" : "error");
  }

  return (
    <SettingsSection label={t("settings.developers.skills.label")}>
      <SettingsCard>
        <SettingsRow title={t("settings.developers.skills.title")} description={t("settings.developers.skills.description")} control={<SkillMenu agents={agents} />} />
        <SettingsRow
          title={t("settings.developers.mcp.title")}
          description={<HelpLink onClick={() => setMcpHelp(true)}>{t("settings.developers.mcp.help")}</HelpLink>}
          control={(ids) => (
            <Switch
              aria-labelledby={ids.labelId}
              disabled={!preferences || saving === "mcp_server"}
              checked={preferences?.mcp_server === true}
              onChange={(checked) => void save("mcp_server", checked)}
            />
          )}
        />
        <SettingsRow title={t("settings.developers.agentLink.title")} description={t("settings.developers.agentLink.description")} onClick={() => setAgentLink(true)} />
      </SettingsCard>
      {agents.forbidden && <ScopeNotice what={t("agents.coding.what")} onGranted={() => void agents.reload()} />}
      {!agents.forbidden && agents.error && <ErrorNote>{agents.error}</ErrorNote>}
      {mcpRefused && <LocalOnlyNotice what={t("settings.developers.mcp.localOnly")} onChanged={() => void save("mcp_server", preferences?.mcp_server !== true)} />}
      <Dialog
        open={mcpHelp}
        onClose={() => setMcpHelp(false)}
        title={t("settings.developers.mcp.helpTitle")}
        description={tNodes("settings.developers.mcp.helpBody", { command: <code>openside mcp</code> })}
      >
        <pre className="dev-code">
          <code>{MCP_CONFIG}</code>
        </pre>
        <p className="settings-text">{tNodes("settings.developers.mcp.helpRemote", { args: <code>"--host", "&lt;name&gt;"</code> })}</p>
        <DialogActions>
          <Button variant="primary" onClick={() => void copyConfig()}>
            <Copy {...ICON} />
            {t("settings.developers.mcp.copy")}
          </Button>
        </DialogActions>
      </Dialog>
      <Dialog open={agentLink} onClose={() => setAgentLink(false)} title={t("settings.developers.agentLink.title")}>
        <AgentLink />
      </Dialog>
    </SettingsSection>
  );
}

/** Five questions, each a heading button over its answer region; any number may be open. */
function Faqs() {
  const base = useId();
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const toggle = (id: string) =>
    setOpen((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  return (
    <SettingsSection label={t("settings.developers.faq.label")}>
      <SettingsCard>
        {FAQS.map((faq) => {
          const expanded = open.has(faq.id);
          const question = `${base}-${faq.id}-question`;
          const answer = `${base}-${faq.id}-answer`;
          return (
            <div className="dev-faq" key={faq.id}>
              <h3 className="dev-faq-heading">
                <button type="button" id={question} className="dev-faq-question" aria-expanded={expanded} aria-controls={answer} onClick={() => toggle(faq.id)}>
                  <span>{t(faq.question)}</span>
                  <ChevronDown className="dev-faq-chevron" strokeWidth={ICON_STROKE} aria-hidden="true" />
                </button>
              </h3>
              <div id={answer} role="region" aria-labelledby={question} className="dev-faq-answer" hidden={!expanded}>
                <p>{t(faq.answer)}</p>
              </div>
            </div>
          );
        })}
      </SettingsCard>
    </SettingsSection>
  );
}

/**
 * Settings › Developers (vm2/developers-0/1): the CLI hero, CLI settings, the browser skill and
 * MCP server, Remote Control and FAQs. Remote Control is not gated: every transport works here.
 */
export function DevelopersSettings() {
  const client = useClient();
  const load = useCallback(() => client.externalAgents(), [client]);
  const agents = useResource(load);
  const switches = usePreferenceSwitches();
  return (
    <div className="personal-page developers-page" data-section="developers">
      <CliHero agents={agents} />
      <SettingsGroup switches={switches} />
      <SkillAndMcp agents={agents} switches={switches} />
      <RemoteControlSection />
      <Faqs />
    </div>
  );
}
