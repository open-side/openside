/**
 * Private daemon credential RPC. The connection, not a frame field, owns
 * session authority; this layer validates every reply and turns daemon refusals
 * into `AuthUnavailable` carrying only application-authored reasons.
 */
import type {
	CredentialErrorCode,
	CredentialReply,
	CredentialResponse,
	CredentialUsage,
	OutcomeWire,
} from "../protocol/types";
import type { ReverseRpc } from "../transport/rpc";
import { AuthUnavailable } from "./errors";
import type { ClassifiedOutcome } from "./outcomes";
import { CredentialAttempt } from "./request-auth";
import { CredentialSecret } from "./secret";

export const SAFE_ERRORS: Record<CredentialErrorCode, string> = {
	auth_unavailable: "No eligible account in the configured pool",
	account_blocked: "Configured account pool is currently blocked",
	account_not_in_pool: "Account is not in the effective configured pool",
	provider_unavailable: "Configured credential provider is unavailable",
	invalid_credential_request: "Daemon rejected the credential request",
};

const MAX_ATTEMPT = 63;
const MAX_SECRET_LENGTH = 16_384;
const AUTH_FIELDS: Record<string, true> = {
	kind: true,
	value: true,
	upstream_account_id: true,
	expires_at_ms: true,
};
const REDACTED = "CredentialLease(credentials=**********)";

export type CredentialRpcPort = Pick<ReverseRpc, "credentialRequest" | "credentialReport">;

export class CredentialLease {
	readonly leaseId: string;
	readonly accountId: string;
	readonly credential: CredentialAttempt;
	/** The daemon reply with its secret already cleared; `credentialReport` keys on it. */
	readonly wire: CredentialResponse;

	constructor(wire: CredentialResponse, credential: CredentialAttempt) {
		this.leaseId = wire.lease_id;
		this.accountId = wire.account.account_id;
		this.credential = credential;
		this.wire = wire;
	}

	toString(): string {
		return REDACTED;
	}

	toJSON(): string {
		return REDACTED;
	}

	[Symbol.for("nodejs.util.inspect.custom")](): string {
		return REDACTED;
	}
}

export interface ReportOptions {
	streamStarted?: boolean;
	usage?: CredentialUsage;
	/** Cancellation reports are fire-and-forget: the run must not wait on the daemon. */
	wait?: boolean;
}

export interface CredentialRpc {
	acquire(
		provider: string,
		model: string,
		requestId: string,
		attempt: number,
		subagent: string | undefined,
		signal: AbortSignal,
	): Promise<CredentialLease>;
	/** Resolves true only when the daemon authorizes replaying the request on a new lease. */
	report(
		lease: CredentialLease,
		outcome: ClassifiedOutcome,
		requestId: string,
		options?: ReportOptions,
	): Promise<boolean>;
}

function scrub(reply: CredentialReply | undefined): void {
	if (reply?.type !== "credential_response") return;
	reply.auth.value = "";
	delete reply.auth.upstream_account_id;
}

function validLease(reply: CredentialResponse): boolean {
	const auth = reply.auth;
	return (
		Object.keys(auth).every(key => Object.hasOwn(AUTH_FIELDS, key)) &&
		typeof auth.value === "string" &&
		auth.value.length >= 1 &&
		auth.value.length <= MAX_SECRET_LENGTH
	);
}

export class DaemonCredentialRpc implements CredentialRpc {
	readonly #port: CredentialRpcPort;

	constructor(port: CredentialRpcPort) {
		this.#port = port;
	}

	async #exchange(
		provider: string,
		requestId: string,
		send: () => Promise<CredentialReply | undefined>,
	): Promise<CredentialReply | undefined> {
		let reply: CredentialReply | undefined;
		try {
			reply = await send();
		} catch (error) {
			if (error instanceof AuthUnavailable) throw error;
			throw new AuthUnavailable("daemon", ["Private credential RPC unavailable"], {
				reason: "provider_unavailable",
			});
		}
		if (reply !== undefined && reply.request_id !== requestId) {
			scrub(reply);
			throw new AuthUnavailable("daemon", ["Private credential response correlation mismatch"], {
				reason: "invalid_credential_request",
			});
		}
		if (reply?.type === "credential_error") {
			throw new AuthUnavailable(provider, [SAFE_ERRORS[reply.code] ?? SAFE_ERRORS.invalid_credential_request], {
				reason: reply.code,
				retryAtMs: reply.retry_at_ms,
			});
		}
		return reply;
	}

	async acquire(
		provider: string,
		model: string,
		requestId: string,
		attempt: number,
		subagent: string | undefined,
		signal: AbortSignal,
	): Promise<CredentialLease> {
		if (!Number.isInteger(attempt) || attempt < 0 || attempt > MAX_ATTEMPT) {
			throw new AuthUnavailable(provider, ["daemon credential rotation attempt limit reached"], {
				reason: "attempt_limit",
			});
		}
		const reply = await this.#exchange(provider, requestId, () =>
			this.#port.credentialRequest(
				subagent === undefined
					? { provider, model, requestId, attempt }
					: { provider, model, requestId, attempt, subagent },
				signal,
			),
		);
		if (reply?.type !== "credential_response" || !validLease(reply)) {
			scrub(reply);
			throw new AuthUnavailable(provider, ["Invalid private credential response"], {
				reason: "invalid_credential_request",
			});
		}
		const secret = new CredentialSecret(reply.auth.value);
		const credential = new CredentialAttempt(
			provider,
			reply.account.source,
			reply.auth.kind,
			secret,
			reply.auth.upstream_account_id,
		);
		scrub(reply);
		return new CredentialLease(reply, credential);
	}

	async report(
		lease: CredentialLease,
		outcome: ClassifiedOutcome,
		requestId: string,
		options: ReportOptions = {},
	): Promise<boolean> {
		const streamStarted = options.streamStarted ?? false;
		const wait = options.wait ?? true;
		const wire: OutcomeWire = outcome.toWire();
		if (options.usage) wire.usage = options.usage;
		wire.usage_windows = [];
		const provider = lease.credential.provider;
		const reply = await this.#exchange("daemon", requestId, () =>
			this.#port.credentialReport({ lease: lease.wire, outcome: wire, requestId, streamStarted }, { await: wait }),
		);
		if (!wait) return false;
		if (reply?.type === "credential_response") scrub(reply);
		if (reply?.type !== "credential_reported" || reply.lease_id !== lease.leaseId) {
			throw new AuthUnavailable(provider, ["Invalid private credential acknowledgement"], {
				reason: "invalid_credential_request",
			});
		}
		if (reply.action === "stop" && ["auth", "rate_limit", "quota", "policy"].includes(outcome.kind)) {
			throw new AuthUnavailable(provider, ["No eligible account in the configured pool"], {
				reason: "auth_unavailable",
				retryAtMs: reply.retry_at_ms,
			});
		}
		return !streamStarted && (reply.action === "refresh" || reply.action === "rotate");
	}
}

let credentialRpcWrapper: ((rpc: CredentialRpc) => CredentialRpc) | undefined;

/**
 * Test hook for the e2e preload: wraps the credential RPC of every stream this
 * process builds afterwards (ledgers, read-only fakes). Only `main.ts` calls it,
 * and only with a nonce-bound preload.
 */
export function wrapCredentialRpcForTests(wrap: (rpc: CredentialRpc) => CredentialRpc): void {
	credentialRpcWrapper = wrap;
}

export function credentialRpcFor(port: CredentialRpcPort): CredentialRpc {
	const rpc = new DaemonCredentialRpc(port);
	return credentialRpcWrapper ? credentialRpcWrapper(rpc) : rpc;
}
