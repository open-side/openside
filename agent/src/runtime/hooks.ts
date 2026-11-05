// One agent's runs over pi-agent-core's loop: the parent and every child use it.
//
// The pi `Agent` class is not used: its steering queue cannot be gated on pending
// deferrals and its transcript only grows through events, while openside needs
// per-request injection (steer, batch, skills, compaction) persisted in history and
// a run that stops, unchanged, while daemon tickets are open.
import {
	type AgentContext,
	type AgentEvent,
	type AgentLoopConfig,
	type AgentMessage,
	type AgentTool,
	type AgentToolResult,
	agentLoop,
	agentLoopContinue,
	defaultConvertToLlm,
	type StreamFn,
} from "@oh-my-pi/pi-agent-core";
import type {
	AssistantMessage,
	AssistantMessageEvent,
	Effort,
	Model,
	ServiceTier,
	ToolResultMessage,
} from "@oh-my-pi/pi-ai";
import { defendResult } from "../capabilities/injection";
import type { GuardDecision } from "../features/research";
import type { JsonValue } from "../protocol/types";
import type { ReportingCompaction } from "./compaction";
import { syntheticUser } from "./content";
import { DEFERRED_PLACEHOLDER, type DeferredLedger } from "./deferred";
import type { PendingCall, PendingDelegate } from "./state";
import { endToolStep, FRONT_RETRY, forModelText, type ToolRun, unknownTool } from "./tools";

export const PRESENT_INSTRUCTIONS =
	"[PRESENT]\n" +
	"When present is available, use it for bounded structured results: product or " +
	"place lists, comparison tables, order/payment summaries, noninteractive form " +
	"summaries, and connection intents. Follow its daemon-supplied schema; cards " +
	"are presentation, never permission. Use id='', status='done', " +
	"interaction='none', no ticket_id, and no favicon_ref. Keep strings plain text. " +
	"Use at most 16 blocks, 8 actions, 30 list items, 12 table columns, 100 rows " +
	"and 600 cells, 8 image sources, 24 KiB of string data and 64 KiB of JSON. " +
	"Actions may only draft a follow_up or open an HTTP(S) link, with at most " +
	"one primary action. The daemon returns only card_id; the person sees the card. " +
	"Do not reproduce the complete card as another answer or Markdown table. " +
	"Give safe context in a short bubble, for example 'Here are the options; " +
	"delivery dates may change.'\n" +
	'Example comparison: present({"card":{"id":"","kind":"table",' +
	'"site":{"origin":null,"name":"Comparison","favicon_ref":null},' +
	'"title":"Options","status":"done","blocks":[{"type":"table",' +
	'"id":"options","columns":[{"id":"name","label":"Name"}],' +
	'"rows":[[{"type":"text","text":"Option A"}]],"layout":"grid"}],' +
	'"actions":[],"interaction":"none","collapsed_summary":"One option"},' +
	"\"images\":[]}). Then say 'This is the option I found,' not the table again.\n" +
	"Example order: use kind='order_summary' with kv_rows and typed money amounts " +
	"in integer minor_units, then say 'Review the total; no order has been placed.' " +
	"Presenting a payment summary never authorizes a purchase.\n" +
	"Ask for real login, OTP or page-form input only through request_login, " +
	"request_otp or request_form; use the daemon's human-step tools for approval " +
	"or takeover. Never put credentials, passwords, codes, authenticator keys " +
	"or payment-field values in present, prose, tool arguments or messages. " +
	"Secure values go directly to the daemon's verified page; outcomes contain " +
	"status only. If the daemon rejects a draft, correct it using the safe rule " +
	"in the retry, never fabricate ticket authority or bypass the rejection.";

/** The model saw tools other than the ones the daemon declared for this agent. */
export class ToolGateError extends Error {
	override readonly name = "ToolGateError";
	constructor() {
		super("Exposed tools differ from daemon tools");
	}
}

/** The provider ended a response with an error (after its own retries and fallbacks). */
export class ModelRunError extends Error {
	override readonly name = "ModelRunError";
}

export type RunOutcome = "completed" | "aborted" | "deferred" | "failed";

export interface RunEnd {
	outcome: RunOutcome;
	/** Set for `failed` runs; ToolGateError and AuthUnavailable keep their class. */
	error?: unknown;
	/** Final answer text; front mode: the send_message text, or "" for stay_silent. */
	finalText: string;
	pending: PendingCall[];
	delegates: PendingDelegate[];
}

export interface AgentOptions {
	/** Subagent id; undefined for the parent. */
	owner?: string;
	model: Model;
	streamFn: StreamFn;
	systemPrompt: string[];
	tools: AgentTool[];
	/** Exactly the tool names every request may expose. */
	gate: ReadonlySet<string>;
	ledger: DeferredLedger;
	retries: number;
	sessionId?: string;
	reasoning?: Effort;
	/** Requests no reasoning even from a reasoning model; wins over `reasoning`. */
	disableReasoning?: boolean;
	serviceTier?: ServiceTier;
	maxTokens?: number;
	/** Front mode: a run ends only through a send_message/stay_silent output. */
	front?: boolean;
	/**
	 * Called once per model request with the persisted history, before compaction;
	 * pushes are kept in history (steer, batch, skill loads).
	 */
	beforeRequest?: (messages: AgentMessage[], signal: AbortSignal | undefined) => Promise<void>;
	/** Called once a request is about to be sent, after compaction. */
	requestStarted?: (messages: readonly AgentMessage[]) => void;
	/** Request-only context; `request` is the 1-based model request of this run. */
	transform?: (messages: AgentMessage[], request: number) => AgentMessage[];
	compaction?: ReportingCompaction;
	onAssistantEvent?: (message: AssistantMessage, event: AssistantMessageEvent) => void;
	/** A completed response, before its tool calls run. */
	onResponse?: (message: AssistantMessage) => void;
	/** Refusal text for a tool call, or undefined to run it. */
	blockTool?: (name: string, messages: readonly AgentMessage[]) => string | undefined;
	/** Decides a final text answer; `retry` counts answers already sent back. */
	finalGuard?: (messages: readonly AgentMessage[], text: string, retry: number) => GuardDecision;
}

function toolCalls(
	message: AgentMessage | undefined,
): { id: string; name: string; arguments: Record<string, unknown> }[] {
	if (message?.role !== "assistant") return [];
	return message.content.flatMap(block => (block.type === "toolCall" ? [block] : []));
}

export function assistantText(message: AgentMessage | undefined): string {
	if (message?.role !== "assistant") return "";
	return message.content.flatMap(block => (block.type === "text" ? [block.text] : [])).join("");
}

function result(
	toolCallId: string,
	toolName: string,
	value: JsonValue | null | undefined,
	isError = false,
): ToolResultMessage {
	return {
		role: "toolResult",
		toolCallId,
		toolName,
		content: [{ type: "text", text: forModelText(value ?? null) }],
		details: value ?? null,
		isError,
		timestamp: Date.now(),
	};
}

export class SessionAgent {
	readonly options: AgentOptions;
	#messages: AgentMessage[];
	#controller: AbortController | undefined;
	#run: ToolRun | undefined;
	#stopNext = false;
	readonly #listeners = new Set<(event: AgentEvent) => void>();

	constructor(options: AgentOptions, initial: AgentMessage[] = []) {
		this.options = options;
		this.#messages = initial;
	}

	get model(): Model {
		return this.options.model;
	}

	get messages(): AgentMessage[] {
		return this.#messages;
	}

	set messages(messages: AgentMessage[]) {
		this.#messages = messages;
	}

	get running(): boolean {
		return this.#controller !== undefined;
	}

	/** Run state the daemon tool proxies report into; only valid while running. */
	get toolRun(): ToolRun {
		if (this.#run === undefined) throw new Error("Agent is not running");
		return this.#run;
	}

	subscribe(listener: (event: AgentEvent) => void): () => void {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	}

	abort(): void {
		this.#controller?.abort();
	}

	prompt(messages: AgentMessage[]): Promise<RunEnd> {
		return this.#execute(async () => messages);
	}

	continue(): Promise<RunEnd> {
		return this.#execute(async () => undefined);
	}

	/** Applies delivered resolutions, then continues; see `resumeRun`. */
	resume(
		pending: readonly PendingCall[],
		delegates: readonly { tool_call_id: string; result?: JsonValue | null }[] = [],
	): Promise<RunEnd> {
		return this.#execute(async signal => {
			const results: AgentMessage[] = [];
			for (const item of pending) {
				const call = this.#findCall(item.tool_call_id);
				if (call === undefined) continue;
				const resolution = item.resolution;
				if (resolution == null) continue;
				if (resolution.status === "result") {
					results.push(result(call.id, call.name, resolution.for_model));
				} else if (resolution.status === "denied") {
					results.push(result(call.id, call.name, resolution.reason, true));
				} else {
					const answer = await this.#executeApproved(call, item.ticket_id, signal);
					if (answer !== undefined) results.push(answer);
				}
			}
			for (const item of delegates) {
				const call = this.#findCall(item.tool_call_id);
				if (call !== undefined) results.push(result(call.id, call.name, item.result));
			}
			this.#messages = [...this.#messages, ...results];
			return undefined;
		});
	}

	#findCall(toolCallId: string): { id: string; name: string; arguments: Record<string, unknown> } | undefined {
		for (let index = this.#messages.length - 1; index >= 0; index--) {
			const call = toolCalls(this.#messages[index]).find(candidate => candidate.id === toolCallId);
			if (call !== undefined) return call;
		}
		return undefined;
	}

	async #executeApproved(
		call: { id: string; name: string; arguments: Record<string, unknown> },
		ticketId: string,
		signal: AbortSignal,
	): Promise<ToolResultMessage | undefined> {
		const tool = this.options.tools.find(candidate => candidate.name === call.name);
		if (tool === undefined) return result(call.id, call.name, `Tool ${call.name} is not available`, true);
		this.options.ledger.approve(call.id, ticketId);
		let output: AgentToolResult<unknown>;
		try {
			output = await tool.execute(call.id, call.arguments, signal);
		} catch (error) {
			if (signal.aborted) throw error;
			return result(call.id, call.name, error instanceof Error ? error.message : String(error), true);
		}
		if (output.details === DEFERRED_PLACEHOLDER) return undefined;
		return {
			role: "toolResult",
			toolCallId: call.id,
			toolName: call.name,
			content: output.content,
			details: output.details,
			isError: output.isError === true,
			timestamp: Date.now(),
		};
	}

	#newRun(): ToolRun {
		return {
			owner: this.options.owner,
			ledger: this.options.ledger,
			retries: this.options.retries,
			failures: new Map(),
			stepFailures: new Set(),
			stepSuccesses: new Set(),
			failure: undefined,
			terminal: false,
			output: undefined,
			delegates: [],
			messages: () => this.#live ?? this.#messages,
		};
	}

	#live: AgentMessage[] | undefined;

	#shouldStop(run: ToolRun): boolean {
		return (
			run.failure !== undefined ||
			run.terminal ||
			run.delegates.length > 0 ||
			this.options.ledger.pendingFor(this.options.owner).length > 0
		);
	}

	async #execute(prepare: (signal: AbortSignal) => Promise<AgentMessage[] | undefined>): Promise<RunEnd> {
		if (this.#controller !== undefined) throw new Error("Agent is already running");
		const controller = new AbortController();
		this.#controller = controller;
		const run = this.#newRun();
		this.#run = run;
		try {
			return await this.#drive(run, controller.signal, prepare);
		} finally {
			this.#controller = undefined;
			this.#run = undefined;
			this.#live = undefined;
			this.#stopNext = false;
		}
	}

	async #drive(
		run: ToolRun,
		signal: AbortSignal,
		prepare: (signal: AbortSignal) => Promise<AgentMessage[] | undefined>,
	): Promise<RunEnd> {
		const options = this.options;
		let error: unknown;
		let start: AgentMessage[] | undefined;
		try {
			start = await prepare(signal);
		} catch (cause) {
			error = cause;
		}
		let request = 0;
		let guardRetries = 0;
		let frontRetries = 0;
		let finalText = "";
		let loop = error === undefined && !signal.aborted && !this.#shouldStop(run);
		while (loop) {
			loop = false;
			const context: AgentContext = {
				systemPrompt: options.systemPrompt,
				messages: [...this.#messages],
				tools: options.tools,
			};
			const tracked = [...this.#messages, ...(start ?? [])];
			this.#live = undefined;
			const config: AgentLoopConfig = {
				model: options.model,
				convertToLlm: defaultConvertToLlm,
				...(options.sessionId !== undefined ? { sessionId: options.sessionId } : {}),
				...(options.reasoning !== undefined ? { reasoning: options.reasoning } : {}),
				...(options.disableReasoning === true ? { disableReasoning: true } : {}),
				...(options.serviceTier !== undefined ? { serviceTier: options.serviceTier } : {}),
				...(options.maxTokens !== undefined ? { maxTokens: options.maxTokens } : {}),
				syncContextBeforeModelCall: async (live, liveSignal) => {
					this.#live = live.messages;
					endToolStep(run);
					this.#stopNext = liveSignal?.aborted === true || this.#shouldStop(run);
					if (this.#stopNext) return;
					const names = new Set((live.tools ?? []).map(tool => tool.name));
					if (names.size !== options.gate.size || [...names].some(name => !options.gate.has(name))) {
						throw new ToolGateError();
					}
					request++;
					await options.beforeRequest?.(live.messages, liveSignal);
					if (options.compaction?.needed(live.messages, options.systemPrompt)) {
						const compacted = await options.compaction.compact(live.messages);
						if (compacted !== live.messages) live.messages.splice(0, live.messages.length, ...compacted);
					}
					options.requestStarted?.(live.messages);
				},
				beforeModelCall: () => (this.#stopNext ? { stop: true } : undefined),
				// A stopping request is still prepared by the loop; it is never sent.
				transformContext: async messages =>
					this.#stopNext ? messages : (options.transform?.(messages, request) ?? messages),
				...(options.onAssistantEvent !== undefined ? { onAssistantMessageEvent: options.onAssistantEvent } : {}),
				...(options.onResponse !== undefined
					? {
							transformAssistantMessage: message => options.onResponse?.(message),
							transformAssistantMessagePreservesToolCalls: true,
						}
					: {}),
				resolveFallbackTool: (name, advertised) => unknownTool(run, name, advertised),
				beforeToolCall: ({ toolCall }) => {
					const reason = options.blockTool?.(toolCall.name, this.#live ?? tracked);
					return reason === undefined ? undefined : { block: true, reason };
				},
				afterToolCall: ({ result: output }) => defendResult(output),
			};
			const stream =
				start !== undefined
					? agentLoop(start, context, config, signal, options.streamFn)
					: agentLoopContinue(context, config, signal, options.streamFn);
			try {
				for await (const event of stream) {
					if (event.type === "message_end" && this.#live === undefined) tracked.push(event.message);
					for (const listener of this.#listeners) listener(event);
				}
			} catch (cause) {
				error = cause;
			}
			this.#messages = this.#live ?? tracked;
			start = undefined;
			if (error !== undefined || signal.aborted) break;
			const last = this.#messages.at(-1);
			if (last?.role === "assistant" && last.stopReason === "error") {
				error = new ModelRunError(last.errorMessage ?? "Model run failed");
				this.#messages = this.#messages.slice(0, -1);
				break;
			}
			if (this.#shouldStop(run)) break;
			const text = assistantText(last);
			if (options.front) {
				// Plain text is never a front output; the person sees only daemon-published messages.
				if (frontRetries >= options.retries) {
					error = new ModelRunError("Exceeded maximum retries for front output");
					break;
				}
				frontRetries++;
				this.#messages = [...this.#messages, syntheticUser(FRONT_RETRY)];
				loop = true;
				continue;
			}
			const decision = options.finalGuard?.(this.#messages, text, guardRetries) ?? { action: "allow" };
			if (decision.action === "retry") {
				guardRetries++;
				this.#messages = [...this.#messages, syntheticUser(decision.message)];
				loop = true;
				continue;
			}
			finalText = decision.action === "replace" ? decision.text : text;
		}
		return this.#finish(run, signal, error, finalText);
	}

	#finish(run: ToolRun, signal: AbortSignal, error: unknown, finalText: string): RunEnd {
		const pending = this.options.ledger.take(this.options.owner);
		const delegates = run.delegates.map(item => ({ ...item, result: null }));
		const open = new Set([...pending.map(item => item.tool_call_id), ...delegates.map(item => item.tool_call_id)]);
		if (signal.aborted) {
			// A cancelled run never resolves its tickets; answer them so history stays well-formed.
			this.#messages = this.#messages.map(message =>
				message.role === "toolResult" && open.has(message.toolCallId)
					? result(message.toolCallId, message.toolName, "Cancelled before this call was resolved", true)
					: message,
			);
			return { outcome: "aborted", finalText: "", pending: [], delegates: [] };
		}
		if (open.size > 0) {
			this.#messages = this.#messages.filter(
				message => !(message.role === "toolResult" && open.has(message.toolCallId)),
			);
		}
		if (error === undefined && run.failure !== undefined) error = new ModelRunError(run.failure);
		if (error !== undefined) return { outcome: "failed", error, finalText: "", pending: [], delegates: [] };
		if (open.size > 0) return { outcome: "deferred", finalText: "", pending, delegates };
		if (this.options.front) {
			const text = run.output?.text;
			return { outcome: "completed", finalText: typeof text === "string" ? text : "", pending: [], delegates: [] };
		}
		return { outcome: "completed", finalText, pending: [], delegates: [] };
	}
}
