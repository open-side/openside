/**
 * Injects one daemon lease into the requests of one model attempt. pi-ai builds
 * its own auth headers from `apiKey`; this layer strips every credential header
 * and re-applies the lease, so the daemon's auth kind (not a token prefix)
 * decides the header and no credential reaches an unexpected origin.
 */
import type { FetchImpl } from "@oh-my-pi/pi-ai";
import type { AccountSource, CredentialErrorRules, CredentialKind } from "../protocol/types";
import { AuthUnavailable } from "./errors";
import { ClassifiedOutcome, classifyRelayResponse, classifyResponse } from "./outcomes";
import type { CredentialSecret } from "./secret";

export const AUTH_HEADERS = [
	"authorization",
	"x-api-key",
	"x-goog-api-key",
	"chatgpt-account-id",
	"openai-organization",
	"openai-project",
] as const;

const CLAUDE_CODE_BETA = "claude-code-20250219";

/** Cloud Code transports build the request body around the account's project. */
const CLOUD_CODE_PROVIDERS: readonly string[] = ["google-gemini-cli", "google-antigravity"];

const REDACTED = "CredentialAttempt(credentials=**********)";

/** Lease-supplied routing: a plugin relay's endpoint, the extra headers it expects and its refusal rules. */
export interface LeaseEndpoint {
	baseUrl?: string;
	headers?: Readonly<Record<string, string>>;
	errorRules?: CredentialErrorRules;
}

export class CredentialAttempt {
	readonly provider: string;
	readonly source: AccountSource;
	readonly kind: CredentialKind;
	readonly secret: CredentialSecret;
	upstreamAccountId: string | undefined;
	/** Daemon-issued endpoint replacing the model's default base URL; absent for first-party accounts. */
	readonly baseUrl: string | undefined;
	readonly headers: Readonly<Record<string, string>>;
	readonly errorRules: CredentialErrorRules | undefined;
	/** Daemon-issued Cloud Code project (`request_metadata.project_id`); private like the token. */
	projectId: string | undefined;
	/**
	 * Fixed, application-authored explanation of the last refusal a relay
	 * signalled (unsupported client, sign-in needed, no capacity); it replaces
	 * the status-only error text and never carries response content.
	 */
	notice: string | undefined;
	/** Classification of the last provider response this attempt saw. */
	outcome: ClassifiedOutcome = new ClassifiedOutcome("transient");
	/** Whether any provider response reached this attempt. */
	responded = false;
	/**
	 * Set when this layer refused a request: pi-ai turns a thrown fetch error
	 * into message text, so the stream driver reads the typed refusal here.
	 */
	refusal: AuthUnavailable | undefined;

	constructor(
		provider: string,
		source: AccountSource,
		kind: CredentialKind,
		secret: CredentialSecret,
		upstreamAccountId?: string,
		endpoint: LeaseEndpoint = {},
		projectId?: string,
	) {
		this.provider = provider;
		this.source = source;
		this.kind = kind;
		this.secret = secret;
		this.upstreamAccountId = upstreamAccountId;
		this.baseUrl = endpoint.baseUrl;
		this.headers = endpoint.headers ?? {};
		this.errorRules = endpoint.errorRules;
		this.projectId = projectId;
	}

	/** Ends the attempt: later requests through its fetch fail closed. */
	clear(): void {
		this.secret.clear();
		this.upstreamAccountId = undefined;
		this.projectId = undefined;
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

/** Lower-cased plain header record; pi-ai's Anthropic transport requires a record. */
export function headerRecord(headers: RequestInit["headers"] | undefined): Record<string, string> {
	const record: Record<string, string> = {};
	if (headers === undefined) return record;
	new Headers(headers).forEach((value, name) => {
		record[name] = value;
	});
	return record;
}

function originKey(url: URL): string {
	return `${url.protocol}//${url.hostname}:${url.port || (url.protocol === "https:" ? "443" : "80")}`;
}

/**
 * The `apiKey` pi-ai builds this attempt's request with. Cloud Code providers
 * parse it as structured credentials and put the project into the request
 * body, so the lease's project rides along with a placeholder token (the real
 * bearer is re-applied per request). Without a project the lease is unusable:
 * `attempt.refusal` is set and `undefined` returned.
 */
export function requestApiKey(attempt: CredentialAttempt, placeholder: string): string | undefined {
	if (!CLOUD_CODE_PROVIDERS.includes(attempt.provider)) return placeholder;
	if (attempt.projectId === undefined) {
		attempt.refusal = new AuthUnavailable(attempt.provider, ["daemon credential missing account metadata"]);
		return undefined;
	}
	return JSON.stringify({ token: placeholder, projectId: attempt.projectId });
}

/**
 * `endpoint` is the provider base URL the model was built with; requests to any
 * other scheme/host/port are refused before they reach `next`.
 */
export function makeCredentialFetch(attempt: CredentialAttempt, endpoint: string, next: FetchImpl): FetchImpl {
	const target = originKey(new URL(endpoint));
	const refuse = (reason: string): never => {
		attempt.refusal = new AuthUnavailable(attempt.provider, [reason]);
		throw attempt.refusal;
	};
	return async (input, init) => {
		const url = new URL(input instanceof Request ? input.url : input);
		const token = attempt.secret.reveal();
		if (!token) refuse("daemon credential request required");
		if (originKey(url) !== target) refuse("refusing credentials to an unexpected origin");
		// A plugin relay authenticates by the person's identity alone and picks the
		// ChatGPT account itself; every other Codex source must carry its account id.
		if (attempt.provider === "openai-codex" && !attempt.upstreamAccountId && attempt.source !== "plugin") {
			refuse("daemon credential missing account metadata");
		}
		const request: RequestInit =
			input instanceof Request
				? {
						method: input.method,
						headers: input.headers,
						body: input.body === null ? undefined : await input.arrayBuffer(),
						signal: input.signal,
						...init,
					}
				: { ...init };
		const headers = headerRecord(request.headers);
		for (const name of AUTH_HEADERS) delete headers[name];
		for (const [name, value] of Object.entries(attempt.headers)) {
			// The RPC layer already rejects these names; this keeps the invariant local too.
			if (!(AUTH_HEADERS as readonly string[]).includes(name.toLowerCase())) headers[name.toLowerCase()] = value;
		}
		if (attempt.provider === "google" && attempt.kind === "api_key") {
			headers["x-goog-api-key"] = token;
		} else if (attempt.provider === "anthropic" && attempt.kind === "api_key") {
			headers["x-api-key"] = token;
		} else {
			headers.authorization = `Bearer ${token}`;
		}
		if (attempt.provider === "openai-codex" && attempt.upstreamAccountId) {
			headers["chatgpt-account-id"] = attempt.upstreamAccountId;
			headers.originator = "openside";
		}
		if (attempt.source === "plugin" && attempt.kind === "claude_code") {
			// The relay serves Claude subscription capacity only to Claude Code-shaped
			// requests; pi-ai drops this beta on tool-less (utility) requests, which
			// upstream then refuses with a bare 429.
			const betas = (headers["anthropic-beta"] ?? "").split(",").filter(beta => beta.trim() !== "");
			if (!betas.includes(CLAUDE_CODE_BETA)) headers["anthropic-beta"] = [CLAUDE_CODE_BETA, ...betas].join(",");
		}
		let response: Response;
		try {
			response = await next(url.href, { ...request, headers });
		} finally {
			// The record may outlive this call in debug dumps or retained inits.
			for (const name of AUTH_HEADERS) delete headers[name];
		}
		attempt.responded = true;
		let body: unknown;
		if (response.status >= 400) {
			let text: string | undefined;
			try {
				text = await response.clone().text();
				body = JSON.parse(text);
			} catch {
				// A relay answers some refusals in plain text; only the classifier reads it.
				body = attempt.source === "plugin" ? text : undefined;
			}
		}
		if (attempt.source === "plugin") {
			const relay = classifyRelayResponse(response.status, response.headers, body, attempt.errorRules);
			attempt.outcome = relay.outcome;
			attempt.notice = relay.notice;
		} else {
			attempt.outcome = classifyResponse(response.status, response.headers.get("retry-after"), body);
		}
		return response;
	};
}
