/**
 * `auth-refresh`: refreshes one stored grant for the daemon and prints the
 * result once. Rust owns refresh policy (when, single-flight, CAS storage);
 * this process only runs pi-ai's provider refresh under the egress guard.
 */
import { type OAuthCredentials, type OAuthProvider, refreshOAuthToken } from "@oh-my-pi/pi-ai";
import { isDefinitiveOAuthFailure } from "@oh-my-pi/pi-ai/error";
import {
	fromPayload,
	type HelperErrorCode,
	type HelperInbound,
	type HelperIo,
	parseInbound,
	REFRESH_ORIGINS,
	stdioHelperIo,
	toPayload,
} from "./protocol/auth-helper";
import { installEgress } from "./providers/egress";

/** Injection seam: tests supply a fake refresher and never touch the network. */
export interface RefreshDeps {
	refresh(provider: string, credentials: OAuthCredentials, signal: AbortSignal): Promise<OAuthCredentials>;
	installEgress(allow: readonly string[]): void;
}

const defaultDeps: RefreshDeps = {
	refresh: (provider, credentials, signal) => refreshOAuthToken(provider as OAuthProvider, credentials, signal),
	installEgress: allow => {
		installEgress(allow);
	},
};

export async function authRefresh(io: HelperIo = stdioHelperIo(), deps: RefreshDeps = defaultDeps): Promise<number> {
	const fail = (code: HelperErrorCode): number => {
		try {
			io.write({ type: "error", code });
		} catch {
			// Nothing else can be reported; the exit status still says failure.
		}
		return 1;
	};
	let request: HelperInbound | undefined;
	try {
		const next = await io.lines.next();
		request = next.done ? undefined : parseInbound(next.value);
	} catch {
		return fail("failed");
	}
	if (request?.type !== "refresh" || !Object.hasOwn(REFRESH_ORIGINS, request.provider)) return fail("failed");
	deps.installEgress(REFRESH_ORIGINS[request.provider] ?? []);
	const stored = fromPayload(request.credentials);
	let refreshed: OAuthCredentials;
	try {
		refreshed = await deps.refresh(request.provider, stored, new AbortController().signal);
	} catch (error) {
		// pi-ai's own dead-grant classifier; the message itself is never forwarded.
		return fail(error instanceof Error && isDefinitiveOAuthFailure(error.message) ? "invalid_grant" : "failed");
	}
	// Providers that do not rotate omit fields; the stored grant's values stay valid.
	const payload = toPayload({
		access: refreshed.access,
		refresh: refreshed.refresh || stored.refresh,
		expires: refreshed.expires,
		projectId: refreshed.projectId || stored.projectId,
		accountId: refreshed.accountId || stored.accountId,
		enterpriseUrl: refreshed.enterpriseUrl || stored.enterpriseUrl,
		apiEndpoint: refreshed.apiEndpoint || stored.apiEndpoint,
	});
	if (!payload) return fail("failed");
	try {
		io.write({ type: "credentials", credentials: payload });
	} catch {
		return fail("failed");
	}
	return 0;
}
