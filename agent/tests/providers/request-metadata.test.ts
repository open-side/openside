import { afterEach, describe, expect, it } from "bun:test";
import { DEFAULT_ANTIGRAVITY_VERSION } from "@oh-my-pi/pi-catalog/wire/gemini-headers";
import type { CredentialReply, CredentialRequestMetadata } from "../../src/protocol/types";
import type { CredentialRpcPort } from "../../src/providers/credential-rpc";
import { AuthUnavailable } from "../../src/providers/errors";
import { resolveModel, setProviderBaseUrlForTests } from "../../src/providers/models";
import { makeStreamFn } from "../../src/providers/stream";
import {
	captureLeases,
	FakeCredentialDaemon,
	type LeaseCapture,
	serveProvider,
	textOf,
	userContext,
} from "./fake-credential-daemon";

const servers: { stop(): void }[] = [];
let leaseCapture: LeaseCapture | undefined;
let restoreEnv: (() => void) | undefined;

afterEach(() => {
	for (const server of servers.splice(0)) server.stop();
	leaseCapture?.restore();
	leaseCapture = undefined;
	restoreEnv?.();
	restoreEnv = undefined;
});

/** The fake daemon, with `metadata` attached to every lease it issues. */
function leasing(daemon: FakeCredentialDaemon, metadata: CredentialRequestMetadata | undefined): CredentialRpcPort {
	return {
		credentialRequest: async (args, signal) => {
			const reply: CredentialReply = await daemon.credentialRequest(args, signal);
			if (reply.type === "credential_response" && metadata !== undefined) reply.auth.request_metadata = metadata;
			return reply;
		},
		credentialReport: (args, options) => daemon.credentialReport(args, options),
	};
}

/** One Cloud Code SSE response that finishes the turn with `text`. */
function cloudCodeStream(text: string): Response {
	const chunk = {
		response: {
			candidates: [{ content: { role: "model", parts: [{ text }] }, finishReason: "STOP" }],
			usageMetadata: { promptTokenCount: 1, candidatesTokenCount: 1, totalTokenCount: 2 },
		},
	};
	return new Response(`data: ${JSON.stringify(chunk)}\n\n`, { headers: { "content-type": "text/event-stream" } });
}

/** Sets PI_AI_ANTIGRAVITY_VERSION to `value` (undefined = unset) until `afterEach` restores it. */
function withAntigravityVersion(value: string | undefined): void {
	const previous = process.env.PI_AI_ANTIGRAVITY_VERSION;
	if (value === undefined) delete process.env.PI_AI_ANTIGRAVITY_VERSION;
	else process.env.PI_AI_ANTIGRAVITY_VERSION = value;
	restoreEnv = () => {
		if (previous === undefined) delete process.env.PI_AI_ANTIGRAVITY_VERSION;
		else process.env.PI_AI_ANTIGRAVITY_VERSION = previous;
	};
}

async function settle(run: Promise<unknown>): Promise<unknown> {
	return run.then(
		() => undefined,
		(error: unknown) => error,
	);
}

describe("lease request metadata", () => {
	for (const [provider, id] of [
		["google-gemini-cli", "google-gemini-cli:gemini-2.5-pro"],
		["google-antigravity", "google-antigravity:gemini-3-pro-high"],
	] as const) {
		it(`test_cloud_code_lease_succeeds_with_project_and_bearer[${provider}]`, async () => {
			leaseCapture = captureLeases();
			withAntigravityVersion(undefined);
			const daemon = new FakeCredentialDaemon(1);
			daemon.kind = "bearer";
			const server = serveProvider(() => cloudCodeStream("done"));
			servers.push(server);
			setProviderBaseUrlForTests(provider, server.url);
			const model = resolveModel(id);
			const stream = makeStreamFn(leasing(daemon, { project_id: "proj-lease-1" }), {
				provider,
				model: id,
				networkAllow: [],
			});
			expect(process.env.PI_AI_ANTIGRAVITY_VERSION).toBe(DEFAULT_ANTIGRAVITY_VERSION);
			const message = await (await stream(model, userContext("hello"), {})).result();
			expect(message.stopReason).toBe("stop");
			expect(textOf(message)).toBe("done");
			expect(server.seen).toHaveLength(1);
			const [first] = server.seen;
			expect(first?.headers.authorization).toBe("Bearer fake-account-1");
			expect(JSON.parse(first?.body ?? "{}").project).toBe("proj-lease-1");
			expect(JSON.stringify(server.seen)).not.toContain("daemon-managed");
			expect(daemon.outcomes()).toEqual(["success"]);
			expect(leaseCapture.leases.every(lease => lease.credential.projectId === undefined)).toBe(true);
			expect(daemon.issued.every(reply => reply.auth.request_metadata === undefined)).toBe(true);
		});
	}

	it("test_operator_antigravity_version_is_kept", () => {
		withAntigravityVersion("9.9.9");
		makeStreamFn(leasing(new FakeCredentialDaemon(1), undefined), {
			provider: "google-antigravity",
			model: "google-antigravity:gemini-3-pro-high",
			networkAllow: [],
		});
		expect(process.env.PI_AI_ANTIGRAVITY_VERSION).toBe("9.9.9");
	});

	it("test_missing_project_fails_closed_before_any_request", async () => {
		const daemon = new FakeCredentialDaemon(1);
		daemon.kind = "bearer";
		const server = serveProvider(() => Response.json({}));
		servers.push(server);
		setProviderBaseUrlForTests("google-gemini-cli", server.url);
		const id = "google-gemini-cli:gemini-2.5-pro";
		const stream = makeStreamFn(leasing(daemon, undefined), {
			provider: "google-gemini-cli",
			model: id,
			networkAllow: [],
		});
		const error = await settle(Promise.resolve(stream(resolveModel(id), userContext("hello"), {})));
		expect(error).toBeInstanceOf(AuthUnavailable);
		expect(server.seen).toHaveLength(0);
		expect(daemon.reports).toHaveLength(1);
	});

	it("test_invalid_metadata_rejects_the_lease", async () => {
		const daemon = new FakeCredentialDaemon(1);
		daemon.kind = "bearer";
		const server = serveProvider(() => Response.json({}));
		servers.push(server);
		setProviderBaseUrlForTests("google-gemini-cli", server.url);
		const id = "google-gemini-cli:gemini-2.5-pro";
		const stream = makeStreamFn(leasing(daemon, { project_id: "bad project" }), {
			provider: "google-gemini-cli",
			model: id,
			networkAllow: [],
		});
		const error = await settle(Promise.resolve(stream(resolveModel(id), userContext("hello"), {})));
		expect(error).toBeInstanceOf(AuthUnavailable);
		expect((error as AuthUnavailable).reason).toBe("invalid_credential_request");
		expect(server.seen).toHaveLength(0);
		expect(daemon.issued.every(reply => reply.auth.value === "")).toBe(true);
	});
});
