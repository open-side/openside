/** Source diagnostics never acquire credentials or independently decide readiness. */
import { statuses } from "./subscriptions";

const SOURCE_NAMES: Record<string, true> = {
	"omp-broker": true,
	vault: true,
	env: true,
	command: true,
	"claude-code": true,
	"codex-cli": true,
	builtin: true,
};

export interface SourceRow {
	provider: string;
	source: string;
	available: boolean;
	reason: string;
	next_step: string;
}

export interface DaemonAccountMetadata {
	sources: unknown[];
	cached_for_ms?: unknown;
	truncated?: unknown;
}

/** Sanitized account metadata the daemon forwards as `credentials._diagnostics.accounts`. */
export function daemonAccountMetadata(credentials: Record<string, unknown>): DaemonAccountMetadata | undefined {
	const diagnostics = credentials._diagnostics;
	if (typeof diagnostics !== "object" || diagnostics === null || !("accounts" in diagnostics)) return undefined;
	const accounts = diagnostics.accounts;
	if (typeof accounts !== "object" || accounts === null || !("sources" in accounts)) return undefined;
	return Array.isArray(accounts.sources) ? { ...accounts, sources: accounts.sources } : undefined;
}

export function cliReason(installed: boolean): string {
	return installed ? "CLI installed; login readiness unknown" : "CLI not installed; login readiness unknown";
}

export function diagnosticSources(credentials: Record<string, unknown>, providers: string[]): SourceRow[] {
	const metadata = daemonAccountMetadata(credentials);
	const rows: SourceRow[] = [];
	if (metadata) {
		for (const source of metadata.sources) {
			if (typeof source !== "object" || source === null || !("name" in source)) continue;
			const name = source.name;
			if (typeof name !== "string" || !Object.hasOwn(SOURCE_NAMES, name)) continue;
			const accounts = "accounts" in source && Array.isArray(source.accounts) ? source.accounts : [];
			for (const provider of providers) {
				const relevant = accounts.filter(
					(account): account is { provider: unknown; available?: unknown } =>
						typeof account === "object" && account !== null && "provider" in account && account.provider === provider,
				);
				rows.push({
					provider,
					source: name,
					available: relevant.some(account => account.available === true),
					reason: relevant.length > 0 ? "Daemon account metadata available" : "No daemon account metadata for provider",
					next_step: "Manage the provider account pool in Models & accounts",
				});
			}
		}
		return rows;
	}
	for (const provider of providers) {
		rows.push({
			provider,
			source: "daemon",
			available: false,
			reason: "Daemon credential metadata required; readiness unknown",
			next_step: "Run openside doctor against the daemon",
		});
	}
	for (const status of statuses()) {
		rows.push({
			provider: status.provider,
			source: status.id,
			available: false,
			reason: cliReason(status.installed),
			next_step: "Manage the subscription account in Models & accounts",
		});
	}
	return rows;
}
