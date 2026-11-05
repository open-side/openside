// Deferred daemon tool calls. pi-agent-core has no deferral, so a deferred call returns a
// placeholder result, the run stops before its next model request, and the placeholder is
// stripped; resolving appends the real result and continues (docs plan §2.2).
import type { JsonValue } from "../protocol/types";
import type { RunEnd, SessionAgent } from "./hooks";
import type { PendingCall } from "./state";

/** Thrown by a `delegate` continuation whose admitted child stopped waiting on a person. */
export class DelegateDeferred extends Error {
	override readonly name = "DelegateDeferred";
	constructor(readonly subagentId: string) {
		super("Delegated subagent is waiting");
	}
}

/** Marks placeholder tool results that stand in for a deferred call during one run. */
export const DEFERRED_PLACEHOLDER = "openside.deferred";

/**
 * Deferred calls recorded during runs, keyed by owner (undefined = the parent), and the
 * approval tickets a resumed call must present to the daemon.
 */
export class DeferredLedger {
	readonly #pending = new Map<string | undefined, PendingCall[]>();
	readonly #approved = new Map<string, string>();

	record(owner: string | undefined, call: PendingCall): void {
		const calls = this.#pending.get(owner) ?? [];
		calls.push(call);
		this.#pending.set(owner, calls);
	}

	pendingFor(owner?: string): PendingCall[] {
		return this.#pending.get(owner) ?? [];
	}

	take(owner?: string): PendingCall[] {
		const calls = this.pendingFor(owner);
		this.#pending.delete(owner);
		return calls;
	}

	approve(toolCallId: string, ticketId: string): void {
		this.#approved.set(toolCallId, ticketId);
	}

	/** The approval ticket for a re-executed call; consumed by its one daemon call. */
	takeApproval(toolCallId: string): string | undefined {
		const ticket = this.#approved.get(toolCallId);
		this.#approved.delete(toolCallId);
		return ticket;
	}
}

/**
 * Applies delivered resolutions (and finished delegate results) to a stopped run and
 * continues it. Approved calls are executed again with their ticket; a call that defers
 * again ends this resume as deferred without a model request.
 */
export function resumeRun(
	agent: SessionAgent,
	pending: readonly PendingCall[],
	delegates: readonly { tool_call_id: string; result?: JsonValue | null }[] = [],
): Promise<RunEnd> {
	return agent.resume(pending, delegates);
}
