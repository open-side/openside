/**
 * Private daemon credential RPC. The connection, not a frame field, owns
 * session authority; this layer validates every reply and turns daemon refusals
 * into `AuthUnavailable` carrying only application-authored reasons.
 */
import type {
	CredentialErrorCode,
	CredentialErrorRules,
	CredentialReply,
	CredentialRequestMetadata,
	CredentialResponse,
	CredentialUsage,
	OutcomeWire,
} from "../protocol/types";
import type { ReverseRpc } from "../transport/rpc";
import { AuthUnavailable } from "./errors";
import type { ClassifiedOutcome } from "./outcomes";
import { AUTH_HEADERS, CredentialAttempt } from "./request-auth";
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
	base_url: true,
	headers: true,
	error_rules: true,
	request_metadata: true,
};
const MAX_BASE_URL_LENGTH = 512;
const MAX_LEASE_HEADERS = 8;
const LEASE_HEADER_NAME = /^x-[a-z0-9-]{1,62}$/;
const LEASE_HEADER_VALUE = /^[\x20-\x7e]{0,256}$/;
const PROJECT_ID = /^[A-Za-z0-9._:-]{1,256}$/;
const MAX_ERROR_RULES = 16;
const MAX_UPSTREAM_ID_LENGTH = 1024;
const AUTH_KINDS: Record<string, true> = { api_key: true, claude_code: true, codex_cli: true, bearer: true };
const RULE_OUTCOMES: Record<string, true> = {
	unsupported_client: true,
	sign_in: true,
	capacity: true,
	rate_limit: true,
};
const RULE_FIELDS: Record<string, true> = {
	status: true,
	hint: true,
	body_contains: true,
	outcome: true,
	message: true,
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
	delete reply.auth.request_metadata;
}

/** An absolute http(s) URL without credentials, query or fragment. */
function validBaseUrl(value: unknown): value is string {
	if (typeof value !== "string" || value.length > MAX_BASE_URL_LENGTH) return false;
	try {
		const url = new URL(value);
		return (
			(url.protocol === "http:" || url.protocol === "https:") &&
			url.hostname !== "" &&
			url.username === "" &&
			url.password === "" &&
			url.search === "" &&
			url.hash === "" &&
			!value.includes("?") &&
			!value.includes("#")
		);
	} catch {
		return false;
	}
}

function validHeaders(value: unknown): value is Record<string, string> {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
	const entries = Object.entries(value);
	return (
		entries.length <= MAX_LEASE_HEADERS &&
		entries.every(
			([name, text]) =>
				LEASE_HEADER_NAME.test(name) &&
				!(AUTH_HEADERS as readonly string[]).includes(name) &&
				typeof text === "string" &&
				LEASE_HEADER_VALUE.test(text),
		)
	);
}

function optionalText(value: unknown, max: number): boolean {
	return (
		value === undefined ||
		value === null ||
		(typeof value === "string" && /^[\x20-\x7e]+$/.test(value) && value.length <= max)
	);
}

function validErrorRules(value: unknown): value is CredentialErrorRules {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
	const { hint_header, rules, ...rest } = value as Record<string, unknown>;
	return (
		Object.keys(rest).length === 0 &&
		(hint_header === undefined ||
			hint_header === null ||
			(typeof hint_header === "string" && LEASE_HEADER_NAME.test(hint_header))) &&
		Array.isArray(rules) &&
		rules.length <= MAX_ERROR_RULES &&
		rules.every((rule: unknown) => {
			if (typeof rule !== "object" || rule === null || Array.isArray(rule)) return false;
			// Every key is checked against RULE_FIELDS below before any value is trusted.
			const fields: Record<string, unknown> = { ...rule };
			const { status, outcome } = fields;
			return (
				Object.keys(fields).every(key => Object.hasOwn(RULE_FIELDS, key)) &&
				typeof status === "number" &&
				Number.isInteger(status) &&
				status >= 400 &&
				status <= 599 &&
				typeof outcome === "string" &&
				Object.hasOwn(RULE_OUTCOMES, outcome) &&
				optionalText(fields.hint, 64) &&
				optionalText(fields.body_contains, 128) &&
				optionalText(fields.message, 200)
			);
		})
	);
}

function validRequestMetadata(value: unknown): value is CredentialRequestMetadata {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
	const { project_id, ...rest } = value as Record<string, unknown>;
	return (
		Object.keys(rest).length === 0 &&
		(project_id === undefined || (typeof project_id === "string" && PROJECT_ID.test(project_id)))
	);
}

function validLease(reply: CredentialResponse): boolean {
	const auth = reply.auth;
	const upstream: unknown = auth.upstream_account_id;
	const expires: unknown = auth.expires_at_ms;
	return (
		Object.keys(auth).every(key => Object.hasOwn(AUTH_FIELDS, key)) &&
		typeof auth.kind === "string" &&
		Object.hasOwn(AUTH_KINDS, auth.kind) &&
		typeof auth.value === "string" &&
		auth.value.length >= 1 &&
		auth.value.length <= MAX_SECRET_LENGTH &&
		(upstream === undefined ||
			upstream === null ||
			(typeof upstream === "string" && upstream.length >= 1 && upstream.length <= MAX_UPSTREAM_ID_LENGTH)) &&
		(expires === undefined ||
			expires === null ||
			(typeof expires === "number" && Number.isSafeInteger(expires) && expires >= 0)) &&
		(auth.base_url === undefined || auth.base_url === null || validBaseUrl(auth.base_url)) &&
		(auth.headers === undefined || auth.headers === null || validHeaders(auth.headers)) &&
		(auth.error_rules === undefined || auth.error_rules === null || validErrorRules(auth.error_rules)) &&
		(auth.request_metadata === undefined ||
			auth.request_metadata === null ||
			validRequestMetadata(auth.request_metadata))
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
			reply.auth.upstream_account_id ?? undefined,
			{
				baseUrl: reply.auth.base_url ?? undefined,
				headers: reply.auth.headers ?? undefined,
				errorRules: reply.auth.error_rules ?? undefined,
			},
			reply.auth.request_metadata?.project_id,
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
			// A plugin relay's refusal notice is manifest- or application-authored, never response text.
			throw new AuthUnavailable(provider, [lease.credential.notice ?? "No eligible account in the configured pool"], {
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
