import type { CredentialOutcome, OutcomeWire } from "../protocol/types";

/** Credential health reports carry classifications, never provider response text. */
export type OutcomeKind = CredentialOutcome;

const QUOTA_CODES: Record<string, true> = {
	insufficient_quota: true,
	quota_exhausted: true,
	quota_exceeded: true,
	usage_limit_reached: true,
	billing_hard_limit_reached: true,
	credit_balance_too_low: true,
};
const AUTH_CODES: Record<string, true> = {
	invalid_api_key: true,
	authentication_error: true,
	invalid_token: true,
	unauthorized: true,
};
const TRANSIENT_LIMIT_CODES: Record<string, true> = {
	concurrency_limit_exceeded: true,
	requests_per_minute_exceeded: true,
	tokens_per_minute_exceeded: true,
	rpm_limit_exceeded: true,
	tpm_limit_exceeded: true,
};
const MAX_RETRY_AFTER_SECONDS = 86_400;

export class ClassifiedOutcome {
	readonly kind: OutcomeKind;
	/** Seconds, already bounded to [0, 86400]. */
	readonly retryAfter: number | undefined;
	readonly scope: "account" | "model";

	constructor(kind: OutcomeKind, retryAfter?: number, scope: "account" | "model" = "account") {
		this.kind = kind;
		this.retryAfter = retryAfter;
		this.scope = scope;
	}

	get retryAfterMs(): number | undefined {
		return this.retryAfter === undefined ? undefined : Math.ceil(this.retryAfter * 1000);
	}

	toWire(): OutcomeWire {
		const wire: OutcomeWire = { outcome: this.kind, scope: this.scope };
		const retryAfterMs = this.retryAfterMs;
		if (retryAfterMs !== undefined) wire.retry_after_ms = retryAfterMs;
		return wire;
	}
}

/** `Retry-After` as delta-seconds or HTTP-date; anything non-finite is unknown. */
export function retryAfterSeconds(value: string | null | undefined): number | undefined {
	if (value === null || value === undefined) return undefined;
	const text = value.trim();
	if (text === "") return undefined;
	let seconds = Number(text);
	if (Number.isNaN(seconds) && !/^[+-]?(nan|inf(inity)?)$/i.test(text)) {
		const date = Date.parse(text);
		if (Number.isNaN(date)) return undefined;
		seconds = (date - Date.now()) / 1000;
	}
	if (!Number.isFinite(seconds)) return undefined;
	return Math.min(MAX_RETRY_AFTER_SECONDS, Math.max(0, seconds));
}

export function classifyResponse(status: number, retryAfter?: string | null, body?: unknown): ClassifiedOutcome {
	if (status >= 200 && status < 300) return new ClassifiedOutcome("success");
	const rawError = typeof body === "object" && body !== null && "error" in body ? body.error : undefined;
	const error: { type?: unknown; code?: unknown; scope?: unknown } =
		typeof rawError === "object" && rawError !== null && !Array.isArray(rawError) ? rawError : {};
	const codes = [error.type, error.code].filter((code): code is string => typeof code === "string");
	const hasAny = (table: Record<string, true>): boolean => codes.some(code => Object.hasOwn(table, code));
	const scope = error.scope === "model" ? "model" : "account";
	const delay = retryAfterSeconds(retryAfter);
	if (status === 401 || (status === 403 && hasAny(AUTH_CODES))) return new ClassifiedOutcome("auth");
	if ((status === 402 || status === 403 || status === 429) && hasAny(QUOTA_CODES)) {
		return new ClassifiedOutcome("quota", delay, scope);
	}
	if (status === 403) return new ClassifiedOutcome("policy", delay, scope);
	if (status === 429) {
		if (hasAny(TRANSIENT_LIMIT_CODES)) return new ClassifiedOutcome("transient");
		return new ClassifiedOutcome("rate_limit", delay);
	}
	return new ClassifiedOutcome("transient");
}
