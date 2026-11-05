/** Credential-free sidecar diagnostics; the daemon owns account readiness. */
import { diagnosticSources } from "./sources";

export function doctorReport(
	credentials: Record<string, unknown>,
	models: Record<string, unknown>,
): Record<string, unknown> {
	const defaultModel = typeof models.default === "string" ? models.default : undefined;
	const fallback: unknown[] = Array.isArray(models.fallback) ? models.fallback : [];
	const identifiers = [
		...(defaultModel === undefined ? [] : [defaultModel]),
		...fallback.filter((value): value is string => typeof value === "string"),
	];
	const providers = [...new Set(identifiers.filter(id => id.includes(":")).map(id => id.slice(0, id.indexOf(":"))))];
	const rows = diagnosticSources(credentials, providers);
	return {
		credentials: rows,
		models: {
			default: defaultModel ?? null,
			fallback,
			providers_ok:
				providers.length > 0 &&
				providers.every(provider => rows.some(row => row.provider === provider && row.available)),
		},
	};
}
