/**
 * Injects one daemon lease into the requests of one model attempt. pi-ai builds
 * its own auth headers from `apiKey`; this layer strips every credential header
 * and re-applies the lease, so the daemon's auth kind (not a token prefix)
 * decides the header and no credential reaches an unexpected origin.
 */
import type { FetchImpl } from "@oh-my-pi/pi-ai";
import type { AccountSource, CredentialKind } from "../protocol/types";
import { AuthUnavailable } from "./errors";
import { ClassifiedOutcome, classifyResponse } from "./outcomes";
import type { CredentialSecret } from "./secret";

export const AUTH_HEADERS = [
	"authorization",
	"x-api-key",
	"x-goog-api-key",
	"chatgpt-account-id",
	"openai-organization",
	"openai-project",
] as const;

const REDACTED = "CredentialAttempt(credentials=**********)";

export class CredentialAttempt {
	readonly provider: string;
	readonly source: AccountSource;
	readonly kind: CredentialKind;
	readonly secret: CredentialSecret;
	upstreamAccountId: string | undefined;
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
	) {
		this.provider = provider;
		this.source = source;
		this.kind = kind;
		this.secret = secret;
		this.upstreamAccountId = upstreamAccountId;
	}

	/** Ends the attempt: later requests through its fetch fail closed. */
	clear(): void {
		this.secret.clear();
		this.upstreamAccountId = undefined;
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
		if (attempt.provider === "openai-codex" && !attempt.upstreamAccountId) {
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
			try {
				body = await response.clone().json();
			} catch {
				body = undefined;
			}
		}
		attempt.outcome = classifyResponse(response.status, response.headers.get("retry-after"), body);
		return response;
	};
}
