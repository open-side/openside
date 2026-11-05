import type {
	CredentialReply,
	CredentialResponse,
	Inbound,
	JsonObject,
	OutboundBody,
	OutcomeWire,
	ToolOutcome,
} from "../protocol/types";
import type { JsonlConnection } from "./jsonl";

/** The connection closed or the caller gave up before the daemon answered. */
export class RpcClosed extends Error {
	constructor(message = "Private daemon RPC unavailable") {
		super(message);
		this.name = "RpcClosed";
	}
}

export interface ToolCallArgs {
	name: string;
	arguments: unknown;
	toolCallId: string;
	subagent?: string;
	/** Ticket of an approval the daemon already granted for this call (deferred resume). */
	approvalTicket?: string;
}

export interface CredentialRequestArgs {
	provider: string;
	model: string;
	requestId: string;
	attempt: number;
	subagent?: string;
}

export interface CredentialReportArgs {
	lease: CredentialResponse;
	outcome: OutcomeWire;
	requestId: string;
	streamStarted: boolean;
}

/**
 * Reverse calls from the sidecar to the daemon (plan §9.1). Replies are correlated by the
 * id this side generates; validating reply kind and `request_id` stays with the caller.
 */
export interface ReverseRpc {
	toolCall(args: ToolCallArgs, signal: AbortSignal): Promise<ToolOutcome>;
	credentialRequest(args: CredentialRequestArgs, signal: AbortSignal): Promise<CredentialReply>;
	/** `await: false` sends without waiting (cancelled requests must not block on the daemon). */
	credentialReport(args: CredentialReportArgs, opts?: { await: boolean }): Promise<CredentialReply | undefined>;
	/** Ordered fire-and-forget outbound message (event, deferred, run_end, snapshot, …). */
	send(message: OutboundBody): void;
}

// Late replies to calls the sidecar stopped waiting for are expected, not protocol errors;
// remember this many ids so they are dropped silently.
const ABANDONED_RPC_LIMIT = 1024;

interface Waiter<T> {
	resolve: (value: T) => void;
	reject: (error: Error) => void;
}

class Abandoned {
	#ids = new Set<string>();

	add(id: string): void {
		this.#ids.add(id);
		if (this.#ids.size > ABANDONED_RPC_LIMIT) {
			const oldest = this.#ids.values().next().value;
			if (oldest !== undefined) this.#ids.delete(oldest);
		}
	}

	/** True once for an abandoned id, then forgets it. */
	take(id: string): boolean {
		return this.#ids.delete(id);
	}
}

const newId = () => crypto.randomUUID().replaceAll("-", "");

export class RpcRouter implements ReverseRpc {
	readonly #connection: JsonlConnection;
	readonly #tools = new Map<string, Waiter<ToolOutcome>>();
	readonly #credentials = new Map<string, Waiter<CredentialReply>>();
	readonly #abandonedTools = new Abandoned();
	readonly #abandonedCredentials = new Abandoned();
	#closed = false;

	constructor(connection: JsonlConnection) {
		this.#connection = connection;
	}

	send(message: OutboundBody): void {
		// Write failures surface as a closed connection on the receive side.
		this.#connection.send(message).catch(() => undefined);
	}

	toolCall(args: ToolCallArgs, signal: AbortSignal): Promise<ToolOutcome> {
		const rpcId = newId();
		return this.#call(this.#tools, this.#abandonedTools, rpcId, signal, () =>
			this.#connection.send({
				type: "tool_call",
				rpc_id: rpcId,
				tool_call_id: args.toolCallId,
				name: args.name,
				args: args.arguments as JsonObject,
				subagent: args.subagent,
				approval_ticket: args.approvalTicket,
			}),
		);
	}

	credentialRequest(args: CredentialRequestArgs, signal: AbortSignal): Promise<CredentialReply> {
		const id = crypto.randomUUID();
		return this.#call(this.#credentials, this.#abandonedCredentials, id, signal, () =>
			this.#connection.send({
				type: "credential_request",
				id,
				request_id: args.requestId,
				provider: args.provider,
				model: args.model,
				attempt: args.attempt,
				subagent: args.subagent,
			}),
		);
	}

	async credentialReport(
		args: CredentialReportArgs,
		opts: { await: boolean } = { await: true },
	): Promise<CredentialReply | undefined> {
		const id = crypto.randomUUID();
		const send = () =>
			this.#connection.send({
				type: "credential_report",
				id,
				request_id: args.requestId,
				lease_id: args.lease.lease_id,
				outcome: args.outcome.outcome,
				stream_started: args.streamStarted,
				retry_after_ms: args.outcome.retry_after_ms,
				scope: args.outcome.scope,
				usage: args.outcome.usage,
				usage_windows: args.outcome.usage_windows ?? [],
			});
		if (!opts.await) {
			if (this.#closed) throw new RpcClosed();
			this.#abandonedCredentials.add(id);
			await send().catch(() => {
				throw new RpcClosed();
			});
			return undefined;
		}
		return this.#call(this.#credentials, this.#abandonedCredentials, id, undefined, send);
	}

	/**
	 * Route a reply to its waiting call. Returns true when `message` was an RPC reply (consumed
	 * here, including late replies to abandoned calls); unknown reply ids get a non-fatal
	 * `bad_request` error as the protocol requires.
	 */
	dispatch(message: Inbound): boolean {
		if (message.type === "tool_result") {
			const waiter = this.#tools.get(message.rpc_id);
			if (this.#abandonedTools.take(message.rpc_id)) return true;
			if (waiter === undefined) {
				this.send({ type: "error", code: "bad_request", message: "Unknown reverse RPC response", fatal: false });
				return true;
			}
			this.#tools.delete(message.rpc_id);
			waiter.resolve(message.outcome);
			return true;
		}
		if (
			message.type === "credential_response" ||
			message.type === "credential_error" ||
			message.type === "credential_reported"
		) {
			const waiter = this.#credentials.get(message.id);
			if (this.#abandonedCredentials.take(message.id)) {
				if (message.type === "credential_response") message.auth.value = "";
				return true;
			}
			if (waiter === undefined) {
				if (message.type === "credential_response") message.auth.value = "";
				this.send({ type: "error", code: "bad_request", message: "Unknown credential RPC response", fatal: false });
				return true;
			}
			this.#credentials.delete(message.id);
			waiter.resolve(message);
			return true;
		}
		return false;
	}

	/** Outstanding `tool_call` RPCs (tests wait for children to release theirs). */
	get pendingCount(): number {
		return this.#tools.size;
	}

	/** Fail every pending call; later calls fail immediately. */
	close(): void {
		this.#closed = true;
		for (const waiters of [this.#tools, this.#credentials] as Map<string, Waiter<unknown>>[]) {
			for (const waiter of waiters.values()) waiter.reject(new RpcClosed());
			waiters.clear();
		}
	}

	#call<T>(
		waiters: Map<string, Waiter<T>>,
		abandoned: Abandoned,
		id: string,
		signal: AbortSignal | undefined,
		send: () => Promise<void>,
	): Promise<T> {
		if (this.#closed) return Promise.reject(new RpcClosed());
		if (signal?.aborted) return Promise.reject(signal.reason ?? new RpcClosed());
		const { promise, resolve, reject } = Promise.withResolvers<T>();
		const onAbort = () => {
			if (waiters.delete(id)) {
				abandoned.add(id);
				reject(signal?.reason ?? new RpcClosed());
			}
		};
		signal?.addEventListener("abort", onAbort, { once: true });
		waiters.set(id, {
			resolve: value => {
				signal?.removeEventListener("abort", onAbort);
				resolve(value);
			},
			reject: error => {
				signal?.removeEventListener("abort", onAbort);
				reject(error);
			},
		});
		send().catch(() => {
			const waiter = waiters.get(id);
			if (waiter !== undefined) {
				waiters.delete(id);
				waiter.reject(new RpcClosed());
			}
		});
		return promise;
	}
}
