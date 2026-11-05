// Opaque run-end snapshots; credential policy remains entirely in the daemon.
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import {
	type AgentState,
	type DeferredCall,
	type DeferredKind,
	type JsonValue,
	LEGACY_STATE_FORMAT,
	MAX_LINE_BYTES,
	type Resolution,
	STATE_FORMAT,
	type UserMessage,
} from "../protocol/types";

export type SubagentStatus = "running" | "waiting_human" | "completed" | "failed" | "cancelled";
export const TERMINAL: ReadonlySet<string> = new Set(["completed", "failed", "cancelled"]);

export interface PendingCall {
	tool_call_id: string;
	ticket_id: string;
	kind: DeferredKind;
	resolution?: Resolution | null;
}

export function pendingWire(call: PendingCall): DeferredCall {
	return { tool_call_id: call.tool_call_id, ticket_id: call.ticket_id, kind: call.kind };
}

/**
 * A parent `delegate` call suspended until its child reaches a terminal state.
 *
 * Unlike ticket resolutions, a stored child result is sidecar-internal and may be
 * reused after restore: the daemon never delivers it.
 */
export interface PendingDelegate {
	tool_call_id: string;
	subagent_id: string;
	result?: JsonValue | null;
}

/** Child conversation persisted only at the child's own run-end points. */
export interface SubagentState {
	id: string;
	parent_tool_call_id: string;
	task: string;
	model: string;
	tools: string[];
	status: SubagentStatus;
	messages: AgentMessage[];
	pending_deferred: PendingCall[];
	steer_buffer: string[];
	script_cursor: number;
	result?: JsonValue | null;
	// A Task started by the front agent: nobody awaits it; its result reaches the
	// conversation as a daemon-queued `[task report]` message.
	background: boolean;
	prompt_epoch?: JsonValue | null;
}

export interface SnapshotData {
	messages: AgentMessage[];
	pending_deferred: PendingCall[];
	pending_delegates: PendingDelegate[];
	subagents: SubagentState[];
	steer_buffer: UserMessage[];
	// Messages whose run ended deferred; the resumed run reports their final outcome.
	deferred_messages: string[];
	script_cursor: number;
	title_attempted: boolean;
	prompt_epoch: JsonValue | null;
}

// Base64 bytes of inline media kept in one snapshot, across the parent history, every
// child history and queued steer messages. Snapshots travel in one wire line (4 MiB), so
// older media is replaced by a note, newest first kept. Observation screenshots have no
// artifact copy; attachments and delivered screenshots remain in daemon storage.
export const SNAPSHOT_MEDIA_BUDGET = 1024 * 1024;

export const SNAPSHOT_TOO_LARGE = "Snapshot exceeds 4 MiB; pending continuation cannot be checkpointed";

/** The retained continuation cannot fit a single protocol checkpoint. */
export class SnapshotTooLarge extends Error {
	override readonly name = "SnapshotTooLarge";
	constructor() {
		super(SNAPSHOT_TOO_LARGE);
	}
}

export function emptySnapshot(): SnapshotData {
	return {
		messages: [],
		pending_deferred: [],
		pending_delegates: [],
		subagents: [],
		steer_buffer: [],
		deferred_messages: [],
		script_cursor: 0,
		title_attempted: false,
		prompt_epoch: null,
	};
}

/** Keeps the newest images of one history within `budget` (in place); returns the budget left. */
export function shedMedia(history: AgentMessage[], budget: number): number {
	for (let m = history.length - 1; m >= 0; m--) {
		const message = history[m] as AgentMessage;
		if (message.role !== "user" && message.role !== "toolResult") continue;
		const content = message.content;
		if (typeof content === "string") continue;
		for (let index = content.length - 1; index >= 0; index--) {
			const block = content[index];
			if (block?.type !== "image") continue;
			const size = block.data.length;
			if (size <= budget) {
				budget -= size;
				continue;
			}
			content[index] = { type: "text", text: `[${block.mimeType} image: omitted from saved conversation state]` };
		}
	}
	return budget;
}

function shedSnapshot(data: SnapshotData, budget: number): void {
	// Queued steer messages are not delivered yet, so their media is kept first; a part
	// that does not fit keeps its artifact reference and reaches the model as a note.
	data.steer_buffer = data.steer_buffer.map(message => ({
		...message,
		parts: message.parts.map(part => {
			const size = part.data_base64?.length ?? 0;
			if (size <= budget) {
				budget -= size;
				return part;
			}
			const { data_base64: _dropped, ...rest } = part;
			return rest;
		}),
	}));
	budget = shedMedia(data.messages, budget);
	for (const child of data.subagents) budget = shedMedia(child.messages, budget);
}

/** The exact `state_snapshot` line the transport sends for this data, minus the newline. */
export function snapshotLineBytes(state: AgentState): number {
	return Buffer.byteLength(JSON.stringify({ v: 1, type: "state_snapshot", state }), "utf8");
}

function encode(data: SnapshotData): AgentState {
	return { format: STATE_FORMAT, data: JSON.stringify(data) };
}

function fits(data: SnapshotData): boolean {
	return snapshotLineBytes(encode(data)) <= MAX_LINE_BYTES;
}

function completedPrefixEnd(history: AgentMessage[], pendingIds: ReadonlySet<string>): number {
	if (history.length <= 1) return 0;
	let end = history.length;
	for (let index = 1; index < history.length; index++) {
		if (history[index]?.role === "user") {
			end = index;
			break;
		}
	}
	for (const message of history.slice(0, end)) {
		if (message.role !== "assistant") continue;
		if (message.content.some(block => block.type === "toolCall" && pendingIds.has(block.id))) return 0;
	}
	return end;
}

function dropOldestTurn(history: AgentMessage[], end: number): void {
	history.splice(0, end);
	// A result cannot outlive the call it answers.
	while (history[0]?.role === "toolResult") history.shift();
}

export interface CaptureInput {
	messages: readonly AgentMessage[];
	pending: readonly PendingCall[];
	steer: readonly UserMessage[];
	cursor: number;
	delegates: readonly PendingDelegate[];
	subagents: readonly SubagentState[];
	deferredMessages: readonly string[];
	titleAttempted?: boolean;
	promptEpoch?: JsonValue | null;
}

/** Builds a bounded snapshot; never mutates the live state it is given. */
export function captureSnapshot(input: CaptureInput): AgentState {
	const data: SnapshotData = structuredClone({
		messages: [...input.messages],
		pending_deferred: [...input.pending],
		pending_delegates: [...input.delegates],
		subagents: [...input.subagents],
		steer_buffer: [...input.steer],
		deferred_messages: [...input.deferredMessages],
		script_cursor: input.cursor,
		title_attempted: input.titleAttempted ?? false,
		prompt_epoch: input.promptEpoch ?? null,
	});
	shedSnapshot(data, SNAPSHOT_MEDIA_BUDGET);
	if (!fits(data)) shedSnapshot(data, 0);
	const histories: [AgentMessage[], Set<string>][] = [
		[
			data.messages,
			new Set([
				...data.pending_deferred.map(item => item.tool_call_id),
				...data.pending_delegates.map(item => item.tool_call_id),
			]),
		],
		...data.subagents.map(
			child =>
				[child.messages, new Set(child.pending_deferred.map(item => item.tool_call_id))] as [
					AgentMessage[],
					Set<string>,
				],
		),
	];
	// A pending call needs its original turn after restart. Only completed prefixes
	// are candidates, even when a protected history is the largest.
	while (!fits(data)) {
		let best: [AgentMessage[], number, number] | undefined;
		for (const [history, pendingIds] of histories) {
			const end = completedPrefixEnd(history, pendingIds);
			if (end <= 0) continue;
			const size = JSON.stringify(history).length;
			if (best === undefined || size > best[2]) best = [history, end, size];
		}
		if (best === undefined) break;
		dropOldestTurn(best[0], best[1]);
	}
	if (!fits(data)) throw new SnapshotTooLarge();
	return encode(data);
}

function record(value: unknown): Record<string, unknown> {
	if (typeof value !== "object" || value === null || Array.isArray(value)) throw new TypeError("Invalid snapshot");
	return value as Record<string, unknown>;
}

function list<T>(value: unknown, fallback: T[] = []): T[] {
	if (value === undefined) return fallback;
	if (!Array.isArray(value)) throw new TypeError("Invalid snapshot");
	return value as T[];
}

const SNAPSHOT_KEYS = new Set([
	"messages",
	"pending_deferred",
	"pending_delegates",
	"subagents",
	"steer_buffer",
	"deferred_messages",
	"script_cursor",
	"title_attempted",
	"prompt_epoch",
]);

/**
 * Parses a `resume_state`. A snapshot of the retired Python runtime cannot be replayed
 * into pi-ai messages; the session starts fresh while the daemon keeps its event log (D4).
 */
export function restoreSnapshot(state: AgentState): { data: SnapshotData; legacy: boolean } {
	if (state.format === LEGACY_STATE_FORMAT) return { data: emptySnapshot(), legacy: true };
	if (state.format !== STATE_FORMAT) throw new TypeError("Unknown snapshot format");
	const raw = record(JSON.parse(state.data));
	for (const key of Object.keys(raw)) {
		// `pins` came from a retired credential-affinity design; ignore it.
		if (key !== "pins" && !SNAPSHOT_KEYS.has(key)) throw new TypeError("Unknown snapshot field");
	}
	const cursor = raw.script_cursor ?? 0;
	if (typeof cursor !== "number" || !Number.isInteger(cursor) || cursor < 0) throw new TypeError("Invalid snapshot");
	const data: SnapshotData = {
		messages: list<AgentMessage>(raw.messages),
		pending_deferred: list<PendingCall>(raw.pending_deferred),
		pending_delegates: list<PendingDelegate>(raw.pending_delegates),
		subagents: list<SubagentState>(raw.subagents),
		steer_buffer: list<UserMessage>(raw.steer_buffer),
		deferred_messages: list<string>(raw.deferred_messages),
		script_cursor: cursor,
		title_attempted: raw.title_attempted === true,
		prompt_epoch: (raw.prompt_epoch ?? null) as JsonValue | null,
	};
	for (const item of data.pending_deferred) {
		const entry = record(item);
		if (typeof entry.tool_call_id !== "string" || typeof entry.ticket_id !== "string") {
			throw new TypeError("Invalid snapshot");
		}
		if (entry.kind !== "approval" && entry.kind !== "external") throw new TypeError("Invalid snapshot");
	}
	for (const child of data.subagents) {
		const entry = record(child);
		entry.messages ??= [];
		entry.pending_deferred ??= [];
		entry.steer_buffer ??= [];
		entry.script_cursor ??= 0;
		entry.background ??= false;
	}
	return { data, legacy: false };
}
