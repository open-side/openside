/**
 * Ultrabrowse research mode (A-9, docs/specs/ultrabrowse.md).
 *
 * The daemon narrows tools and prompt for the `ultrabrowse` profile; this module adds
 * what only the agent loop can do:
 *
 * - higher reasoning: `modelSettings` asks for high thinking effort;
 * - a planning step: browser tools are refused until the current turn states a
 *   `Plan:`, and that plan is re-injected on every later request (`withPlanReminder`);
 * - enforced citations: `citationGuard` sends answers with uncited claims back to the
 *   model, then annotates them with `[citation needed]` once retries are spent.
 *
 * The plan is re-derived from history on every request, so it survives sidecar
 * restarts and deferred resumes without its own persistence. User messages the runtime
 * injects that are not a new task (citation retries, the plan reminder, the prompt
 * epoch tail) carry `synthetic: true` and do not start a new turn.
 */

import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import type { UserMessage } from "@oh-my-pi/pi-ai";
import * as citations from "./citations";
import { PY_SPACE, splitlines, strip } from "./pytext";

export const ULTRABROWSE = "ultrabrowse";
// Thinking effort per profile; other profiles keep the provider's default behavior.
export const THINKING: Record<string, "high"> = { [ULTRABROWSE]: "high" };
// Results whose `url` field is the page the tool loaded or acted on. `websearch` is not one:
// a result snippet is not a page the model read.
export const VISITING_TOOLS: ReadonlySet<string> = new Set([
	"browser_open_tab",
	"browser_navigate",
	"browser_act",
	"browser_repl",
	"webfetch",
]);
export const CITATION_RETRIES = 2;
export const PLAN_REQUIRED =
	"Before using web or browser tools, write a line `Plan:` followed by numbered research steps, then browse.";
const WS = `[${PY_SPACE}]`;
const PLAN_HEADING = new RegExp(`^${WS}*(?:#{1,6}${WS}*)?\\**plan\\**${WS}*:?\\**${WS}*(.*)$`, "iu");
const PLAN_ITEM = new RegExp(`^${WS}*(?:\\d+[.)]|[-*+])${WS}+(.+)$`, "u");

export type GuardDecision =
	| { action: "allow" }
	| { action: "retry"; message: string }
	| { action: "replace"; text: string };

export function isResearch(profile: unknown): boolean {
	return profile === ULTRABROWSE;
}

/** Provider-neutral thinking setting; the runtime maps it to the stream's reasoning effort. */
export function modelSettings(profile: unknown): { thinking: "high" } | undefined {
	const thinking = typeof profile === "string" && Object.hasOwn(THINKING, profile) ? THINKING[profile] : undefined;
	return thinking !== undefined ? { thinking } : undefined;
}

/** Messages since the latest user prompt (a new or steered task). */
export function currentTurn(messages: readonly AgentMessage[]): readonly AgentMessage[] {
	for (let index = messages.length - 1; index >= 0; index -= 1) {
		const message = messages[index];
		if (message?.role === "user" && !message.synthetic) return messages.slice(index + 1);
	}
	return messages;
}

/** Steps of the last `Plan:` block in `text`. */
export function parsePlan(text: string): string[] {
	let steps: string[] = [];
	let collecting = false;
	for (const line of splitlines(text)) {
		const heading = PLAN_HEADING.exec(line);
		if (heading) {
			const first = strip(heading[1] as string);
			steps = first ? [first] : [];
			collecting = true;
			continue;
		}
		if (!collecting) continue;
		const item = PLAN_ITEM.exec(line);
		if (item) {
			steps.push(strip(item[1] as string));
		} else if (strip(line)) {
			collecting = false;
		}
	}
	return steps;
}

/** The plan the model stated in the current turn; empty when it has not planned. */
export function planSteps(messages: readonly AgentMessage[]): string[] {
	let steps: string[] = [];
	for (const message of currentTurn(messages)) {
		if (message.role !== "assistant") continue;
		for (const part of message.content) {
			if (part.type === "text") {
				const parsed = parsePlan(part.text);
				if (parsed.length) steps = parsed;
			}
		}
	}
	return steps;
}

/** Pages the daemon's browser tools reported loading in this conversation. */
export function visitedSources(messages: readonly AgentMessage[]): Set<string> {
	const found = new Set<string>();
	for (const message of messages) {
		if (message.role !== "toolResult" || !VISITING_TOOLS.has(message.toolName)) continue;
		for (const url of citations.resultUrls(message.details)) found.add(url);
	}
	return found;
}

export function retryMessage(report: citations.Report, visited: Set<string>): string {
	const claims = report.uncited.map(claim => `- ${claim.text}`).join("\n");
	const pages =
		Array.from(visited)
			.sort()
			.map(url => `- ${url}`)
			.join("\n") || "- (none yet)";
	return (
		"These factual sentences have no titled link to a page you opened with webfetch or a browser tool:\n" +
		`${claims}\n` +
		"Pages opened in this session:\n" +
		`${pages}\n` +
		"Rewrite the answer: cite each fact with an inline Markdown link `[Page title](url)` to one of these pages (bare URLs and reference-style links do not count), browse for a source first, or drop the fact or mark it " +
		`${citations.MARKER}.`
	);
}

/**
 * Decision for a final text answer. `retry` is how many times this answer was already
 * sent back; `maxRetries` is the session's retry budget (`agent.tool_retries`).
 */
export function citationGuard(
	messages: readonly AgentMessage[],
	output: string,
	retry: number,
	maxRetries: number,
): GuardDecision {
	const visited = visitedSources(messages);
	const report = citations.check(output, visited);
	if (!report.uncited.length) return { action: "allow" };
	if (retry < Math.min(CITATION_RETRIES, maxRetries))
		return { action: "retry", message: retryMessage(report, visited) };
	// Retries are spent: deliver the answer, visibly marking what stayed unsupported.
	return { action: "replace", text: citations.annotate(output, report) };
}

/** Refusal text for a web or browser tool called before the current turn states a plan. */
export function planGate(toolName: string, messages: readonly AgentMessage[]): string | undefined {
	const gated = toolName.startsWith("browser_") || toolName === "websearch" || toolName === "webfetch";
	return gated && !planSteps(messages).length ? PLAN_REQUIRED : undefined;
}

/**
 * The request context with the current plan appended as an ephemeral reminder (never
 * persisted). Nothing is added without a plan or when the context ends in a model turn.
 */
export function withPlanReminder(context: readonly AgentMessage[]): AgentMessage[] {
	const steps = planSteps(context);
	const last = context[context.length - 1];
	if (!steps.length || last === undefined || last.role === "assistant") return [...context];
	const lines = steps.map((step, index) => `${index + 1}. [ ] ${step}`);
	lines.push(`(0/${steps.length} completed)`);
	const reminder: UserMessage = {
		role: "user",
		content: [
			{ type: "text", text: "<plan-reminder>\n" },
			{
				type: "text",
				text: `Your current plan (keep it updated with the planning tools):\n\n${lines.join("\n")}\n</plan-reminder>`,
			},
		],
		synthetic: true,
		timestamp: Date.now(),
	};
	return [...context, reminder];
}

/**
 * Whether text deltas wait until the response proves not to be the final answer.
 *
 * A final answer can be sent back by the citation guard, so ultrabrowse only streams
 * text that accompanies tool calls; the answer arrives once, as assistant.final.
 */
export function holdsFinalText(profile: unknown): boolean {
	return isResearch(profile);
}
