// Sidecar-owned child runs for the daemon-admitted `delegate` and `create_task` tools.
//
// Children are plain agents owned by the session: their tickets belong to the parent
// session, a parent `delegate` call stays deferred until its child is terminal, and
// both conversations live in the same run-end snapshot (ADR-020). Protocol §10 fixes
// the cancel, steer and recovery rules implemented here.
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import { SUBAGENT_INSTRUCTIONS } from "../features/browse-policy.ts";
import type { DeferredCall, JsonObject, JsonValue } from "../protocol/types.ts";
import { MAX_USER_TEXT_BYTES } from "../protocol/types.ts";
import { SteerSignal } from "../scripted.ts";
import { userText } from "./content.ts";
import { DelegateDeferred, resumeRun } from "./deferred.ts";
import { forkHistory } from "./fork.ts";
import { PRESENT_INSTRUCTIONS, type RunEnd, type SessionAgent } from "./hooks.ts";
import type { SubagentHost } from "./session.ts";
import { type PendingCall, pendingWire, type SubagentState, type SubagentStatus, TERMINAL } from "./state.ts";
import type { Continuation } from "./tools.ts";

export const PROGRESS_TEXT_LIMIT = 500;

/** The daemon's `delegate` / `create_task` tool result: the admitted child configuration. */
export interface Admission {
	subagent_id: string;
	model: string;
	tools: string[];
	context: "fresh" | "fork";
}

export function parseAdmission(value: unknown): Admission {
	if (typeof value !== "object" || value === null || Array.isArray(value)) {
		throw new TypeError("Subagent admission must be an object");
	}
	const record = value as Record<string, unknown>;
	const { subagent_id, model, tools } = record;
	const context = record.context ?? "fresh";
	if (
		typeof subagent_id !== "string" ||
		typeof model !== "string" ||
		!Array.isArray(tools) ||
		!tools.every(tool => typeof tool === "string") ||
		(context !== "fresh" && context !== "fork")
	) {
		throw new TypeError("Invalid subagent admission");
	}
	return { subagent_id, model, tools, context };
}

export class Child {
	readonly state: SubagentState;
	pending: Map<string, PendingCall>;
	agent: SessionAgent | undefined;
	readonly cursor: { index: number };
	task: Promise<void> | undefined;
	// Cleared before a run's terminal callbacks, so a parent finishing at the same
	// moment never mistakes a stopping child for active work.
	active = false;
	cancelRequested = false;
	readonly steerReceived = new SteerSignal();
	// Set while the delegating parent tool call awaits this child's first stop.
	waiter: PromiseWithResolvers<SubagentStatus> | undefined;

	constructor(state: SubagentState) {
		this.state = state;
		this.pending = new Map(state.pending_deferred.map(item => [item.tool_call_id, item]));
		this.cursor = { index: state.script_cursor };
	}

	get id(): string {
		return this.state.id;
	}

	capture(): SubagentState {
		return structuredClone({
			...this.state,
			pending_deferred: [...this.pending.values()],
			script_cursor: this.cursor.index,
		});
	}

	requestCancel(): void {
		this.cancelRequested = true;
		if (this.agent?.running) this.agent.abort();
	}
}

function isTerminal(child: Child): boolean {
	return TERMINAL.has(child.state.status);
}

function newChildState(
	admission: Admission,
	parentToolCallId: string,
	task: string,
	messages: AgentMessage[],
	background: boolean,
): SubagentState {
	return {
		id: admission.subagent_id,
		parent_tool_call_id: parentToolCallId,
		task,
		model: admission.model,
		tools: admission.tools,
		status: "running",
		messages,
		pending_deferred: [],
		steer_buffer: [],
		script_cursor: 0,
		result: null,
		background,
		prompt_epoch: null,
	};
}

class ChildCancelled extends Error {
	override readonly name = "ChildCancelled";
}

export class Subagents {
	readonly children = new Map<string, Child>();

	constructor(private readonly host: SubagentHost) {}

	/** Rebuilds children from a snapshot; call after `host.pendingDelegates` is restored. */
	restore(states: SubagentState[]): void {
		this.children.clear();
		for (const state of states) {
			const child = new Child(state);
			this.children.set(child.id, child);
			// Saved resolutions need the daemon's durable delivery check before reuse.
			for (const pending of child.pending.values()) pending.resolution = null;
			const corrupt = [child.id, child.state.model, ...child.state.tools].some(value => value.includes("[REDACTED]"));
			if (corrupt || (!isTerminal(child) && child.pending.size === 0)) {
				// Nothing can wake a child without tickets; failing it keeps its
				// parent's delegate call from waiting forever.
				child.state.status = "failed";
				child.pending = new Map();
				child.state.result = {
					status: "failed",
					subagent_id: child.id,
					error_code: corrupt ? "corrupt_child_identity" : "interrupted",
					message: corrupt
						? "Stored child routing identity was redacted; it cannot be recovered"
						: "Subagent run was interrupted",
				};
			}
		}
		for (const item of this.host.pendingDelegates.values()) {
			if (item.result == null) item.result = this.childResult(item.subagent_id);
		}
	}

	/** Reports children failed for a redacted identity after `ready`; true when a snapshot is due. */
	reportRepaired(): boolean {
		let repaired = false;
		for (const child of this.children.values()) {
			const result = child.state.result;
			if (typeof result !== "object" || result === null || Array.isArray(result)) continue;
			if (result.error_code !== "corrupt_child_identity") continue;
			repaired = true;
			this.host.emit({
				kind: "subagent.lifecycle",
				subagent_id: child.id,
				status: "failed",
				error_code: "corrupt_child_identity",
				diagnostic: String(result.message),
			});
		}
		return repaired;
	}

	capture(): SubagentState[] {
		return [...this.children.values()].map(child => child.capture());
	}

	/** Waiting children's deferred calls, as the daemon sees them next to the parent's. */
	deferredWire(): DeferredCall[] {
		const calls: DeferredCall[] = [];
		for (const child of this.children.values()) {
			if (isTerminal(child)) continue;
			for (const item of child.pending.values()) calls.push(pendingWire(item));
		}
		return calls;
	}

	findPending(toolCallId: string, ticketId: string): { child: Child; pending: PendingCall } | undefined {
		for (const child of this.children.values()) {
			const pending = child.pending.get(toolCallId);
			if (pending !== undefined && pending.ticket_id === ticketId && !isTerminal(child)) {
				return { child, pending };
			}
		}
		return undefined;
	}

	knownSubagents(): string[] {
		return [...this.children.values()].filter(child => !isTerminal(child)).map(child => child.id);
	}

	/** Foreground children only: a background Task never keeps the session busy. */
	foregroundActive(): boolean {
		for (const child of this.children.values()) {
			if (child.active && !child.state.background) return true;
		}
		return false;
	}

	anyActive(): boolean {
		for (const child of this.children.values()) if (child.active) return true;
		return false;
	}

	childResult(subagentId: string): JsonValue | null {
		const child = this.children.get(subagentId);
		if (child === undefined) {
			return { status: "failed", subagent_id: subagentId, message: "Subagent state was lost" };
		}
		return isTerminal(child) ? (child.state.result ?? null) : null;
	}

	/** Runs the child the daemon admitted; its deferral defers this parent call. */
	readonly delegate: Continuation = async (ctx, args, admitted) => {
		const admission = parseAdmission(admitted);
		const task = String(args.task ?? "");
		const history = admission.context === "fork" ? forkHistory(ctx.parentMessages) : [];
		const child = new Child(newChildState(admission, ctx.toolCallId, task, history, false));
		this.children.set(child.id, child);
		const waiter = Promise.withResolvers<SubagentStatus>();
		child.waiter = waiter;
		this.startChild(child, task, undefined);
		let status: SubagentStatus;
		try {
			status = await abortable(waiter.promise, ctx.signal);
		} catch (error) {
			// The parent run was cancelled; never leave its child running unowned.
			child.requestCancel();
			await child.task;
			throw error;
		} finally {
			child.waiter = undefined;
		}
		if (status === "waiting_human") throw new DelegateDeferred(child.id);
		return child.state.result ?? null;
	};

	/** Starts the admitted Task and returns at once; the front agent stays free. */
	readonly createTask: Continuation = async (ctx, args, admitted) => {
		const admission = parseAdmission(admitted);
		const goal = String(args.goal ?? "");
		const child = new Child(newChildState(admission, ctx.toolCallId, goal, [], true));
		this.children.set(child.id, child);
		this.startChild(child, goal, undefined);
		return { task_id: child.id, status: "running" };
	};

	readonly steerTask: Continuation = async (_ctx, args): Promise<JsonValue> => {
		const taskId = String(args.task_id ?? "");
		const delivered = await this.steer({ subagent_id: taskId, text: String(args.message ?? "") });
		if (!delivered) return { status: "denied", reason: "task_finished" };
		return { task_id: taskId, status: "steered" };
	};

	readonly cancelTask: Continuation = async (_ctx, args, admitted) => {
		// The daemon already recorded the cancellation; stop the child here.
		await this.cancel(String(args.task_id ?? ""));
		return admitted;
	};

	/** The continuation map the session's tool proxy consults for daemon-admitted calls. */
	continuations(): Record<string, Continuation> {
		return {
			delegate: this.delegate,
			create_task: this.createTask,
			steer_task: this.steerTask,
			cancel_task: this.cancelTask,
		};
	}

	childAgent(child: Child): SessionAgent {
		if (child.agent !== undefined) return child.agent;
		const host = this.host;
		const allowed = new Set(child.state.tools);
		// Output references remain host-scoped; this does not grant filesystem reads.
		allowed.add("read");
		const model = host.resolveChildModel(child.state.model, {
			subagent: child.id,
			scriptKey: child.state.parent_tool_call_id,
			cursor: child.cursor,
			steer: child.steerReceived,
		});
		const policies = [SUBAGENT_INSTRUCTIONS];
		if (allowed.has("present")) policies.push(PRESENT_INSTRUCTIONS);
		const agent = host.buildAgent({
			systemPrompt: host.spec.system_prompt_parts,
			tools: host.spec.tools.filter(tool => allowed.has(tool.name)),
			model,
			subagent: child.id,
			initial: child.state.messages,
			policy: policies.join("\n\n"),
			promptEpoch: child.state.prompt_epoch ?? null,
			recallQuery: child.state.task,
			onPromptEpoch: epoch => {
				child.state.prompt_epoch = epoch;
			},
			steer: () => this.drainSteer(child),
			steerSignal: child.steerReceived,
		});
		agent.subscribe(event => {
			if (event.type !== "message_end" || event.message.role !== "assistant") return;
			const message = event.message;
			// Like the former after-model-request hook, only answered requests report.
			if (message.stopReason === "aborted" || message.stopReason === "error") return;
			const usage = message.usage;
			host.emit({
				kind: "usage",
				model: message.model || child.state.model,
				input_tokens: usage.input + usage.cacheRead + usage.cacheWrite,
				output_tokens: usage.output,
			});
			const progress: JsonObject = {
				kind: "model_response",
				tool_calls: message.content.filter(part => part.type === "toolCall").map(part => part.name),
			};
			const text = message.content
				.filter(part => part.type === "text")
				.map(part => part.text)
				.join("");
			if (text) progress.text = [...text].slice(0, PROGRESS_TEXT_LIMIT).join("");
			host.emit({ kind: "subagent.progress", subagent_id: child.id, progress });
		});
		child.agent = agent;
		return agent;
	}

	/** Steer text queued for the child, injected at its next model request boundary. */
	private drainSteer(child: Child): AgentMessage[] {
		const injected: AgentMessage[] = [];
		while (child.state.steer_buffer.length > 0) {
			const text = child.state.steer_buffer.shift() as string;
			injected.push(userText(text));
			this.host.emit({ kind: "subagent.progress", subagent_id: child.id, progress: { kind: "steer_injected" } });
		}
		this.host.emit({ kind: "subagent.progress", subagent_id: child.id, progress: { kind: "model_request" } });
		return injected;
	}

	startChild(child: Child, prompt: string | undefined, resume: PendingCall[] | undefined): void {
		child.active = true;
		child.cancelRequested = false;
		// Start on a later microtask, like a scheduled task: callers finish their own
		// bookkeeping (ticket resolution, waiter setup) before the child emits anything.
		child.task = Promise.resolve().then(() => this.driveChild(child, prompt, resume));
	}

	/** Resumes a waiting child once every one of its tickets has a delivered resolution. */
	resumeIfReady(child: Child): boolean {
		if (this.host.shuttingDown || this.host.fatal || child.active || child.pending.size === 0) return false;
		for (const item of child.pending.values()) if (item.resolution == null) return false;
		// child.pending stays until the run ends: a snapshot taken meanwhile must still
		// show this child waiting on its (now delivered) tickets.
		this.startChild(child, undefined, [...child.pending.values()]);
		return true;
	}

	async driveChild(child: Child, prompt: string | undefined, resume: PendingCall[] | undefined): Promise<void> {
		let status: SubagentStatus = "failed";
		let output = "";
		let diagnostic: string | undefined;
		let messages = child.state.messages;
		let pending: PendingCall[] = [];
		try {
			this.host.emit({ kind: "subagent.lifecycle", subagent_id: child.id, status: "running" });
			const agent = this.childAgent(child);
			let next = prompt;
			let resuming = resume;
			while (true) {
				if (child.cancelRequested) throw new ChildCancelled();
				let end: RunEnd;
				if (resuming !== undefined) {
					end = await resumeRun(agent, resuming);
					resuming = undefined;
				} else {
					end = await agent.prompt([userText(next ?? "")]);
				}
				messages = agent.messages;
				if (child.cancelRequested || end.outcome === "aborted") throw new ChildCancelled();
				if (end.error !== undefined) throw end.error;
				if (end.outcome === "deferred" && end.pending.length > 0) {
					pending = end.pending;
					status = "waiting_human";
					break;
				}
				// A steer that arrived during the final model request still reaches the
				// child instead of being dropped with the finished run.
				if (child.state.steer_buffer.length > 0) {
					next = child.state.steer_buffer.shift();
					continue;
				}
				status = "completed";
				output = end.finalText;
				break;
			}
		} catch (error) {
			if (error instanceof ChildCancelled) {
				if (this.host.shuttingDown && child.state.status === "waiting_human") {
					// Shutdown keeps the last consistent state; the restored child waits
					// for the daemon to redeliver or interrupt its tickets.
					child.task = undefined;
					child.active = false;
					return;
				}
				if (child.agent !== undefined) messages = child.agent.messages;
				status = "cancelled";
			} else {
				status = "failed";
				diagnostic = await this.redactedDiagnostic(child, error);
			}
		} finally {
			child.task = undefined;
			child.active = false;
		}
		child.state.messages = messages;
		child.pending = new Map(pending.map(item => [item.tool_call_id, item]));
		child.state.status = status;
		if (status === "completed") {
			child.state.result = { status: "completed", subagent_id: child.id, output };
		} else if (status === "failed") {
			child.state.result = {
				status: "failed",
				subagent_id: child.id,
				error_code: "child_run_failed",
				message: diagnostic ?? null,
			};
		} else if (status === "cancelled") {
			child.state.result = { status: "cancelled", subagent_id: child.id };
		}
		this.host.emit({
			kind: "subagent.lifecycle",
			subagent_id: child.id,
			status,
			...(status === "completed" ? { output } : {}),
			...(status === "failed" ? { error_code: "child_run_failed" } : {}),
			...(diagnostic !== undefined ? { diagnostic } : {}),
		});
		await this.childStopped(child);
	}

	/** The daemon owns the complete secret registry, including browser cookies. */
	private async redactedDiagnostic(child: Child, error: unknown): Promise<string> {
		const name = error instanceof Error ? error.name : "Error";
		const message = error instanceof Error ? error.message : String(error);
		let diagnostic = `${name}: exception details unavailable`;
		try {
			const safe = await this.host.rpc.toolCall(
				{
					name: "__redact_diagnostic",
					arguments: { diagnostic: `${name}: ${message}` },
					toolCallId: crypto.randomUUID().replaceAll("-", ""),
					subagent: child.id,
				},
				new AbortController().signal,
			);
			if (safe.status === "done" && typeof safe.for_model === "string") diagnostic = safe.for_model;
		} catch {
			// A diagnostic transport failure must not replace the primary failure.
		}
		return diagnostic;
	}

	async childStopped(child: Child): Promise<void> {
		const host = this.host;
		if (child.waiter !== undefined) {
			const waiter = child.waiter;
			child.waiter = undefined;
			waiter.resolve(child.state.status);
			return;
		}
		if (host.shuttingDown || host.fatal) return;
		if (isTerminal(child)) {
			for (const item of host.pendingDelegates.values()) {
				if (item.subagent_id === child.id) item.result = child.state.result ?? null;
			}
		} else if (child.pending.size > 0) {
			host.rpc.send({ type: "deferred", calls: [...child.pending.values()].map(pendingWire) });
		}
		if (host.parentRunning) return;
		// Persist the child's outcome before the parent consumes it, so a crash during
		// the parent's resumption never replays the child's run.
		await host.snapshot();
		if (host.resumeIfReady()) return;
		if (host.startNext({ allowSteer: true })) return;
		if (!this.foregroundActive()) host.rpc.send({ type: "settled" });
	}

	async cancel(subagentId: string): Promise<void> {
		const child = this.children.get(subagentId);
		if (child === undefined || isTerminal(child)) return;
		if (child.active) {
			child.requestCancel();
			return;
		}
		child.pending.clear();
		child.state.status = "cancelled";
		child.state.result = { status: "cancelled", subagent_id: child.id };
		this.host.emit({ kind: "subagent.lifecycle", subagent_id: child.id, status: "cancelled" });
		await this.childStopped(child);
	}

	async steer(message: { subagent_id: string; text: string }): Promise<boolean> {
		if (new TextEncoder().encode(message.text).byteLength > MAX_USER_TEXT_BYTES) {
			this.host.error("bad_request", "Message text exceeds 1 MiB");
			return false;
		}
		const child = this.children.get(message.subagent_id);
		if (child === undefined || isTerminal(child)) return false;
		child.state.steer_buffer.push(message.text);
		child.steerReceived.set();
		this.host.emit({ kind: "subagent.progress", subagent_id: child.id, progress: { kind: "steer_queued" } });
		if (!child.active && !this.host.parentRunning) await this.host.snapshot();
		return true;
	}

	/** The child half of `discard_deferred`: every waiting child ends cancelled. */
	discardAll(): void {
		for (const child of this.children.values()) {
			if (isTerminal(child)) continue;
			child.pending.clear();
			child.state.status = "cancelled";
			child.state.result = { status: "cancelled", subagent_id: child.id };
		}
	}

	async stopAll(): Promise<void> {
		const tasks: Promise<void>[] = [];
		for (const child of this.children.values()) {
			if (!child.active) continue;
			if (child.task !== undefined) tasks.push(child.task);
			child.requestCancel();
		}
		await Promise.all(tasks);
	}
}

function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
	if (signal.aborted) return Promise.reject(signal.reason);
	const { promise: settled, resolve, reject } = Promise.withResolvers<T>();
	const onAbort = () => reject(signal.reason);
	signal.addEventListener("abort", onAbort, { once: true });
	promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", onAbort));
	return settled;
}
