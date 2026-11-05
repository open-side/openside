/**
 * `/name` skill mentions: progressive disclosure without a model round trip.
 *
 * The daemon lists the enabled skills in a system-prompt block and owns the `skill`
 * tool that returns a skill's instructions. When a user message mentions `/name` for
 * a skill in that list, the sidecar calls the same daemon tool *before* the model
 * request and puts the call and its result into the history. The model starts with
 * the instructions loaded and the session transcript shows the load. A skill that is
 * no longer loadable (disabled or removed since the session opened) is skipped
 * silently: the daemon already said no.
 */

import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import type { Api, AssistantMessage, Provider, ToolCall, ToolResultMessage } from "@oh-my-pi/pi-ai";
import type { Done, JsonObject, ToolOutcome } from "../protocol/types";
import { PY_SPACE, splitlines, strip } from "./pytext";

export const SKILL_TOOL = "skill";
// Must match `INDEX_OPEN`/`INDEX_CLOSE` in `crates/opensided/src/skills.rs`.
export const INDEX_OPEN = "<available_skills>";
export const INDEX_CLOSE = "</available_skills>";
export const MAX_MENTIONS = 5;

const ENTRY = /^- ([a-z0-9]+(?:-[a-z0-9]+)*):/u;
// A mention starts a word and ends before whitespace or sentence punctuation; a
// following `/` or `.name` means it is a path or file name, not a mention.
const MENTION_SOURCE = `(?:^|(?<=[${PY_SPACE}(\\[{"']))/([a-z0-9]+(?:-[a-z0-9]+)*)(?=$|[${PY_SPACE})\\]}"',;!?]|[.:](?:$|[${PY_SPACE}]))`;

/** One daemon tool call through the reverse RPC, already bound to the session/subagent. */
export type ToolInvoker = (toolCallId: string, name: string, args: JsonObject) => Promise<ToolOutcome>;

/** The tool-result history message for a completed call (the runtime's `done` mapping). */
export type DoneToResult = (toolCallId: string, toolName: string, outcome: Done) => ToolResultMessage;

/** Model identity stamped on the synthetic assistant turn that holds the skill calls. */
export interface MentionOrigin {
	api: Api;
	provider: Provider;
	model: string;
}

/** Skill names listed in the daemon's `<available_skills>` block. */
export function indexNames(promptParts: Iterable<string>): string[] {
	const names: string[] = [];
	for (const part of promptParts) {
		const start = part.indexOf(INDEX_OPEN);
		const end = part.indexOf(INDEX_CLOSE);
		if (start < 0 || end < start) continue;
		for (const line of splitlines(part.slice(start + INDEX_OPEN.length, end))) {
			const entry = ENTRY.exec(strip(line));
			if (entry && !names.includes(entry[1] as string)) names.push(entry[1] as string);
		}
	}
	return names;
}

/** Known skill names mentioned as `/name`, in order, without repeats. */
export function mentioned(text: string, names: Iterable<string>): string[] {
	const known = new Set(names);
	const found: string[] = [];
	for (const match of text.matchAll(new RegExp(MENTION_SOURCE, "gu"))) {
		const name = match[1] as string;
		if (known.has(name) && !found.includes(name)) found.push(name);
	}
	return found.slice(0, MAX_MENTIONS);
}

/**
 * History messages that load every skill mentioned in `texts`: one assistant turn
 * with the `skill` tool calls, then one tool result per loaded skill.
 */
export async function loadMentions(
	call: ToolInvoker,
	names: Iterable<string>,
	texts: Iterable<string>,
	origin: MentionOrigin,
	toResult: DoneToResult,
): Promise<AgentMessage[]> {
	const known = Array.from(names);
	const wanted: string[] = [];
	for (const text of texts) {
		for (const name of mentioned(text, known)) {
			if (!wanted.includes(name)) wanted.push(name);
		}
	}
	const calls: ToolCall[] = [];
	const results: ToolResultMessage[] = [];
	for (const name of wanted.slice(0, MAX_MENTIONS)) {
		const toolCallId = `skill-mention-${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;
		const outcome = await call(toolCallId, SKILL_TOOL, { name });
		if (outcome.status !== "done") continue;
		calls.push({ type: "toolCall", id: toolCallId, name: SKILL_TOOL, arguments: { name } });
		results.push(toResult(toolCallId, SKILL_TOOL, outcome));
	}
	if (!calls.length) return [];
	const turn: AssistantMessage = {
		role: "assistant",
		content: calls,
		api: origin.api,
		provider: origin.provider,
		model: origin.model,
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: "toolUse",
		timestamp: Date.now(),
	};
	return [turn, ...results];
}
