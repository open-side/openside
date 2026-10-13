import {
  BookOpen,
  Brain,
  CodeXml,
  FilePenLine,
  FileText,
  Globe,
  KeyRound,
  MessageCircleQuestion,
  Search,
  Terminal,
  Users,
  type LucideIcon,
} from "lucide-react";
import type { BrowserStateView, TimelineItem } from "../state/session";
import { hostOf } from "./toolStatus";

export type ToolItem = Extract<TimelineItem, { type: "tool" }>;

export type ToolKindId = "browse" | "search" | "read" | "edit" | "shell" | "helper" | "skill" | "memory" | "vault" | "ask" | "other";

/** What the conversation knows about a call beyond its own arguments: the page it ran on and the element it touched. */
export interface ToolFacts {
  /** Site the browser was on for this call. */
  host: string | null;
  /** Title of the page a navigation opened, from the next page read or the daemon's browser state. */
  title: string | null;
  /** Accessible name of the element a click, fill or choice acted on. */
  label: string | null;
  /** `data:image/` icon of the site; anything else is never loaded. */
  favicon: string | null;
}

export type ToolFactsMap = ReadonlyMap<string, ToolFacts>;

export interface ToolDescription {
  /** A sentence in the agent's voice: past tense once the call finished, present while it runs. */
  title: string;
  /** One quiet extra fact (the site of a page title, the command that ran). */
  detail?: string;
  kind: ToolKindId;
  Icon: LucideIcon;
}

const KIND_ICON: Record<ToolKindId, LucideIcon> = {
  browse: Globe,
  search: Search,
  read: FileText,
  edit: FilePenLine,
  shell: Terminal,
  helper: Users,
  skill: BookOpen,
  memory: Brain,
  vault: KeyRound,
  ask: MessageCircleQuestion,
  other: CodeXml,
};

export function iconOf(kind: ToolKindId): LucideIcon {
  return KIND_ICON[kind];
}

const TITLE_LIMIT = 60;

function clip(text: string, limit = TITLE_LIMIT): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > limit ? `${flat.slice(0, limit - 1).trimEnd()}…` : flat;
}

function quoted(label: string | null | undefined): string | null {
  return label ? `“${clip(label, 48)}”` : null;
}

/** "Email address" → "email address"; a name with its own capitals ("Gmail ID") stays as written. */
function field(label: string): string {
  const text = clip(label, 40);
  return /^[A-Z][a-z]*( [a-z]+)*$/.test(text) ? text.charAt(0).toLowerCase() + text.slice(1) : text;
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

function str(args: Record<string, unknown>, key: string): string | null {
  const value = args[key];
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function firstLine(text: string | null): string | undefined {
  const line = text?.split("\n").find((entry) => entry.trim());
  return line ? clip(line, 90) : undefined;
}

function baseName(path: string | null): string | null {
  return path?.split(/[\\/]/).filter(Boolean).at(-1) ?? null;
}

/** `my_server__fetch_page` → "fetch page": a tool nobody wrote copy for still reads as words. */
export function humanize(name: string): string {
  const last = name.split("__").filter(Boolean).at(-1) ?? name;
  return last.replace(/[^\p{L}\p{N}]+/gu, " ").trim().toLowerCase() || "a tool";
}

interface Call {
  args: Record<string, unknown>;
  facts: ToolFacts | undefined;
}

interface Phrase {
  kind: ToolKindId;
  past: (call: Call) => string;
  present: (call: Call) => string;
  detail?: (call: Call) => string | undefined;
}

const fixed = (kind: ToolKindId, past: string, present: string, detail?: Phrase["detail"]): Phrase => ({
  kind,
  past: () => past,
  present: () => present,
  ...(detail ? { detail } : {}),
});

function siteOf(call: Call): string | null {
  return call.facts?.host ?? hostOf(call.args.url) ?? hostOf(`https://${str(call.args, "site") ?? ""}`) ?? null;
}

const open: Phrase = {
  kind: "browse",
  past: ({ args, facts }) => {
    const page = facts?.title ? clip(facts.title) : (facts?.host ?? hostOf(args.url));
    return page ? `Opened ${page}` : "Opened a page";
  },
  present: ({ args, facts }) => {
    const host = facts?.host ?? hostOf(args.url);
    return host ? `Opening ${host}` : "Opening a page";
  },
  detail: ({ args, facts }) => (facts?.title ? (facts.host ?? hostOf(args.url) ?? undefined) : undefined),
};

const act: Phrase = {
  kind: "browse",
  past: ({ args, facts }) => {
    const label = quoted(facts?.label);
    switch (args.action) {
      case "click":
        return label ? `Clicked ${label}` : "Clicked on the page";
      case "type":
        return `${facts?.label ? `Filled the ${field(facts.label)} field` : "Filled in a field"}${args.submit === true ? " and submitted" : ""}`;
      case "press":
        return `Pressed ${str(args, "key") ?? "a key"}`;
      case "select":
        return label ? `Chose an option in ${label}` : "Chose an option";
      case "hover":
        return label ? `Hovered over ${label}` : "Hovered over the page";
      case "focus":
        return label ? `Selected ${label}` : "Selected a field";
      default:
        return "Interacted with the page";
    }
  },
  present: ({ args, facts }) => {
    const label = quoted(facts?.label);
    switch (args.action) {
      case "click":
        return label ? `Clicking ${label}` : "Clicking on the page";
      case "type":
        return facts?.label ? `Filling the ${field(facts.label)} field` : "Filling in a field";
      case "press":
        return `Pressing ${str(args, "key") ?? "a key"}`;
      case "select":
        return label ? `Choosing an option in ${label}` : "Choosing an option";
      case "hover":
        return label ? `Hovering over ${label}` : "Hovering over the page";
      case "focus":
        return label ? `Selecting ${label}` : "Selecting a field";
      default:
        return "Interacting with the page";
    }
  },
};

const pointer: Phrase = {
  kind: "browse",
  past: ({ args }) => {
    switch (args.action) {
      case "double_click":
        return "Double-clicked on the page";
      case "type":
        return "Typed on the page";
      case "scroll":
        return "Scrolled the page";
      default:
        return "Clicked on the page";
    }
  },
  present: ({ args }) => {
    switch (args.action) {
      case "double_click":
        return "Double-clicking on the page";
      case "type":
        return "Typing on the page";
      case "scroll":
        return "Scrolling the page";
      default:
        return "Clicking on the page";
    }
  },
};

const file = (call: Call) => baseName(str(call.args, "path"));

/** Every tool the daemon registers for the agent, in the voice of the person watching it work. */
const PHRASES: Record<string, Phrase> = {
  browser_open_tab: open,
  browser_navigate: open,
  browser_tabs: fixed("browse", "Checked the open tabs", "Checking the open tabs"),
  browser_snapshot: fixed("browse", "Read the page", "Reading the page"),
  browser_screenshot: fixed("browse", "Read the page", "Reading the page"),
  browser_close_tab: fixed("browse", "Closed a tab", "Closing a tab"),
  browser_act: act,
  browser_pointer: pointer,
  browser_repl: fixed("browse", "Worked on the page", "Working on the page"),
  browser_space_reset: fixed("browse", "Reset the browser", "Resetting the browser"),
  read: { kind: "read", past: (call) => `Read ${file(call) ?? "a file"}`, present: (call) => `Reading ${file(call) ?? "a file"}` },
  write: { kind: "edit", past: (call) => `Saved ${file(call) ?? "a file"}`, present: (call) => `Saving ${file(call) ?? "a file"}` },
  edit: { kind: "edit", past: (call) => `Edited ${file(call) ?? "a file"}`, present: (call) => `Editing ${file(call) ?? "a file"}` },
  bash: fixed("shell", "Ran a command", "Running a command", ({ args }) => firstLine(str(args, "command"))),
  request_human_step: fixed("ask", "Asked you to take a step", "Waiting for you"),
  ask_user: fixed("ask", "Asked you a question", "Asking you a question", ({ args }) => firstLine(str(args, "question"))),
  request_login: fixed("ask", "Asked you to sign in", "Waiting for you to sign in"),
  request_choice: fixed("ask", "Asked you to choose", "Waiting for your choice"),
  wait_for_approval: fixed("ask", "Waited for your approval", "Waiting for your approval"),
  request_otp: fixed("ask", "Asked you for a code", "Waiting for your code"),
  register_authenticator: fixed("vault", "Set up an authenticator", "Setting up an authenticator"),
  memory_search: fixed("memory", "Checked memory", "Checking memory", ({ args }) => firstLine(str(args, "query"))),
  memory_write: fixed("memory", "Remembered something", "Remembering something"),
  skill: {
    kind: "skill",
    past: ({ args }) => `Loaded the ${str(args, "name") ?? "a"} skill`.replace("the a skill", "a skill"),
    present: ({ args }) => `Loading the ${str(args, "name") ?? "a"} skill`.replace("the a skill", "a skill"),
  },
  vault_login: {
    kind: "vault",
    past: (call) => `Filled your saved login on ${siteOf(call) ?? "the page"}`,
    present: (call) => `Filling your saved login on ${siteOf(call) ?? "the page"}`,
  },
  vault_otp: {
    kind: "vault",
    past: (call) => `Entered a saved code on ${siteOf(call) ?? "the page"}`,
    present: (call) => `Entering a saved code on ${siteOf(call) ?? "the page"}`,
  },
  vault_passkey: {
    kind: "vault",
    past: (call) => (call.args.action === "register" ? `Created a passkey for ${siteOf(call) ?? "this site"}` : `Signed in to ${siteOf(call) ?? "the site"} with a passkey`),
    present: (call) => (call.args.action === "register" ? `Creating a passkey for ${siteOf(call) ?? "this site"}` : `Signing in to ${siteOf(call) ?? "the site"} with a passkey`),
  },
  delegate: fixed("helper", "Asked a helper", "Asking a helper", ({ args }) => firstLine(str(args, "task"))),
  create_task: fixed("helper", "Started a task", "Starting a task", ({ args }) => firstLine(str(args, "goal"))),
  steer_task: fixed("helper", "Updated a task", "Updating a task"),
  cancel_task: fixed("helper", "Cancelled a task", "Cancelling a task"),
  get_tasks: fixed("helper", "Checked on tasks", "Checking on tasks"),
  send_message: fixed("other", "Sent a message", "Writing a message"),
};

/** Tool names the daemon registers; the table above must speak for each of them. */
export function hasPhrase(name: string): boolean {
  return name in PHRASES;
}

/** Tools no one wrote copy for (plugin or future tools) still get a sentence and a sensible icon. */
function generic(name: string): Phrase {
  const words = humanize(name);
  const lower = name.toLowerCase();
  const kind: ToolKindId = /search|grep|glob|find/.test(lower)
    ? "search"
    : /fetch|web|url|browse/.test(lower)
      ? "browse"
      : /shell|exec|command|terminal/.test(lower)
        ? "shell"
        : "other";
  if (kind === "search") return fixed("search", "Searched", "Searching");
  return fixed(kind, `Used ${words}`, `Using ${words}`);
}

function phraseOf(name: string): Phrase {
  return PHRASES[name.toLowerCase()] ?? generic(name);
}

export function toolKind(name: string): ToolKindId {
  return phraseOf(name).kind;
}

export function describeTool(tool: ToolItem, facts?: ToolFacts): ToolDescription {
  const phrase = phraseOf(tool.name);
  const call: Call = { args: record(tool.args), facts };
  const detail = phrase.detail?.(call);
  return {
    title: tool.done ? phrase.past(call) : phrase.present(call),
    ...(detail ? { detail } : {}),
    kind: phrase.kind,
    Icon: KIND_ICON[phrase.kind],
  };
}

const isNavigation = (name: string) => name === "browser_navigate" || name === "browser_open_tab";

export function opensPage(tool: ToolItem): boolean {
  return isNavigation(tool.name.toLowerCase());
}

/** A browser call that works on the page a navigation opened (as opposed to opening one). */
export function isPageStep(tool: ToolItem): boolean {
  const name = tool.name.toLowerCase();
  return name.startsWith("browser_") && !isNavigation(name) && name !== "browser_tabs" && name !== "browser_close_tab";
}

// --- facts read from the conversation --------------------------------------------------------

/** `[e12] button "Sign in"` lines of a snapshot; diff lines may carry a leading +/-/~. */
const SNAPSHOT_NODE = /^[+\-~\s]*\[(e\d+)\]\s+(\S+)(?:\s+"((?:[^"\\]|\\.)*)")?/gm;

function snapshotText(result: unknown): string {
  if (typeof result === "string") return result;
  const text = record(result).snapshot;
  return typeof text === "string" ? text : "";
}

function unescapeName(name: string): string {
  return name.replace(/\\(["\\])/g, "$1");
}

const SAFE_ICON = /^data:image\/(?:png|x-icon|vnd\.microsoft\.icon|jpeg|gif|webp);base64,[A-Za-z0-9+/=]+$/;

/** The daemon fetches the icon; the page must never be able to make the UI load a remote image. */
export function safeFavicon(value: unknown): string | null {
  return typeof value === "string" && SAFE_ICON.test(value) ? value : null;
}

function cleanTitle(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? clip(value, 80) : null;
}

/**
 * Walks the calls once, in order, to learn what each acted on: a click's target comes from the
 * latest page read of its tab (refs name elements there), a navigation's title from the next
 * page read, and favicons from the browser states the daemon reported per site.
 */
export function collectToolFacts(items: readonly TimelineItem[], states: Readonly<Record<string, BrowserStateView>> = {}): ToolFactsMap {
  const facts = new Map<string, ToolFacts>();
  const tabs = new Map<string, { host: string | null; nav: ToolFacts | null; labels: Map<string, { role: string; name: string }> }>();
  let lastHost: string | null = null;

  for (const item of items) {
    if (item.type !== "tool") continue;
    const name = item.name.toLowerCase();
    if (!name.startsWith("browser_") && !name.startsWith("vault_")) continue;
    const args = record(item.args);
    const result = record(item.result);
    const tab = str(args, "tab_id") ?? str(result, "tab_id") ?? "";
    let page = tabs.get(tab);
    const entry: ToolFacts = { host: page?.host ?? lastHost, title: null, label: null, favicon: null };

    if (isNavigation(name)) {
      const host = hostOf(args.url);
      entry.host = host ?? entry.host;
      page = { host: entry.host, nav: entry, labels: new Map() };
      tabs.set(tab, page);
      lastHost = entry.host;
    } else if (name === "browser_snapshot" && page) {
      const text = snapshotText(item.result);
      for (const match of text.matchAll(SNAPSHOT_NODE)) {
        const [, ref, role, label] = match;
        if (!ref || !role) continue;
        const named = label ? unescapeName(label) : "";
        if (role === "RootWebArea") {
          if (page.nav && !page.nav.title) page.nav.title = cleanTitle(named);
        } else if (named) {
          page.labels.set(ref, { role, name: named });
        }
      }
    } else if (name === "browser_act" && page) {
      const target = str(args, "target");
      const node = target ? page.labels.get(target) : undefined;
      if (node) entry.label = clip(node.name, 60);
    }
    facts.set(item.id, entry);
  }

  const faviconByHost = new Map<string, string>();
  for (const state of Object.values(states)) {
    const host = hostOf(state.url) ?? (state.host || null);
    const icon = safeFavicon(state.favicon);
    if (host && icon) faviconByHost.set(host, icon);
    const action = state.action;
    const known = action ? facts.get(action.tool_call_id) : undefined;
    if (known && !known.label && action?.target) known.label = clip(action.target, 60);
    // The daemon reads the title of the page the tab is on now; older navigations keep what their page read gave.
    const nav = tabs.get(state.tab_id)?.nav;
    if (nav && !nav.title && nav.host === host) nav.title = cleanTitle(state.title);
  }
  for (const entry of facts.values()) entry.favicon = entry.host ? (faviconByHost.get(entry.host) ?? null) : null;
  return facts;
}

// --- group labels ----------------------------------------------------------------------------

function joinWords(parts: string[]): string {
  if (parts.length <= 1) return parts[0] ?? "";
  return `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}`;
}

function plural(count: number, one: string, many: string): string {
  return count === 1 ? one : many.replace("{n}", String(count));
}

/** Hosts of the browser calls, in the order first visited. */
export function hostsOf(tools: readonly ToolItem[], facts: ToolFactsMap): string[] {
  const hosts = new Set<string>();
  for (const tool of tools) {
    if (toolKind(tool.name) !== "browse") continue;
    const host = facts.get(tool.id)?.host ?? hostOf(record(tool.args).url);
    if (host) hosts.add(host);
  }
  return [...hosts];
}

/** The phrase a group of one kind contributes to the group's label, e.g. "read 2 files". */
function kindSummary(kind: ToolKindId, tools: ToolItem[], facts: ToolFactsMap, sole: boolean): string {
  const count = tools.length;
  switch (kind) {
    case "browse": {
      const hosts = hostsOf(tools, facts);
      const where = hosts.length === 0 ? "the web" : hosts.length === 1 ? hosts[0]! : hosts.length === 2 ? `${hosts[0]} and ${hosts[1]}` : `${hosts.length} sites`;
      const steps = tools.filter((tool) => isPageStep(tool) || opensPage(tool)).length;
      return `Browsed ${where}${sole && steps > 1 ? ` · ${steps} steps` : ""}`;
    }
    case "search":
      return "Searched";
    case "read":
      return count === 1 ? describeTool(tools[0]!).title : `Read ${count} files`;
    case "edit": {
      const verbs = new Set(tools.map((tool) => (tool.name === "write" ? "Saved" : "Edited")));
      const verb = verbs.size === 1 ? [...verbs][0]! : "Edited";
      return count === 1 ? describeTool(tools[0]!).title : `${verb} ${count} files`;
    }
    case "shell":
      return plural(count, "Ran a command", "Ran {n} commands");
    case "helper":
      return plural(count, describeTool(tools[0]!).title, "Delegated {n} tasks");
    case "skill":
      return plural(count, describeTool(tools[0]!).title, "Loaded {n} skills");
    case "memory":
      return "Checked memory";
    case "vault":
      return "Used a saved login";
    case "ask":
      return plural(count, describeTool(tools[0]!).title, "Asked you {n} things");
    default:
      return count === 1 ? describeTool(tools[0]!).title : `Used ${count} tools`;
  }
}

/**
 * The label of a run of calls: the work, not the count ("Opened acme.com and read 2 files").
 * A call still running names itself instead, so the label never claims more than has happened.
 */
export function groupSummary(tools: readonly ToolItem[], facts: ToolFactsMap): string {
  const running = tools.findLast((tool) => !tool.done);
  if (running) return describeTool(running, facts.get(running.id)).title;
  const byKind = new Map<ToolKindId, ToolItem[]>();
  for (const tool of tools) {
    const kind = toolKind(tool.name);
    byKind.set(kind, [...(byKind.get(kind) ?? []), tool]);
  }
  const kinds = [...byKind.entries()];
  const shown = kinds.slice(0, 2).map(([kind, list], index) => {
    const text = kindSummary(kind, list, facts, kinds.length === 1);
    return index === 0 ? text : text.charAt(0).toLowerCase() + text.slice(1);
  });
  const more = kinds.length - shown.length;
  return `${joinWords(shown)}${more > 0 ? ` +${more} more` : ""}` || "Worked on it";
}

// --- technical details -----------------------------------------------------------------------

const ID_KEY = /(?:_id|^id)$/i;

/** Ids (tab, ticket, snapshot, message) mean nothing to a person; they show only with `localStorage.openside.debug`. */
export function debugMode(): boolean {
  try {
    return typeof localStorage !== "undefined" && localStorage.getItem("openside.debug") !== null;
  } catch {
    return false;
  }
}

function strip(value: unknown, depth = 0): unknown {
  if (Array.isArray(value)) return depth > 6 ? value : value.map((entry) => strip(entry, depth + 1));
  if (!value || typeof value !== "object" || depth > 6) return value;
  return Object.fromEntries(Object.entries(value as Record<string, unknown>).filter(([key]) => !ID_KEY.test(key)).map(([key, entry]) => [key, strip(entry, depth + 1)]));
}

/** What the "Technical details" disclosure may show: no ids, and never the text typed into a field. */
export function technicalView(tool: ToolItem, value: unknown): unknown {
  let shown = debugMode() ? value : strip(value);
  const args = record(tool.args);
  if (value === tool.args && typeof args.text === "string" && (args.action === "type" || tool.name.toLowerCase() === "browser_pointer")) {
    shown = { ...record(shown), text: `<${args.text.length} characters>` };
  }
  return shown;
}
