import type { SubagentView, TimelineItem } from "../state/session";
import { describeTool } from "./toolCopy";
import { toolPhrase } from "./toolStatus";

type ToolItem = Extract<TimelineItem, { type: "tool" }>;

/** One call a Task made, in the words its row and detail sheet use. */
export interface TaskStep {
  id: string;
  text: string;
  state: "running" | "done" | "failed";
}

type Entry = Record<string, unknown>;

const text = (value: unknown): string | null => (typeof value === "string" && value.trim() ? value.trim() : null);

/**
 * The Task's calls rebuilt from its `subagent.progress` stream, where the daemon wraps each of
 * its `tool.started` / `tool.ended` events; `status` is the daemon's one-line `status_text`.
 */
export function taskCalls(progress: readonly unknown[]): (ToolItem & { status?: string })[] {
  const calls = new Map<string, ToolItem & { status?: string }>();
  for (const entry of progress as Entry[]) {
    if (!entry || typeof entry !== "object") continue;
    const id = text(entry.tool_call_id);
    if (!id) continue;
    const status = text(entry.status_text);
    if (entry.kind === "tool.started" && typeof entry.name === "string") {
      calls.set(id, { type: "tool", id, name: entry.name, args: entry.args, updates: [], done: false, ...(status ? { status } : {}) });
    } else if (entry.kind === "tool.ended") {
      const call = calls.get(id);
      if (call) calls.set(id, { ...call, done: true, result: entry.result, isError: entry.is_error === true, ...(status ? { status } : {}) });
    }
  }
  return [...calls.values()];
}

/** Every call of the Task in order, as people would say it: "Opened github.com", "Read a page". */
export function taskSteps(progress: readonly unknown[]): TaskStep[] {
  return taskCalls(progress).map((call) => ({
    id: call.id,
    text: call.status ?? (call.done ? describeTool(call).title : toolPhrase(call, null)),
    state: !call.done ? "running" : call.isError ? "failed" : "done",
  }));
}

/** First sentence of `text` without Markdown marks. */
export function firstSentence(value: string): string | null {
  const plain = value.replace(/[*_`#>]+/g, "").replace(/\s+/g, " ").trim();
  return plain.split(/(?<=[.!?。])\s/)[0] || null;
}

/** The one line a Task row shows under its title: what it did last, or how it ended. */
export function taskLine(task: SubagentView): string {
  switch (task.status) {
    case "completed":
      return (task.output ? firstSentence(task.output) : null) ?? "Done";
    case "failed":
      return task.reason?.trim() || "Couldn’t finish";
    case "cancelled":
      return "Cancelled";
    case "waiting_human":
      return "Waiting for you";
    case "running": {
      const last = taskSteps(task.progress).at(-1);
      if (last) return last.text;
      return task.progress.length ? "Thinking" : "Starting";
    }
  }
}
