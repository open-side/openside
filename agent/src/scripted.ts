// Offline model script format from docs/specs/sidecar-protocol.md §9 (OPENSIDE_AGENT_SCRIPT).
import { readFileSync } from "node:fs";
import type { Context, Message } from "@oh-my-pi/pi-ai";
import { createMockModel, type MockContent, type MockModel, registerMockApi } from "@oh-my-pi/pi-ai/providers/mock";

// Registered once so streamSimple (and any streamFn that delegates to it) can drive
// scripted models through the regular provider dispatch.
registerMockApi("openside/scripted");

export type ScriptPartType = "text" | "tool_call" | "wait_steer" | "media_digest" | "echo";

export interface ScriptPart {
	/**
	 * media_digest: reply with `mime:sha256` of every image the model received, so offline
	 * tests can prove attachments and tool images reached it. echo: reply with the latest
	 * user prompt text, so offline tests can prove which instruction (e.g. a steer) reached
	 * this agent.
	 */
	type: ScriptPartType;
	text: string;
	name: string;
	args: Record<string, unknown>;
	id?: string;
	/** wait_steer only: bound for this wait instead of the script-wide delay_ms. */
	timeout_ms?: number;
}

export interface ScriptResponse {
	parts: ScriptPart[];
}

export interface Script {
	responses: ScriptResponse[];
	delay_ms: number;
	title: string;
	/**
	 * Child turns keyed by the parent's `delegate` tool_call id, which is stable across
	 * restarts, unlike creation order of concurrently delegated children.
	 */
	subagents: Record<string, ScriptResponse[]>;
}

/** Same JSON as the snapshot's `script_cursor`; `scriptedModel` advances `index` in place. */
export type ScriptCursor = { index: number };

export type ScriptRole = "parent" | { subagent: string };

const DEFAULT_TITLE = "Discuss the request";
const PART_TYPES = new Set<string>(["text", "tool_call", "wait_steer", "media_digest", "echo"]);
const PART_KEYS = new Set(["type", "text", "name", "args", "id", "timeout_ms"]);

class ScriptError extends Error {
	constructor(where: string) {
		super(`Invalid agent script: ${where}`);
		this.name = "ScriptError";
	}
}

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nonNegativeInt(value: unknown, where: string): number {
	if (typeof value !== "number" || !Number.isInteger(value) || value < 0) throw new ScriptError(where);
	return value;
}

function parsePart(value: unknown, where: string): ScriptPart {
	if (!isObject(value)) throw new ScriptError(where);
	for (const key of Object.keys(value)) if (!PART_KEYS.has(key)) throw new ScriptError(`${where}.${key}`);
	const { type, text = "", name = "", args = {}, id, timeout_ms } = value;
	if (typeof type !== "string" || !PART_TYPES.has(type)) throw new ScriptError(`${where}.type`);
	if (typeof text !== "string") throw new ScriptError(`${where}.text`);
	if (typeof name !== "string") throw new ScriptError(`${where}.name`);
	if (!isObject(args)) throw new ScriptError(`${where}.args`);
	if (id != null && typeof id !== "string") throw new ScriptError(`${where}.id`);
	const part: ScriptPart = { type: type as ScriptPartType, text, name, args };
	if (id != null) part.id = id;
	if (timeout_ms != null) part.timeout_ms = nonNegativeInt(timeout_ms, `${where}.timeout_ms`);
	return part;
}

function parseResponses(value: unknown, where: string): ScriptResponse[] {
	if (!Array.isArray(value)) throw new ScriptError(where);
	return value.map((response, index) => {
		if (!isObject(response) || !Array.isArray(response.parts)) throw new ScriptError(`${where}[${index}]`);
		return { parts: response.parts.map((part, at) => parsePart(part, `${where}[${index}].parts[${at}]`)) };
	});
}

/** Validate a parsed script document (protocol §9). */
export function parseScript(value: unknown): Script {
	if (!isObject(value)) throw new ScriptError("document");
	const { responses, delay_ms = 0, title = DEFAULT_TITLE, subagents = {} } = value;
	if (typeof title !== "string") throw new ScriptError("title");
	if (!isObject(subagents)) throw new ScriptError("subagents");
	return {
		responses: parseResponses(responses, "responses"),
		delay_ms: nonNegativeInt(delay_ms, "delay_ms"),
		title,
		subagents: Object.fromEntries(
			Object.entries(subagents).map(([key, turns]) => [key, parseResponses(turns, `subagents.${key}`)]),
		),
	};
}

export function loadScript(path: string): Script {
	return parseScript(JSON.parse(readFileSync(path, "utf8")));
}

export function scriptedTitle(script: Script): string {
	return script.title;
}

/**
 * Latch for `wait_steer`: the session sets it when a steer reaches this agent and clears it
 * at the start of every model request, like the former per-agent `steer_received` event.
 */
export class SteerSignal {
	#set = false;
	#waiters = new Set<() => void>();

	set(): void {
		this.#set = true;
		for (const wake of this.#waiters) wake();
		this.#waiters.clear();
	}

	clear(): void {
		this.#set = false;
	}

	/** Resolves when set, after `timeoutMs`, or on abort, whichever comes first. */
	wait(timeoutMs: number, signal?: AbortSignal): Promise<void> {
		if (this.#set || timeoutMs <= 0 || signal?.aborted) return Promise.resolve();
		const { promise, resolve } = Promise.withResolvers<void>();
		const done = () => {
			clearTimeout(timer);
			signal?.removeEventListener("abort", done);
			this.#waiters.delete(done);
			resolve();
		};
		const timer = setTimeout(done, timeoutMs);
		signal?.addEventListener("abort", done, { once: true });
		this.#waiters.add(done);
		return promise;
	}
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
	if (ms <= 0 || signal?.aborted) return Promise.resolve();
	const { promise, resolve } = Promise.withResolvers<void>();
	const done = () => {
		clearTimeout(timer);
		signal?.removeEventListener("abort", done);
		resolve();
	};
	const timer = setTimeout(done, ms);
	signal?.addEventListener("abort", done, { once: true });
	return promise;
}

function userTexts(message: Message): string[] {
	// Synthetic user messages carry per-request context (memory tail, reminders), not the prompt.
	if (message.role !== "user" || message.synthetic) return [];
	if (typeof message.content === "string") return [message.content];
	return message.content.flatMap(block => (block.type === "text" ? [block.text] : []));
}

/** Latest non-synthetic user prompt text; a leading data-provenance block is skipped. */
export function lastUserText(messages: readonly Message[]): string {
	for (let index = messages.length - 1; index >= 0; index--) {
		const texts = userTexts(messages[index] as Message);
		const [first, second] = texts;
		if (first === undefined) continue;
		return second !== undefined && first.includes('"provenance"') ? second : first;
	}
	return "";
}

export function mediaDigest(messages: readonly Message[]): string {
	const digests: string[] = [];
	for (const message of messages) {
		if ((message.role !== "user" && message.role !== "toolResult") || typeof message.content === "string") continue;
		for (const block of message.content) {
			if (block.type !== "image") continue;
			const hash = new Bun.CryptoHasher("sha256").update(Buffer.from(block.data, "base64")).digest("hex");
			digests.push(`${block.mimeType}:${hash}`);
		}
	}
	return `media ${digests.length > 0 ? digests.join(" ") : "none"}`;
}

const REF = /\{\{result:([^.}]+)\.([^}]+)\}\}/g;

function resultValue(messages: readonly Message[], callId: string, path: string): string {
	const message = messages.find(item => item.role === "toolResult" && item.toolCallId === callId);
	if (message === undefined || message.role !== "toolResult") return "";
	let content: unknown = message.details;
	for (const key of path.split(".")) {
		if (Array.isArray(content) && /^\d+$/.test(key)) {
			content = content[Number(key)];
		} else if (isObject(content)) {
			content = content[key];
		} else {
			return "";
		}
	}
	if (content == null) return "";
	return typeof content === "string" ? content : pythonStr(content);
}

// Former scripts compared against Python's str() of the result value; keep that rendering
// for the scalar cases scripts rely on (booleans and numbers).
function pythonStr(value: unknown): string {
	if (value === true) return "True";
	if (value === false) return "False";
	if (typeof value === "number") return String(value);
	return JSON.stringify(value);
}

/** Resolve `{{user_text}}`, `{{media_digest}}` and `{{result:call.path}}` without hard-coded daemon ids. */
export function resolveRefs(args: Record<string, unknown>, messages: readonly Message[]): Record<string, unknown> {
	const resolve = (item: unknown): unknown => {
		if (typeof item === "string") {
			if (item === "{{user_text}}") return lastUserText(messages);
			if (item === "{{media_digest}}") return mediaDigest(messages);
			return item.replace(REF, (_, callId: string, path: string) => resultValue(messages, callId, path));
		}
		if (Array.isArray(item)) return item.map(resolve);
		if (isObject(item)) return Object.fromEntries(Object.entries(item).map(([key, child]) => [key, resolve(child)]));
		return item;
	};
	return Object.fromEntries(Object.entries(args).map(([key, item]) => [key, resolve(item)]));
}

function partText(part: ScriptPart, messages: readonly Message[]): string {
	if (part.type === "media_digest") return mediaDigest(messages);
	if (part.type === "echo") return lastUserText(messages);
	return part.text;
}

function words(text: string): number {
	const trimmed = text.trim();
	return trimmed === "" ? 0 : trimmed.split(/[\s",.:]+/).length;
}

function messageWords(message: Message): number {
	if (typeof message.content === "string") return words(message.content);
	let count = 0;
	for (const block of message.content) {
		if (block.type === "text") count += words(block.text);
		else if (block.type === "toolCall") count += 1 + words(JSON.stringify(block.arguments));
	}
	return count;
}

/**
 * Word-count usage like the former pydantic-ai FunctionModel (50 base prompt tokens), so
 * offline runs report non-zero, deterministic usage events that benches and budgets can sum.
 */
function estimateUsage(context: Context, content: readonly MockContent[]): { input: number; output: number } {
	let input = 50;
	for (const prompt of context.systemPrompt ?? []) input += words(prompt);
	for (const message of context.messages) input += messageWords(message);
	let output = 0;
	for (const block of content) {
		if (typeof block === "string") output += words(block);
		else if (block.type === "toolCall") output += 1 + words(JSON.stringify(block.arguments));
	}
	return { input, output };
}

/**
 * A mock model that replays `script` one response per model request. A child uses the
 * turns keyed by its parent's `delegate` tool_call id and inherits the script delay.
 */
export function scriptedModel(script: Script, cursor: ScriptCursor, role: ScriptRole, steer: SteerSignal): MockModel {
	const responses = role === "parent" ? script.responses : (script.subagents[role.subagent] ?? []);
	return createMockModel({
		id: "script",
		provider: "script",
		handler: async (context: Context, options) => {
			const response = responses[cursor.index];
			if (response === undefined) return { content: ["[script exhausted]"] };
			cursor.index += 1;
			const wait = response.parts.find(part => part.type === "wait_steer");
			if (wait !== undefined) {
				await steer.wait(wait.timeout_ms ?? script.delay_ms, options?.signal);
			} else {
				await sleep(script.delay_ms, options?.signal);
			}
			const content: MockContent[] = [];
			for (const part of response.parts) {
				if (part.type === "tool_call") {
					content.push({
						type: "toolCall",
						...(part.id === undefined ? {} : { id: part.id }),
						name: part.name,
						arguments: resolveRefs(part.args, context.messages),
					});
				} else if (part.type !== "wait_steer") {
					content.push(partText(part, context.messages));
				}
			}
			if (content.length === 0) content.push("");
			return { content, usage: estimateUsage(context, content) };
		},
	});
}
