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

function urlIn(value: unknown): string | null {
  if (!value || typeof value !== "object") return null;
  return hostOf((value as Record<string, unknown>).url);
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

const ON_PAGE: Record<string, string> = {
  click: "Clicking on",
  type: "Typing on",
  press: "Pressing a key on",
  select: "Choosing an option on",
  scroll: "Scrolling",
};

const TASK_TOOLS: Record<string, string> = {
  task_start: "Starting a task",
  task_steer: "Updating a task",
  task_cancel: "Cancelling a task",
  task_status: "Checking on tasks",
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
    return target ? `Opening ${target}` : "Loading a browser";
  }
  if (name === "browser_act" || name === "browser_pointer") {
    const verb = ON_PAGE[argOf(tool, "action") ?? ""];
    if (verb) return site ? `${verb} ${site}` : `${verb} the page`;
    return site ? `Looking at ${site}` : "Looking at the page";
  }
  if (name === "browser_snapshot" || name === "browser_screenshot") return "Reading a page";
  if (name === "browser_tabs") return "Checking open tabs";
  if (name === "browser_close_tab") return "Closing a tab";
  if (name.startsWith("browser")) return site ? `Working on ${site}` : "Using the browser";
  if (/request_otp|request_human_step|ask_user|approval|takeover|register_authenticator/.test(name)) return "Waiting for you";
  if (/fetch|web/.test(name)) return "Reading a page";
  if (/search|grep|glob|find/.test(name)) return "Searching";
  if (/write|edit|patch|create_file/.test(name)) return file ? `Editing ${file}` : "Editing files";
  if (/^read|list|^ls/.test(name)) return file ? `Reading ${file}` : "Reading files";
  if (/bash|shell|exec|command|terminal/.test(name)) return "Running a command";
  if (/delegate|subagent/.test(name)) return "Delegating work";
  const taskVerb = TASK_TOOLS[name];
  if (taskVerb) return taskVerb;
  if (/skill/.test(name)) return "Loading a skill";
  if (/memory|remember|recall/.test(name)) return "Checking memory";
  if (/vault|secret|credential/.test(name)) return "Using the vault";
  return "Working";
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
  if (last?.name.toLowerCase().startsWith("browser")) return "Reading a page";
  return "Thinking";
}

/** The calls a background task's model made, in order, as people would name them. */
export function taskSteps(progress: readonly unknown[]): string[] {
  const steps: string[] = [];
  for (const entry of progress) {
    if (!entry || typeof entry !== "object") continue;
    const { kind, tool_calls: calls } = entry as { kind?: unknown; tool_calls?: unknown };
    if (kind !== "model_response" || !Array.isArray(calls)) continue;
    for (const name of calls) {
      if (typeof name !== "string") continue;
      // Progress names the calls without their arguments, so one tool reads the same each time.
      const step = toolPhrase({ type: "tool", id: "", name, args: undefined, updates: [], done: true }, null);
      if (steps.at(-1) !== step) steps.push(step);
    }
  }
  return steps;
}

/** What a running task is doing now: waiting on its model, or the calls it last asked for. */
export function taskPhrase(progress: readonly unknown[]): string {
  const last = progress.at(-1) as { kind?: unknown; tool_calls?: unknown } | undefined;
  if (last?.kind === "model_request") return "Thinking";
  if (last?.kind === "model_response" && Array.isArray(last.tool_calls) && last.tool_calls.length) {
    return taskSteps([last]).at(-1) ?? "Working";
  }
  return "Working";
}

const OUTCOME_NOTES: Record<string, string> = {
  "failed:interrupted": "Interrupted",
  "failed:auth_unavailable": "Not sent: the model sign-in is unavailable",
  "aborted:stopped": "Stopped",
  "aborted:deleted": "Removed",
};

/** The small line under a message whose run did not complete; the daemon's reason codes read as sentences. */
export function outcomeNote(outcome: string, reason: string | undefined): string {
  const known = OUTCOME_NOTES[`${outcome}:${reason}`];
  if (known) return known;
  const label = outcome.charAt(0).toUpperCase() + outcome.slice(1);
  return reason ? `${label}: ${reason}` : label;
}

/** A conversation titled with a bare URL reads as its site. */
export function displayTitle(title: string): string {
  const trimmed = title.trim();
  return /^\S+$/.test(trimmed) ? (hostOf(trimmed) ?? title) : title;
}
