import { Tooltip } from "@base-ui/react/tooltip";
import { ArrowUpRight, Asterisk, BookOpen, ChevronDown, Folder, FolderOpen, FolderX, Hand, HatGlasses, LoaderCircle, Mic, Paperclip, Plus, Shield, ShieldCheck, SkipForward, type LucideIcon } from "lucide-react";
import { useEffect, useState, type ReactElement } from "react";
import { useNavigate } from "react-router";
import type { ChatConfirmation, ContextUsage, PolicyMode, Project } from "../api/types";
import { formatNumber, t } from "../i18n";
import { cx, ICON_STROKE, MenuGroupLabel, MenuItem, MenuSeparator, PopoverMenu, SubmenuItem } from "../ui";
import { settingsPath } from "../settings/nav";
import { keyLabel } from "../shortcuts";
import {
  CONFIRMATION_OPTIONS,
  EFFORT_OPTIONS,
  effortLabel,
  MODE_OPTIONS,
  modeBlock,
  modeLabel,
  type SessionOptions,
  type ThinkingEffort,
} from "./sessionOptions";
import type { Dictation } from "./useDictation";

export type ChangeOptions = (patch: Partial<SessionOptions>) => void;

/** Menus open above the card in a thread and below the Home pill, as in Aside (`L 14`, `L 37`). */
export type MenuSide = "top" | "bottom";

/**
 * Aside's dark tooltip over a toolbar control. Extra props come from a menu trigger wrapping this
 * element and are passed on, so one button can be both a tooltip and a menu trigger.
 */
export function Hint({ text, children, ...rest }: { text: string; children: ReactElement } & Record<string, unknown>) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger render={children} delay={400} {...rest} />
      <Tooltip.Portal>
        <Tooltip.Positioner side="top" sideOffset={6} collisionPadding={8}>
          <Tooltip.Popup className="composer-tooltip">{text}</Tooltip.Popup>
        </Tooltip.Positioner>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}

const caret = <ChevronDown className="composer-chip-caret" strokeWidth={ICON_STROKE} aria-hidden="true" />;

// The daemon's cost gate: an agent client gets 403 local_socket_required for these.
const COSTLY_EFFORT: readonly ThinkingEffort[] = ["xhigh", "max"];

const MODE_ICON: Record<PolicyMode, LucideIcon> = { "read-only": BookOpen, guard: ShieldCheck, full: Asterisk };

/** What the Projects rows and Attach folder need from the composer. */
export interface ProjectControls {
  /** Every project, by name; empty until read. */
  projects: readonly Project[];
  /**
   * This browser has the computer's local authority, so it may open the folder picker and create
   * projects; null until the daemon says. Remote devices can only pick an existing project.
   */
  local: boolean | null;
  /** Moves the chat into this project, or out of every project and folder with null. */
  onPick: (projectId: string | null) => void;
  /** Makes a project from a folder chosen in the picker, or from scratch, and picks it. */
  onCreate: (from: "folder" | "scratch") => void;
  /** Gives the chat a folder chosen in the picker, outside any project. */
  onAttachFolder: () => void;
}

/** Last segment of an absolute path, for a folder named in a chip. */
export function folderName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
}

/** The chip's words: the chosen project, else the attached folder, else "Project". */
function projectLabel(options: SessionOptions, projects: readonly Project[]): string | null {
  if (options.projectId) return projects.find((project) => project.id === options.projectId)?.name ?? null;
  return options.workspace ? folderName(options.workspace) : null;
}

/**
 * Header, divider, the existing projects (the current one checked) and No project, then the two
 * ways to make one (`L 18`). Making one needs the folder picker or a new folder on the computer
 * running openside, so other devices see both rows disabled with the reason. A project whose
 * folder needs confirming is disabled too: the daemon refuses chats in it until then.
 */
function ProjectRows({ options, controls }: { options: SessionOptions; controls: ProjectControls }) {
  const { projects, local, onPick, onCreate } = controls;
  const remote = local === false ? t("thread.project.localOnly") : undefined;
  const chosen = options.projectId !== null || options.workspace !== null;
  return (
    <>
      <MenuGroupLabel>{t("thread.project.title")}</MenuGroupLabel>
      <MenuSeparator />
      {projects.map((project) => (
        <MenuItem
          key={project.id}
          icon={<Folder strokeWidth={ICON_STROKE} />}
          checked={options.projectId === project.id}
          disabled={project.needs_confirmation}
          description={project.needs_confirmation ? t("thread.project.needsConfirming") : undefined}
          onClick={() => onPick(project.id)}
        >
          {project.name}
        </MenuItem>
      ))}
      {projects.length || chosen ? (
        <>
          <MenuItem icon={<FolderX strokeWidth={ICON_STROKE} />} checked={!chosen} onClick={() => onPick(null)}>
            {t("thread.project.none")}
          </MenuItem>
          <MenuSeparator />
        </>
      ) : null}
      <MenuItem icon={<FolderOpen strokeWidth={ICON_STROKE} />} disabled={local !== true} description={remote} onClick={() => onCreate("folder")}>
        {t("thread.project.existingFolder")}
      </MenuItem>
      <MenuItem icon={<Plus strokeWidth={ICON_STROKE} />} disabled={local !== true} description={remote} onClick={() => onCreate("scratch")}>
        {t("thread.project.scratch")}
      </MenuItem>
    </>
  );
}

export interface PlusMenuProps {
  options: SessionOptions;
  /** The chat exists: incognito is fixed, the rest is changed in place. */
  existing: boolean;
  /** This device's ceiling (the daemon default); looser modes are refused to paired devices. */
  deviceLimit: PolicyMode | null;
  onChange: ChangeOptions;
  onUpload: () => void;
  project: ProjectControls;
  side: MenuSide;
}

/** `+` (`L 14`): Permission ▸, Projects ▸, Incognito mode, then Upload photos & files and Attach folder. */
export function PlusMenu({ options, existing, deviceLimit, onChange, onUpload, project, side }: PlusMenuProps) {
  const navigate = useNavigate();
  const mode = options.mode ?? deviceLimit ?? "guard";
  const projectName = projectLabel(options, project.projects);
  return (
    <PopoverMenu
      label={t("thread.options.plusMenu")}
      side={side}
      align="start"
      sideOffset={5}
      popupClassName="composer-plus-menu"
      trigger={
        <button type="button" className="composer-plus" aria-label={t("thread.options.plusTrigger")}>
          <Plus strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      }
    >
      <SubmenuItem icon={<Shield strokeWidth={ICON_STROKE} />} label={t("thread.options.permission")} value={<span className="composer-menu-value">{modeLabel(mode)}</span>} popupClassName="composer-permission-menu">
        {MODE_OPTIONS.map((option) => {
          const blocked = modeBlock(option.value, deviceLimit);
          const Icon = MODE_ICON[option.value];
          return (
            <MenuItem key={option.value} icon={<Icon strokeWidth={ICON_STROKE} />} checked={mode === option.value} disabled={blocked !== null} onClick={() => onChange({ mode: option.value })}>
              {t(option.labelKey)}
            </MenuItem>
          );
        })}
        <MenuSeparator />
        <MenuItem trailing={<ArrowUpRight strokeWidth={ICON_STROKE} aria-hidden="true" />} onClick={() => navigate(settingsPath("agents"))}>
          {t("thread.options.globalSetting")}
        </MenuItem>
      </SubmenuItem>
      <SubmenuItem
        icon={<Folder strokeWidth={ICON_STROKE} />}
        label={t("thread.project.title")}
        value={
          projectName ? (
            <span className="composer-menu-value" title={projectName}>
              {projectName}
            </span>
          ) : undefined
        }
        popupClassName="composer-project-menu"
      >
        <ProjectRows options={options} controls={project} />
      </SubmenuItem>
      <MenuItem icon={<HatGlasses strokeWidth={ICON_STROKE} />} checked={options.incognito} disabled={existing} onClick={() => onChange({ incognito: !options.incognito })}>
        {t("thread.options.incognito")}
      </MenuItem>
      <MenuSeparator />
      <MenuItem icon={<Paperclip strokeWidth={ICON_STROKE} />} onClick={onUpload}>
        {t("thread.options.upload")}
      </MenuItem>
      {/* The attached folder, if any, is named in the row's tooltip. */}
      <MenuItem
        icon={<FolderOpen strokeWidth={ICON_STROKE} />}
        disabled={project.local !== true}
        description={project.local === false ? t("thread.project.localOnly") : (options.workspace ?? undefined)}
        onClick={project.onAttachFolder}
      >
        {t("thread.options.attachFolder")}
      </MenuItem>
    </PopoverMenu>
  );
}

/** Home's Project chip (`L 18`): the same rows as the `+` menu's Projects ▸, named after the pick. */
export function ProjectMenu({ options, project, side }: { options: SessionOptions; project: ProjectControls; side: MenuSide }) {
  return (
    <PopoverMenu
      label={t("thread.project.title")}
      side={side}
      align="start"
      sideOffset={5}
      popupClassName="composer-project-menu"
      trigger={
        <Hint text={t("thread.project.hint")}>
          <button type="button" className="composer-chip">
            <Folder className="composer-chip-icon" strokeWidth={ICON_STROKE} aria-hidden="true" />
            <span className="composer-chip-label">{projectLabel(options, project.projects) ?? t("thread.project.chip")}</span>
            {caret}
          </button>
        </Hint>
      }
    >
      <ProjectRows options={options} controls={project} />
    </PopoverMenu>
  );
}

/** Whether file writes and commands ask first (`L 16`); the default comes from Preferences. */
export function ConfirmationMenu({ value, onChange, side }: { value: ChatConfirmation; onChange: ChangeOptions; side: MenuSide }) {
  const label = t(CONFIRMATION_OPTIONS.find((option) => option.value === value)?.labelKey ?? "thread.confirmation.askFirst");
  const Icon = value === "skip" ? SkipForward : Hand;
  return (
    <PopoverMenu
      label={t("thread.confirmation.title")}
      side={side}
      align="start"
      sideOffset={5}
      popupClassName="composer-confirmation-menu"
      trigger={
        <button type="button" className="composer-chip" aria-label={t("thread.options.trigger", { label: t("thread.confirmation.title"), value: label })}>
          <Icon className="composer-chip-icon" strokeWidth={ICON_STROKE} aria-hidden="true" />
          <span className="composer-chip-label">{label}</span>
          {caret}
        </button>
      }
    >
      {CONFIRMATION_OPTIONS.map((option) => (
        <MenuItem key={option.value} checked={value === option.value} onClick={() => onChange({ confirmation: option.value })}>
          {t(option.labelKey)}
        </MenuItem>
      ))}
    </PopoverMenu>
  );
}

export interface EffortMenuProps {
  options: SessionOptions;
  /** The level the chat's model runs at when no effort is picked; null when the provider decides. */
  defaultEffort: ThinkingEffort | null;
  existing: boolean;
  agentClient: boolean;
  onChange: ChangeOptions;
  side: MenuSide;
}

/**
 * Effort (`L 17`): Low to Max, then Ultrabrowse, the browsing profile shown as the top rung. The
 * profile is fixed when a chat is created, so an existing chat only shows whether it has it.
 * Mod+Shift+. opens the menu.
 */
export function EffortMenu({ options, defaultEffort, existing, agentClient, onChange, side }: EffortMenuProps) {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.code === "Period") {
        event.preventDefault();
        setOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const isUltrabrowse = options.profile === "ultrabrowse";
  // An unset effort stays unset (the request names none, so a model that cannot serve a pinned
  // level never fails the chat); the chip shows the level the model then runs at. A level outside
  // the composer's ladder, or one the provider decides, reads as the default.
  const unpicked = defaultEffort !== null && EFFORT_OPTIONS.some((option) => option.value === defaultEffort) ? effortLabel(defaultEffort) : t("thread.options.default");
  const value = isUltrabrowse ? t("thread.effort.ultrabrowse") : options.effort ? effortLabel(options.effort) : unpicked;
  return (
    <PopoverMenu
      label={t("thread.options.effort")}
      side={side}
      align="end"
      sideOffset={5}
      open={open}
      onOpenChange={setOpen}
      popupClassName="composer-effort-menu"
      trigger={
        <Hint text={t("thread.options.effortHint", { keys: keyLabel(["Mod", "Shift", "."]) })}>
          <button type="button" className="composer-chip composer-effort-chip" aria-label={t("thread.options.trigger", { label: t("thread.options.effort"), value })}>
            <span className={cx("composer-chip-label", isUltrabrowse && "composer-ultrabrowse")}>{value}</span>
            {caret}
          </button>
        </Hint>
      }
    >
      {EFFORT_OPTIONS.map((option) => (
        <MenuItem
          key={option.value}
          checked={!isUltrabrowse && options.effort === option.value}
          disabled={agentClient && COSTLY_EFFORT.includes(option.value)}
          onClick={() => onChange(existing ? { effort: option.value } : { effort: option.value, profile: null })}
        >
          {t(option.labelKey)}
        </MenuItem>
      ))}
      <MenuItem
        checked={isUltrabrowse}
        disabled={agentClient || (existing && !isUltrabrowse)}
        textValue={t("thread.effort.ultrabrowse")}
        onClick={() => !existing && onChange({ profile: "ultrabrowse", effort: null })}
      >
        <span className="composer-ultrabrowse">{t("thread.effort.ultrabrowse")}</span>
      </MenuItem>
    </PopoverMenu>
  );
}

const RING = 14;
const RING_STROKE = 1.5;
const RING_RADIUS = (RING - RING_STROKE) / 2;
const RING_LENGTH = 2 * Math.PI * RING_RADIUS;

/** How full the chat's model context is (`L 21`): a 14px ring, the numbers in its tooltip. */
export function ContextRing({ context }: { context: ContextUsage | null | undefined }) {
  const limit = context?.window_tokens ?? null;
  const used = context?.used_tokens ?? 0;
  const share = limit ? Math.min(1, used / limit) : 0;
  const text = limit
    ? t("thread.context.usage", { percent: Math.round(share * 100), used: formatNumber(used, { notation: "compact" }), window: formatNumber(limit, { notation: "compact" }) })
    : t("thread.context.usageUnknown");
  return (
    <Hint text={text}>
      <span className={cx("composer-ring", share >= 0.9 && "is-full")} role="img" aria-label={text} tabIndex={0}>
        <svg width={RING} height={RING} viewBox={`0 0 ${RING} ${RING}`} aria-hidden="true">
          <circle className="composer-ring-track" cx={RING / 2} cy={RING / 2} r={RING_RADIUS} strokeWidth={RING_STROKE} fill="none" />
          <circle
            className="composer-ring-fill"
            cx={RING / 2}
            cy={RING / 2}
            r={RING_RADIUS}
            strokeWidth={RING_STROKE}
            fill="none"
            strokeDasharray={RING_LENGTH}
            strokeDashoffset={RING_LENGTH * (1 - share)}
            strokeLinecap="round"
            transform={`rotate(-90 ${RING / 2} ${RING / 2})`}
          />
        </svg>
      </span>
    </Hint>
  );
}

/**
 * The microphone (`L 13`, `L 21`): a filled circle while the box is empty, a grey glyph beside
 * Send once something is typed or a run can be stopped. Unavailable dictation stays visible with
 * a tooltip that says why, so it is `aria-disabled` rather than `disabled` (which drops hover).
 */
export function MicButton({ dictation, variant }: { dictation: Dictation; variant: "circle" | "glyph" }) {
  const { state, reason, toggle } = dictation;
  const off = state === "unavailable" || state === "checking";
  const transcribing = state === "transcribing";
  // While a clip is being turned into text the button cancels it; its description says what is happening.
  const label = state === "recording" ? t("thread.mic.stop") : transcribing ? t("thread.mic.cancel") : t("thread.mic.start");
  return (
    <Hint text={reason ?? label}>
      <button
        type="button"
        className={cx(variant === "circle" ? "composer-send composer-mic-circle" : "composer-mic", state === "recording" && "is-recording")}
        aria-label={label}
        // The tooltip only shows on hover; a screen reader hears why the microphone is off here.
        aria-description={reason ?? (transcribing ? t("thread.mic.transcribing") : undefined)}
        aria-pressed={state === "recording"}
        aria-disabled={off || undefined}
        onClick={() => !off && toggle()}
      >
        {transcribing ? (
          <LoaderCircle className="composer-spin" strokeWidth={2} aria-hidden="true" />
        ) : (
          <Mic strokeWidth={variant === "circle" ? 2 : ICON_STROKE} aria-hidden="true" />
        )}
      </button>
    </Hint>
  );
}
