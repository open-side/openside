import { Menu } from "@base-ui/react/menu";
import { Check, ChevronDown, Lock, Paperclip, Plus, Settings2 } from "lucide-react";
import { useState, type KeyboardEvent, type ReactElement, type ReactNode } from "react";
import { Link, useLocation } from "react-router";
import type { ModelCatalog, PolicyMode } from "../api/types";
import { providerLabel } from "../models/providers";
import { manageLinkState } from "../pages/manageRoutes";
import { ICON_STROKE } from "../ui";
import {
  EFFORT_OPTIONS,
  effortLabel,
  MODE_OPTIONS,
  modeBlock,
  modeLabel,
  type SessionOptions,
  type SessionProfile,
  type SessionSpeed,
  type ThinkingEffort,
} from "./sessionOptions";

// Base UI radio groups treat `null` as "nothing selected"; the request's "leave it out" needs a value.
const DEFAULT = "default";
const fromRadio = <T extends string>(value: string): T | null => (value === DEFAULT ? null : (value as T));

export type ChangeOptions = (patch: Partial<SessionOptions>) => void;

/** One toolbar popup: a compact text trigger above the composer, a bottom sheet on phones (thread.css). */
function OptionMenu({
  trigger,
  label,
  align = "start",
  onOpenChange,
  children,
}: {
  trigger: ReactElement;
  label: string;
  align?: "start" | "end";
  onOpenChange?: (open: boolean) => void;
  children: ReactNode;
}) {
  return (
    <Menu.Root onOpenChange={onOpenChange}>
      <Menu.Trigger render={trigger} />
      <Menu.Portal>
        <Menu.Backdrop className="composer-menu-backdrop" />
        <Menu.Positioner className="ui-menu-positioner composer-menu-positioner" side="top" align={align} sideOffset={8} collisionPadding={8}>
          <Menu.Popup className="ui-menu composer-menu surface-overlay thin-scroll" aria-label={label}>
            {children}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}

function OptionTrigger({ label, value, icon, ...rest }: { label: string; value: string; icon?: ReactNode; title?: string }) {
  return (
    <button type="button" className="composer-option" aria-label={`${label}: ${value}`} {...rest}>
      {icon}
      <span className="composer-option-label">{value}</span>
      <ChevronDown size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </button>
  );
}

function Choice({ value, label, detail, disabled }: { value: string; label: string; detail?: string; disabled?: boolean }) {
  return (
    <Menu.RadioItem value={value} label={label} disabled={disabled} closeOnClick className="ui-menu-item composer-menu-item">
      <span className="composer-menu-copy">
        <span>{label}</span>
        {detail ? <small>{detail}</small> : null}
      </span>
      <Menu.RadioItemIndicator className="composer-menu-check">
        <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </Menu.RadioItemIndicator>
    </Menu.RadioItem>
  );
}

function PermissionChoices({ mode, ceiling, deviceLimit, agentName, onChange }: PermissionProps & { mode: PolicyMode | null }) {
  return (
    <Menu.RadioGroup className="composer-menu-group" value={mode ?? DEFAULT} onValueChange={(value: string) => onChange({ mode: fromRadio<PolicyMode>(value) })}>
      <Menu.GroupLabel className="ui-menu-title">Permission</Menu.GroupLabel>
      <Choice value={DEFAULT} label="Default" detail={modeLabel(ceiling)} />
      {MODE_OPTIONS.map((option) => {
        const blocked = modeBlock(option.value, ceiling, deviceLimit, agentName);
        return <Choice key={option.value} value={option.value} label={option.label} disabled={blocked !== null} detail={blocked ?? undefined} />;
      })}
    </Menu.RadioGroup>
  );
}

interface PermissionProps {
  onChange: ChangeOptions;
  ceiling: PolicyMode;
  /** The daemon default: paired devices cannot start a task looser than it. `null` until known. */
  deviceLimit: PolicyMode | null;
  agentName: string;
}

export interface PlusMenuProps {
  onUpload: () => void;
  /** New task only: the creation-time choices that have no trigger of their own. */
  task?: PermissionProps & { options: SessionOptions };
}

/** `+`: permission and private mode for a new task, and files for any message. */
export function PlusMenu({ onUpload, task }: PlusMenuProps) {
  return (
    <OptionMenu
      label="Add to this task"
      trigger={
        <button type="button" className="ui-icon-button composer-attach" aria-label="Add files and options">
          <Plus size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      }
    >
      {task ? (
        <>
          <PermissionChoices {...task} mode={task.options.mode} />
          <Menu.Separator className="ui-menu-separator" />
          <Menu.CheckboxItem
            className="ui-menu-item composer-menu-item"
            checked={task.options.incognito}
            onCheckedChange={(checked) => task.onChange({ incognito: checked })}
          >
            <span className="ui-menu-icon" aria-hidden="true">
              <Lock size={18} strokeWidth={ICON_STROKE} />
            </span>
            <span className="composer-menu-copy">
              <span>Private task</span>
              <small>Not saved to browsing history; opens from History</small>
            </span>
            <Menu.CheckboxItemIndicator className="composer-menu-check">
              <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </Menu.CheckboxItemIndicator>
          </Menu.CheckboxItem>
          <Menu.Separator className="ui-menu-separator" />
        </>
      ) : null}
      <Menu.Item className="ui-menu-item composer-menu-item" onClick={onUpload}>
        <span className="ui-menu-icon" aria-hidden="true">
          <Paperclip size={18} strokeWidth={ICON_STROKE} />
        </span>
        <span>Upload photos &amp; files</span>
      </Menu.Item>
    </OptionMenu>
  );
}

export function PermissionMenu({ options, ...permission }: PermissionProps & { options: SessionOptions }) {
  return (
    <OptionMenu label="Permission" trigger={<OptionTrigger label="Permission" value={modeLabel(options.mode ?? permission.ceiling)} />}>
      <PermissionChoices {...permission} mode={options.mode} />
    </OptionMenu>
  );
}

/** The catalog's label for a model id; ids the catalog does not know stay as typed. */
export function catalogLabel(catalog: ModelCatalog | null, id: string): string {
  for (const provider of catalog?.providers ?? []) {
    const found = provider.models.find((model) => model.id === id);
    if (found) return found.label;
  }
  return id;
}

/**
 * The model for this one task. Unlike `ModelSheet` it never changes the person: the pick only
 * goes into the new session's create request.
 */
export function ModelMenu({ catalog, model, fallback, onChange }: { catalog: ModelCatalog | null; model: string | null; fallback: string | null; onChange: ChangeOptions }) {
  const location = useLocation();
  const [query, setQuery] = useState("");
  const current = model ?? fallback;
  const needle = query.trim().toLowerCase();
  const matches = (...texts: string[]) => !needle || texts.some((text) => text.toLowerCase().includes(needle));
  const groups = (catalog?.providers ?? [])
    .map((provider) => ({
      provider: provider.provider,
      models: provider.models.filter((entry) => matches(entry.label, entry.id, providerLabel(provider.provider))),
    }))
    .filter((group) => group.models.length > 0);
  const showDefault = matches("Default", fallback ?? "");
  // Typing goes to the field: the menu's own type-to-select and Home/End would take the keys.
  const keepKeys = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== "Escape" && event.key !== "ArrowDown" && event.key !== "ArrowUp" && event.key !== "Tab") event.stopPropagation();
  };
  return (
    <OptionMenu
      label="Model"
      align="end"
      onOpenChange={(open) => !open && setQuery("")}
      trigger={<OptionTrigger label="Model" value={current ? catalogLabel(catalog, current) : "Model"} title={current ?? undefined} />}
    >
      <input
        type="search"
        className="composer-menu-search"
        aria-label="Search models"
        placeholder="Search models"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={keepKeys}
      />
      <div className="composer-menu-list thin-scroll">
        <Menu.RadioGroup className="composer-menu-group" value={model ?? DEFAULT} onValueChange={(value: string) => onChange({ model: fromRadio<string>(value) })}>
          {showDefault ? <Choice value={DEFAULT} label="Default" detail={fallback ? catalogLabel(catalog, fallback) : undefined} /> : null}
          {groups.map((group) => (
            <Menu.Group key={group.provider} className="composer-menu-group">
              <Menu.GroupLabel className="ui-menu-title">{providerLabel(group.provider)}</Menu.GroupLabel>
              {group.models.map((entry) => (
                <Choice
                  key={entry.id}
                  value={entry.id}
                  label={entry.label}
                  disabled={!entry.available}
                  detail={entry.available ? undefined : entry.reason || "Not available right now"}
                />
              ))}
            </Menu.Group>
          ))}
        </Menu.RadioGroup>
        {!showDefault && groups.length === 0 ? <p className="composer-menu-empty">No model matches.</p> : null}
      </div>
      <Menu.Separator className="ui-menu-separator" />
      <Menu.LinkItem className="ui-menu-item composer-menu-item" render={<Link to="/models" state={manageLinkState(location)} />}>
        <span className="ui-menu-icon" aria-hidden="true">
          <Settings2 size={18} strokeWidth={ICON_STROKE} />
        </span>
        <span>Manage models &amp; accounts</span>
      </Menu.LinkItem>
    </OptionMenu>
  );
}

// The daemon's cost gate: an agent client gets 403 local_socket_required for these.
const COSTLY_EFFORT: readonly ThinkingEffort[] = ["xhigh", "max"];
const AGENT_CLIENT_REASON = "Not available to an agent client";

/** Thinking effort, plus the Ultrabrowse profile: both say how hard this task works, but the profile is its own field. */
export function EffortMenu({ options, onChange, agentClient }: { options: SessionOptions; onChange: ChangeOptions; agentClient: boolean }) {
  const effort = options.effort ? effortLabel(options.effort) : "Effort";
  const value = options.profile === "ultrabrowse" ? `${effort} · Ultrabrowse` : effort;
  return (
    <OptionMenu label="Effort" align="end" trigger={<OptionTrigger label="Effort" value={value} />}>
      <Menu.RadioGroup className="composer-menu-group" value={options.effort ?? DEFAULT} onValueChange={(next: string) => onChange({ effort: fromRadio<ThinkingEffort>(next) })}>
        <Menu.GroupLabel className="ui-menu-title">Effort</Menu.GroupLabel>
        <Choice value={DEFAULT} label="Default" detail="The model’s own setting" />
        {EFFORT_OPTIONS.map((option) => {
          const blocked = agentClient && COSTLY_EFFORT.includes(option.value);
          return <Choice key={option.value} value={option.value} label={option.label} disabled={blocked} detail={blocked ? AGENT_CLIENT_REASON : undefined} />;
        })}
      </Menu.RadioGroup>
      <Menu.Separator className="ui-menu-separator" />
      <Menu.RadioGroup className="composer-menu-group" value={options.profile ?? DEFAULT} onValueChange={(next: string) => onChange({ profile: fromRadio<SessionProfile>(next) })}>
        <Menu.GroupLabel className="ui-menu-title">Profile</Menu.GroupLabel>
        <Choice value={DEFAULT} label="Standard" />
        <Choice
          value="ultrabrowse"
          label="Ultrabrowse"
          disabled={agentClient}
          detail={agentClient ? AGENT_CLIENT_REASON : "Tools and instructions tuned for long browsing tasks"}
        />
      </Menu.RadioGroup>
    </OptionMenu>
  );
}

export function SpeedMenu({ options, onChange, agentClient }: { options: SessionOptions; onChange: ChangeOptions; agentClient: boolean }) {
  return (
    <OptionMenu label="Speed" align="end" trigger={<OptionTrigger label="Speed" value={options.speed === "fast" ? "Fast" : "Speed"} />}>
      <Menu.RadioGroup className="composer-menu-group" value={options.speed ?? DEFAULT} onValueChange={(next: string) => onChange({ speed: fromRadio<SessionSpeed>(next) })}>
        <Menu.GroupLabel className="ui-menu-title">Speed</Menu.GroupLabel>
        <Choice value={DEFAULT} label="Default" />
        <Choice
          value="fast"
          label="Fast"
          disabled={agentClient}
          detail={agentClient ? AGENT_CLIENT_REASON : "Priority service where the provider offers it"}
        />
      </Menu.RadioGroup>
    </OptionMenu>
  );
}
