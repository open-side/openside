/** Authentication errors carry only application-authored diagnostic reasons. */
export class AuthUnavailable extends Error {
	override readonly name = "AuthUnavailable";
	readonly provider: string;
	readonly safeErrors: string[];
	readonly reason: string | undefined;
	readonly retryAtMs: number | undefined;

	constructor(
		provider: string,
		safeErrors: readonly string[] = [],
		options: { reason?: string; retryAtMs?: number } = {},
	) {
		const detail = safeErrors.join("; ") || "no usable credential source";
		super(`${provider}: ${detail}. Check the configured account pool in Models & accounts.`);
		this.provider = provider;
		this.safeErrors = [...safeErrors];
		this.reason = options.reason;
		this.retryAtMs = options.retryAtMs;
	}
}

/**
 * A person fixed the session's thinking effort and the candidate model cannot serve it.
 * The run fails instead of silently downgrading; the text names only catalog data.
 */
export class EffortUnsupported extends Error {
	override readonly name = "EffortUnsupported";

	constructor(effort: string, model: string, supported: readonly string[]) {
		super(`effort ${effort} is not supported by ${model}; supported: ${supported.join(", ") || "none"}`);
	}
}

/** Walks `cause` chains so a wrapped pool failure is never mistaken for a provider error. */
export function findAuthUnavailable(error: unknown): AuthUnavailable | undefined {
	const seen = new Set<unknown>();
	let cause: unknown = error;
	while (cause instanceof Error && !seen.has(cause)) {
		seen.add(cause);
		if (cause instanceof AuthUnavailable) return cause;
		cause = cause.cause;
	}
	return undefined;
}
