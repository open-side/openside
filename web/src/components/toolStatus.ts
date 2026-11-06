import type { BrowserState } from "../api/types";
import { t, tMaybe, type MessageKey } from "../i18n";
import type { TimelineItem } from "../state/session";

type ToolItem = Extract<TimelineItem, { type: "tool" }>;

/** `https://nid.naver.com/login` → `nid.naver.com`; anything unparsable → null. */
export function hostOf(url: unknown): string | null {
  if (typeof url !== "string" || !url) return null;
  try {
    const { protocol, hostname } = new URL(url);
    return protocol === "http:" || protocol === "https:" ? hostname.replace(/^www\./, "") || null : null;
  } catch {
    return null;
  }
}

function pageFacet(value: unknown): string | null {
  if (typeof value !== "string") return null;
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || url.hostname !== "github.com" || url.port ||
      !["/trending", "/trending/developers"].includes(url.pathname)) return null;
    const periods = url.searchParams.getAll("since");
    if (periods.length !== 1) return null;
    const period = periods[0]!;
    return period === "daily" || period === "weekly" || period === "monthly" ? t(`timeline.status.trending.${period}`) : null;
  } catch {
    return null;
  }
}

function urlIn(value: unknown): string | null {
  if (!value || typeof value !== "object") return null;
  const url = (value as Record<string, unknown>).url;
  const host = hostOf(url);
  const facet = pageFacet(url);
  return host && facet ? `${host} (${facet})` : host;
}

/**
 * The site the agent's browser was last seen on, from navigate/open arguments and tool results.
 * Click and type calls only name a tab, so their phrase borrows this host.
 */
export function lastHost(items: readonly TimelineItem[]): string | null {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index]!;
    if (item.type !== "tool") continue;
    const host = urlIn(item.result) ?? urlIn(item.args);
    if (host) return host;
  }
  return null;
}

function argOf(tool: ToolItem, key: string): string | null {
  const args = tool.args;
  if (!args || typeof args !== "object") return null;
  const value = (args as Record<string, unknown>)[key];
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

/** `browser_act` / `browser_pointer` actions with a status line of their own; any other reads as looking. */
const PAGE_VERBS = ["click", "type", "press", "select", "scroll"] as const;

const TASK_TOOLS: Record<string, MessageKey> = {
  create_task: "timeline.status.startingTask",
  steer_task: "timeline.status.updatingTask",
  cancel_task: "timeline.status.cancellingTask",
  get_tasks: "timeline.status.checkingTasks",
  send_message: "timeline.status.writing",
};

/** Message id prefix the daemon gives a finished task's report to the front agent. */
export const TASK_REPORT_PREFIX = "task-report-";

/**
 * What a person would say the agent is doing during `tool`: "Clicking on
 * nid.naver.com", "Reading a page". `host` is the site the browser is on, if known.
 */
export function toolPhrase(tool: ToolItem, host: string | null): string {
  const name = tool.name.toLowerCase();
  const site = urlIn(tool.args) ?? host;
  const file = argOf(tool, "path")?.split(/[\\/]/).filter(Boolean).at(-1) ?? null;
  if (name === "browser_navigate" || name === "browser_open_tab") {
    const target = urlIn(tool.args);
    return target ? t("timeline.status.opening", { site: target }) : t("timeline.status.loadingBrowser");
  }
  if (name === "browser_act" || name === "browser_pointer") {
    const action = argOf(tool, "action");
    const verb = PAGE_VERBS.find((candidate) => candidate === action) ?? "look";
    return site ? t(`timeline.status.${verb}.site`, { site }) : t(`timeline.status.${verb}.page`);
  }
  if (name === "browser_snapshot" || name === "browser_screenshot") return t("timeline.status.readingPage");
  if (name === "browser_tabs") return t("timeline.status.checkingTabs");
  if (name === "browser_downloads") return t("timeline.status.checkingDownloads");
  if (name === "browser_close_tab") return t("timeline.status.closingTab");
  if (name.startsWith("browser")) return site ? t("timeline.status.workingOn", { site }) : t("timeline.status.usingBrowser");
  if (/request_otp|request_human_step|ask_user|approval|takeover|register_authenticator/.test(name)) return t("timeline.status.waiting");
  if (/fetch|web/.test(name)) return t("timeline.status.readingPage");
  if (/search|grep|glob|find/.test(name)) return t("timeline.status.searching");
  if (/write|edit|patch|create_file/.test(name)) return file ? t("timeline.status.editing", { file }) : t("timeline.status.editingFiles");
  if (/^read|list|^ls/.test(name)) return file ? t("timeline.status.reading", { file }) : t("timeline.status.readingFiles");
  if (/bash|shell|exec|command|terminal/.test(name)) return t("timeline.status.runningCommand");
  if (/delegate|subagent/.test(name)) return t("timeline.status.delegating");
  const taskVerb = TASK_TOOLS[name];
  if (taskVerb) return t(taskVerb);
  if (/skill/.test(name)) return t("timeline.status.loadingSkill");
  if (/memory|remember|recall/.test(name)) return t("timeline.status.checkingMemory");
  if (/vault|secret|credential/.test(name)) return t("timeline.status.usingVault");
  return t("timeline.status.working");
}

/**
 * The transient status line under the latest message while a run has nothing new to show:
 * the running call's phrase, "Reading a page" right after a browser call returned (the model
 * is reading its result), otherwise "Thinking".
 */
export function statusPhrase(tools: readonly ToolItem[], host: string | null): string {
  const running = tools.findLast((tool) => !tool.done);
  if (running) return toolPhrase(running, host);
  const last = tools.at(-1);
  if (last?.name.toLowerCase().startsWith("browser")) return t("timeline.status.readingPage");
  return t("timeline.status.thinking");
}

/**
 * One line for a tab's latest `browser.state`, as a person would narrate it: "Typing on
 * nid.example.com", "Opening github.com", "Waiting for you".
 */
export function browserLine(state: BrowserState): string {
  const site = state.host || null;
  const facet = pageFacet(state.url);
  let line: string;
  if (state.phase === "waiting_human") line = t("timeline.status.waiting");
  else if (state.phase === "error") line = state.facts.error_text || (site ? t("timeline.browser.loadFailed", { site }) : t("timeline.browser.loadFailedAny"));
  else if (state.action) {
    // The daemon words `phrase` itself (English); only the connective around it is ours.
    const { phrase, target } = state.action;
    line = target ? `${phrase} ${target}` : /\s/.test(phrase) ? phrase : site ? t("timeline.browser.actionOn", { phrase, site }) : t("timeline.browser.actionOnAny", { phrase });
  } else if (state.phase === "loading") line = site ? t("timeline.status.opening", { site }) : t("timeline.browser.openingAny");
  else line = site ? t("timeline.browser.reading", { site }) : t("timeline.status.readingPage");
  return facet ? `${line} (${facet})` : line;
}

const OUTCOME_NOTES: Record<string, MessageKey> = {
  "failed:interrupted": "timeline.outcome.interrupted",
  "failed:auth_unavailable": "timeline.outcome.authUnavailable",
  "aborted:stopped": "timeline.outcome.stopped",
  "aborted:deleted": "timeline.outcome.removed",
};

/** The small line under a message whose run did not complete; the daemon's reason codes read as sentences. */
export function outcomeNote(outcome: string, reason: string | undefined): string {
  const known = OUTCOME_NOTES[`${outcome}:${reason}`];
  if (known) return t(known);
  const label = tMaybe(`timeline.outcome.${outcome}`) ?? outcome.charAt(0).toUpperCase() + outcome.slice(1);
  if (!reason) return label;
  return tMaybe(`timeline.outcome.${outcome}Reason`, { reason }) ?? `${label}: ${reason}`;
}

/** A conversation titled with a bare URL reads as its site. */
export function displayTitle(title: string): string {
  const trimmed = title.trim();
  return /^\S+$/.test(trimmed) ? (hostOf(trimmed) ?? title) : title;
}
