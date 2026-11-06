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
import { t, type MessageKey } from "../i18n";
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
  return last.replace(/[^\p{L}\p{N}]+/gu, " ").trim().toLowerCase() || t("timeline.tool.unnamed");
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

const fixed = (kind: ToolKindId, past: MessageKey, present: MessageKey, detail?: Phrase["detail"]): Phrase => ({
  kind,
  past: () => t(past),
  present: () => t(present),
  ...(detail ? { detail } : {}),
});

/** A sentence naming what the call acted on, or the generic one when nothing is known. */
function about(value: string | null, named: MessageKey, unnamed: MessageKey, param: string): string {
  return value ? t(named, { [param]: value }) : t(unnamed);
}

/** Keys of one verb in both tenses: `<base>.past`, `<base>.pastAny`, `<base>.present`, `<base>.presentAny`. */
type Tensed = "open" | "click" | "press" | "choose" | "hover" | "focus" | "readFile" | "saveFile" | "editFile" | "skill" | "savedLogin" | "savedCode" | "passkeyCreate" | "passkeySignIn" | "webFetch";

function past(base: Tensed, value: string | null, param: string): string {
  return about(value, `timeline.tool.${base}.past`, `timeline.tool.${base}.pastAny`, param);
}

function present(base: Tensed, value: string | null, param: string): string {
  return about(value, `timeline.tool.${base}.present`, `timeline.tool.${base}.presentAny`, param);
}

function siteOf(call: Call): string | null {
  return call.facts?.host ?? hostOf(call.args.url) ?? hostOf(`https://${str(call.args, "site") ?? ""}`) ?? null;
}

const open: Phrase = {
  kind: "browse",
  past: ({ args, facts }) => past("open", facts?.title ? clip(facts.title) : (facts?.host ?? hostOf(args.url)), "page"),
  present: ({ args, facts }) => present("open", facts?.host ?? hostOf(args.url), "host"),
  detail: ({ args, facts }) => (facts?.title ? (facts.host ?? hostOf(args.url) ?? undefined) : undefined),
};

const act: Phrase = {
  kind: "browse",
  past: ({ args, facts }) => {
    const label = quoted(facts?.label);
    switch (args.action) {
      case "click":
        return past("click", label, "label");
      case "type": {
        const submitted = args.submit === true;
        if (facts?.label) return t(submitted ? "timeline.tool.fill.pastSubmit" : "timeline.tool.fill.past", { field: field(facts.label) });
        return t(submitted ? "timeline.tool.fill.pastAnySubmit" : "timeline.tool.fill.pastAny");
      }
      case "press":
        return past("press", str(args, "key"), "key");
      case "select":
        return past("choose", label, "label");
      case "hover":
        return past("hover", label, "label");
      case "focus":
        return past("focus", label, "label");
      default:
        return t("timeline.tool.interact.past");
    }
  },
  present: ({ args, facts }) => {
    const label = quoted(facts?.label);
    switch (args.action) {
      case "click":
        return present("click", label, "label");
      case "type":
        return about(facts?.label ? field(facts.label) : null, "timeline.tool.fill.present", "timeline.tool.fill.presentAny", "field");
      case "press":
        return present("press", str(args, "key"), "key");
      case "select":
        return present("choose", label, "label");
      case "hover":
        return present("hover", label, "label");
      case "focus":
        return present("focus", label, "label");
      default:
        return t("timeline.tool.interact.present");
    }
  },
};

const pointer: Phrase = {
  kind: "browse",
  past: ({ args }) => {
    switch (args.action) {
      case "double_click":
        return t("timeline.tool.doubleClick.past");
      case "type":
        return t("timeline.tool.typePage.past");
      case "scroll":
        return t("timeline.tool.scroll.past");
      default:
        return t("timeline.tool.click.pastAny");
    }
  },
  present: ({ args }) => {
    switch (args.action) {
      case "double_click":
        return t("timeline.tool.doubleClick.present");
      case "type":
        return t("timeline.tool.typePage.present");
      case "scroll":
        return t("timeline.tool.scroll.present");
      default:
        return t("timeline.tool.click.presentAny");
    }
  },
};

const file = (call: Call) => baseName(str(call.args, "path"));

const onFile = (kind: ToolKindId, base: "readFile" | "saveFile" | "editFile"): Phrase => ({
  kind,
  past: (call) => past(base, file(call), "file"),
  present: (call) => present(base, file(call), "file"),
});

const onSite = (base: "savedLogin" | "savedCode"): Phrase => ({
  kind: "vault",
  past: (call) => past(base, siteOf(call), "site"),
  present: (call) => present(base, siteOf(call), "site"),
});

/** Every tool the daemon registers for the agent, in the voice of the person watching it work. */
const PHRASES: Record<string, Phrase> = {
  browser_open_tab: open,
  browser_navigate: open,
  browser_tabs: fixed("browse", "timeline.tool.tabs.past", "timeline.tool.tabs.present"),
  browser_downloads: fixed("browse", "timeline.tool.downloads.past", "timeline.tool.downloads.present"),
  browser_snapshot: fixed("browse", "timeline.tool.readPage.past", "timeline.tool.readPage.present"),
  browser_screenshot: fixed("browse", "timeline.tool.readPage.past", "timeline.tool.readPage.present"),
  browser_close_tab: fixed("browse", "timeline.tool.closeTab.past", "timeline.tool.closeTab.present"),
  browser_act: act,
  browser_pointer: pointer,
  browser_repl: fixed("browse", "timeline.tool.repl.past", "timeline.tool.repl.present"),
  browser_space_reset: fixed("browse", "timeline.tool.resetBrowser.past", "timeline.tool.resetBrowser.present"),
  computer_use_apps: fixed("other", "timeline.tool.computerApps.past", "timeline.tool.computerApps.present"),
  computer_use_connect: fixed("other", "timeline.tool.computerConnect.past", "timeline.tool.computerConnect.present"),
  computer_use_screenshot: fixed("other", "timeline.tool.computerLook.past", "timeline.tool.computerLook.present"),
  computer_use_act: fixed("other", "timeline.tool.computerAct.past", "timeline.tool.computerAct.present"),
  computer_use_stop: fixed("other", "timeline.tool.computerStop.past", "timeline.tool.computerStop.present"),
  read: onFile("read", "readFile"),
  write: onFile("edit", "saveFile"),
  edit: onFile("edit", "editFile"),
  bash: fixed("shell", "timeline.tool.command.past", "timeline.tool.command.present", ({ args }) => firstLine(str(args, "command"))),
  request_human_step: fixed("ask", "timeline.tool.humanStep.past", "timeline.tool.humanStep.present"),
  ask_user: fixed("ask", "timeline.tool.question.past", "timeline.tool.question.present", ({ args }) => firstLine(str(args, "question"))),
  request_login: fixed("ask", "timeline.tool.login.past", "timeline.tool.login.present"),
  request_choice: fixed("ask", "timeline.tool.choice.past", "timeline.tool.choice.present"),
  wait_for_approval: fixed("ask", "timeline.tool.approval.past", "timeline.tool.approval.present"),
  request_otp: fixed("ask", "timeline.tool.code.past", "timeline.tool.code.present"),
  register_authenticator: fixed("vault", "timeline.tool.authenticator.past", "timeline.tool.authenticator.present"),
  memory_search: fixed("memory", "timeline.tool.memorySearch.past", "timeline.tool.memorySearch.present", ({ args }) => firstLine(str(args, "query"))),
  websearch: fixed("search", "timeline.tool.webSearch.past", "timeline.tool.webSearch.present", ({ args }) => firstLine(str(args, "query"))),
  webfetch: {
    kind: "browse",
    past: ({ args, facts }) => past("webFetch", facts?.title ? clip(facts.title) : (facts?.host ?? hostOf(args.url)), "page"),
    present: ({ args, facts }) => present("webFetch", facts?.host ?? hostOf(args.url), "host"),
    detail: ({ args, facts }) => (facts?.title ? (facts.host ?? hostOf(args.url) ?? undefined) : undefined),
  },
  memory_write: fixed("memory", "timeline.tool.memoryWrite.past", "timeline.tool.memoryWrite.present"),
  skill: {
    kind: "skill",
    past: ({ args }) => past("skill", str(args, "name"), "name"),
    present: ({ args }) => present("skill", str(args, "name"), "name"),
  },
  vault_login: onSite("savedLogin"),
  vault_otp: onSite("savedCode"),
  vault_passkey: {
    kind: "vault",
    past: (call) => (call.args.action === "register" ? past("passkeyCreate", siteOf(call), "site") : past("passkeySignIn", siteOf(call), "site")),
    present: (call) => (call.args.action === "register" ? present("passkeyCreate", siteOf(call), "site") : present("passkeySignIn", siteOf(call), "site")),
  },
  delegate: fixed("helper", "timeline.tool.delegate.past", "timeline.tool.delegate.present", ({ args }) => firstLine(str(args, "task"))),
  create_task: fixed("helper", "timeline.tool.startTask.past", "timeline.tool.startTask.present", ({ args }) => firstLine(str(args, "goal"))),
  steer_task: fixed("helper", "timeline.tool.updateTask.past", "timeline.tool.updateTask.present"),
  cancel_task: fixed("helper", "timeline.tool.cancelTask.past", "timeline.tool.cancelTask.present"),
  get_tasks: fixed("helper", "timeline.tool.checkTasks.past", "timeline.tool.checkTasks.present"),
  send_message: fixed("other", "timeline.tool.sendMessage.past", "timeline.tool.sendMessage.present"),
};

/** Tool names the daemon registers; the table above must speak for each of them. */
export function hasPhrase(name: string): boolean {
  return name in PHRASES;
}

/** Tools no one wrote copy for (plugin or future tools) still get a sentence and a sensible icon. */
function generic(name: string): Phrase {
  const lower = name.toLowerCase();
  const kind: ToolKindId = /search|grep|glob|find/.test(lower)
    ? "search"
    : /fetch|web|url|browse/.test(lower)
      ? "browse"
      : /shell|exec|command|terminal/.test(lower)
        ? "shell"
        : "other";
  if (kind === "search") return fixed("search", "timeline.tool.search.past", "timeline.tool.search.present");
  return {
    kind,
    past: () => t("timeline.tool.generic.past", { name: humanize(name) }),
    present: () => t("timeline.tool.generic.present", { name: humanize(name) }),
  };
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
    if (name === "webfetch") {
      // The daemon's citation record names the page it read; no tab is involved.
      const result = record(item.result);
      const icon = record(result.favicon_ref);
      facts.set(item.id, {
        host: hostOf(result.url) ?? hostOf(record(item.args).url) ?? null,
        title: typeof result.title === "string" && result.title ? cleanTitle(result.title) : null,
        label: null,
        favicon: safeFavicon(icon.data_url),
      });
      continue;
    }
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
  for (const entry of facts.values()) entry.favicon = (entry.host ? faviconByHost.get(entry.host) : undefined) ?? entry.favicon;
  return facts;
}

// --- durations -------------------------------------------------------------------------------

/**
 * Whole seconds between two daemon event times; null when either is missing or unreadable, so a
 * duration is never estimated. Floored: 0.6s reads "Worked", 1.9s "Worked for 1s".
 */
export function spanSeconds(start: string | undefined, end: string | number | undefined): number | null {
  const from = start ? Date.parse(start) : NaN;
  const to = typeof end === "number" ? end : end ? Date.parse(end) : NaN;
  if (Number.isNaN(from) || Number.isNaN(to) || to < from) return null;
  return Math.floor((to - from) / 1000);
}

/** 45 → "45s", 125 → "2m 5s", 3720 → "1h 2m". */
export function formatDuration(seconds: number): string {
  if (seconds < 60) return t("timeline.duration.seconds", { seconds });
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return seconds % 60 ? t("timeline.duration.minutesSeconds", { minutes, seconds: seconds % 60 }) : t("timeline.duration.minutes", { minutes });
  const hours = Math.floor(minutes / 60);
  return minutes % 60 ? t("timeline.duration.hoursMinutes", { hours, minutes: minutes % 60 }) : t("timeline.duration.hours", { hours });
}

/** A turn's work line: "Working for 4s" while it runs, "Worked for 12s" after; plain "Working"/"Worked" under a second or untimed. */
export function workLabel(seconds: number | null, live: boolean): string {
  if (!seconds) return t(live ? "timeline.activity.working" : "timeline.activity.worked");
  return t(live ? "timeline.activity.workingFor" : "timeline.activity.workedFor", { duration: formatDuration(seconds) });
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
    shown = { ...record(shown), text: t("timeline.tool.hiddenText", { count: args.text.length }) };
  }
  return shown;
}
