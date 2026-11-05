/** Daemon-supplied public metadata; standalone account readiness is unknown. */
import { cliReason, daemonAccountMetadata } from "./sources";
import { statuses } from "./subscriptions";

export async function accountsReport(
	credentials: Record<string, unknown>,
	_models: Record<string, unknown>,
): Promise<Record<string, unknown>> {
	const metadata = daemonAccountMetadata(credentials);
	if (metadata) {
		return {
			sources: metadata.sources,
			cached_for_ms: metadata.cached_for_ms ?? 0,
			truncated: metadata.truncated ?? false,
		};
	}
	return {
		sources: statuses().map(status => ({
			name: status.id,
			kind: status.id,
			available: false,
			reason: cliReason(status.installed),
			accounts: [],
		})),
		cached_for_ms: 0,
		truncated: false,
	};
}
