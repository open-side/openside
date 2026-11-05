// Daemon-owned tool schemas and reverse RPC; no local execution tools.
import type { AgentMessage, AgentTool, AgentToolResult } from "@oh-my-pi/pi-agent-core";
import type { ImageContent, TextContent, TSchema } from "@oh-my-pi/pi-ai";
import type { Done, JsonObject, JsonValue, ToolOutcome, ToolSpec } from "../protocol/types";
import { userContent } from "./content";
import { DEFERRED_PLACEHOLDER, type DeferredLedger, DelegateDeferred } from "./deferred";

/** Runs after the daemon admitted a call (policy, audit) with its `for_model` value. */
export type Continuation = (
	ctx: { toolCallId: string; parentMessages: AgentMessage[]; signal: AbortSignal },
	args: JsonObject,
	admitted: JsonValue,
) => Promise<JsonValue>;

export type ToolInvoker = (
	args: { name: string; arguments: JsonObject; toolCallId: string; approvalTicket?: string },
	signal: AbortSignal,
) => Promise<ToolOutcome>;

export const FRONT_OUTPUTS: ReadonlySet<string> = new Set(["send_message", "stay_silent"]);
export const STAY_SILENT: ToolSpec = {
	name: "stay_silent",
	description: "End this turn without sending a message to the person.",
	parameters: { type: "object", properties: {}, additionalProperties: false },
	approval: "read",
	deferrable: false,
};
export const FRONT_RETRY = "End the turn with send_message or stay_silent.";
const FRONT_UNDELIVERED = "Message delivery did not complete; use a valid message output.";

/** Per-run state a proxy reports into; owned by the agent that runs the tools. */
export interface ToolRun {
	readonly owner: string | undefined;
	readonly ledger: DeferredLedger;
	readonly retries: number;
	/**
	 * Failed model steps per tool name in this run, as pydantic-ai 2.55 `ToolManager` counted
	 * them: parallel failures of one tool in a single response count once, and a count is
	 * cleared only when that tool succeeds (a step that does not call it keeps the count).
	 */
	failures: Map<string, number>;
	/** Tools that failed in the current step; folded into `failures` by `endToolStep`. */
	readonly stepFailures: Set<string>;
	/** Tools that succeeded in the current step; `endToolStep` clears their counts. */
	readonly stepSuccesses: Set<string>;
	/** Set when a tool exhausted its retries; the run fails before the next model request. */
	failure: string | undefined;
	/** Front mode: a terminal output (send_message/stay_silent) completed this run. */
	terminal: boolean;
	/** Front mode: the completed output's arguments (`{}` for stay_silent). */
	output: JsonObject | undefined;
	readonly delegates: { tool_call_id: string; subagent_id: string }[];
	messages(): AgentMessage[];
}

export function forModelText(value: JsonValue): string {
	return typeof value === "string" ? value : JSON.stringify(value);
}

export function doneResult(done: Done): AgentToolResult<JsonValue> {
	const content: (TextContent | ImageContent)[] = [{ type: "text", text: forModelText(done.for_model) }];
	// Binary results (e.g. screenshots) reach vision models next to the JSON value.
	content.push(...userContent(done.parts));
	return { content, details: done.for_model, isError: false };
}

function retry(run: ToolRun, name: string, text: string): AgentToolResult<JsonValue> {
	if ((run.failures.get(name) ?? 0) >= run.retries) {
		run.failure = `Tool ${name} exceeded max retries count of ${run.retries}`;
	}
	run.stepFailures.add(name);
	return { content: [{ type: "text", text }], details: null, isError: true };
}

/** Call before each model request: folds the finished step into the per-tool counts. */
export function endToolStep(run: ToolRun): void {
	const next = new Map<string, number>();
	for (const [name, count] of run.failures) if (!run.stepSuccesses.has(name)) next.set(name, count);
	for (const name of run.stepFailures) next.set(name, (run.failures.get(name) ?? 0) + 1);
	run.failures = next;
	run.stepFailures.clear();
	run.stepSuccesses.clear();
}

function succeeded(run: ToolRun, name: string, result: AgentToolResult<JsonValue>): AgentToolResult<JsonValue> {
	run.stepSuccesses.add(name);
	return result;
}

/**
 * Answers a call to a tool this request did not advertise. Like the former pydantic-ai
 * runtime, the model is told the tool is unknown and the miss counts against the retry
 * budget, so a model stuck on a missing tool fails the run instead of looping forever.
 */
export function unknownTool(run: ToolRun, name: string, advertised: readonly AgentTool[]): AgentTool {
	const available = advertised.map(tool => `'${tool.name}'`).join(", ");
	const text =
		available === ""
			? `Unknown tool name: '${name}'. No tools available.`
			: `Unknown tool name: '${name}'. Available tools: ${available}`;
	return {
		name,
		label: name,
		description: "",
		parameters: { type: "object", properties: {} } as unknown as TSchema,
		lenientArgValidation: true,
		intent: "omit",
		async execute() {
			return retry(run, name, text);
		},
	};
}

function placeholder(): AgentToolResult<JsonValue> {
	return {
		content: [{ type: "text", text: "Deferred until the daemon resolves this call." }],
		details: DEFERRED_PLACEHOLDER,
	};
}

function outcomeResult(
	run: ToolRun,
	name: string,
	toolCallId: string,
	outcome: ToolOutcome,
): AgentToolResult<JsonValue> | undefined {
	if (outcome.status === "deferred") {
		run.ledger.record(run.owner, { tool_call_id: toolCallId, ticket_id: outcome.ticket_id, kind: outcome.kind });
		return placeholder();
	}
	if (outcome.status === "denied") return retry(run, name, outcome.reason + (outcome.hint ? `\n${outcome.hint}` : ""));
	if (outcome.status === "error") return retry(run, name, outcome.message);
	return undefined;
}

export function proxyTool(
	spec: ToolSpec,
	invoke: ToolInvoker,
	run: () => ToolRun,
	continuation?: Continuation,
): AgentTool {
	return {
		name: spec.name,
		label: spec.name,
		description: spec.description,
		parameters: spec.parameters as unknown as TSchema,
		// The daemon validates its own canonical schema.
		lenientArgValidation: true,
		intent: "omit",
		async execute(toolCallId, params, signal) {
			const state = run();
			if (spec.name.startsWith("__")) return retry(state, spec.name, "Internal runtime operations are not model tools");
			const args = (params ?? {}) as JsonObject;
			const approvalTicket = state.ledger.takeApproval(toolCallId);
			const abort = signal ?? new AbortController().signal;
			const outcome = await invoke(
				{ name: spec.name, arguments: args, toolCallId, ...(approvalTicket ? { approvalTicket } : {}) },
				abort,
			);
			const failed = outcomeResult(state, spec.name, toolCallId, outcome);
			if (failed !== undefined) return failed;
			const done = outcome as Done;
			if (continuation === undefined) return succeeded(state, spec.name, doneResult(done));
			try {
				const value = await continuation(
					{ toolCallId, parentMessages: state.messages(), signal: abort },
					args,
					done.for_model,
				);
				return succeeded(state, spec.name, doneResult({ ...done, for_model: value }));
			} catch (error) {
				if (!(error instanceof DelegateDeferred)) throw error;
				state.delegates.push({ tool_call_id: toolCallId, subagent_id: error.subagentId });
				return placeholder();
			}
		},
	};
}

/**
 * Terminal front outputs: the daemon validates and publishes `send_message`; at most one
 * output completes a run, so later calls in the same batch never reach the daemon.
 */
export function frontOutputTool(spec: ToolSpec, invoke: ToolInvoker, run: () => ToolRun): AgentTool {
	const description =
		spec.name === "send_message" ? `${spec.description} Ends the turn; send at most one message.` : spec.description;
	return {
		name: spec.name,
		label: spec.name,
		description,
		parameters: spec.parameters as unknown as TSchema,
		lenientArgValidation: true,
		intent: "omit",
		concurrency: "exclusive",
		async execute(toolCallId, params, signal) {
			const state = run();
			const args = (params ?? {}) as JsonObject;
			if (state.terminal) {
				return { content: [{ type: "text", text: "The turn already ended." }], details: null, isError: true };
			}
			if (spec.name === "stay_silent") {
				if (Object.keys(args).length > 0) return retry(state, spec.name, FRONT_RETRY);
				state.terminal = true;
				state.output = {};
				return { content: [{ type: "text", text: "{}" }], details: {}, isError: false };
			}
			const outcome = await invoke(
				{ name: spec.name, arguments: args, toolCallId },
				signal ?? new AbortController().signal,
			);
			if (outcome.status === "done") {
				state.terminal = true;
				state.output = args;
				return { content: [{ type: "text", text: JSON.stringify(args) }], details: args, isError: false };
			}
			if (outcome.status === "denied") {
				return retry(state, spec.name, outcome.reason + (outcome.hint ? `; ${outcome.hint}` : ""));
			}
			if (outcome.status === "error") return retry(state, spec.name, outcome.message);
			return retry(state, spec.name, FRONT_UNDELIVERED);
		},
	};
}
