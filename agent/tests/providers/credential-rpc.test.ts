import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { promptLog } from "../../src/features/prompt-epoch";
import type { CredentialReply, CredentialResponse } from "../../src/protocol/types";
import { DaemonCredentialRpc } from "../../src/providers/credential-rpc";
import { AuthUnavailable } from "../../src/providers/errors";
import { resolveModel, setProviderBaseUrlForTests } from "../../src/providers/models";
import { ClassifiedOutcome } from "../../src/providers/outcomes";
import { makeStreamFn } from "../../src/providers/stream";
import type { CredentialReportArgs, CredentialRequestArgs } from "../../src/transport/rpc";
import {
	chatStream,
	FakeCredentialDaemon,
	type ProviderServer,
	serveProvider,
	textOf,
	userContext,
} from "./fake-credential-daemon";

const CANARY = "synthetic-daemon-rpc-secret-canary";

function credential(request: CredentialRequestArgs, account: string): CredentialResponse {
	return {
		v: 1,
		type: "credential_response",
		id: `rpc-${request.attempt}`,
		request_id: request.requestId,
		lease_id: `lease-${account}-${request.attempt}`,
		generation: 1,
		account: { source: "vault", account_id: `opaque-${account}` },
		auth: { kind: "api_key", value: `${CANARY}-${account}` },
	};
}

function acknowledge(report: CredentialReportArgs, action: "complete" | "rotate"): CredentialReply {
	return {
		v: 1,
		type: "credential_reported",
		id: `ack-${report.lease.lease_id}`,
		request_id: report.requestId,
		lease_id: report.lease.lease_id,
		generation: 2,
		action,
	};
}

let server: ProviderServer | undefined;

afterEach(() => {
	server?.stop();
	server = undefined;
});

describe("credential rpc", () => {
	for (const failure of [401, 429]) {
		it(`test_runtime_awaits_report_ack_rotates_and_requests_every_turn[${failure}]`, async () => {
			server = serveProvider((_request, index) =>
				index === 0
					? Response.json({ error: CANARY }, { status: failure, headers: { "retry-after": "7" } })
					: chatStream(),
			);
			setProviderBaseUrlForTests("openai-chat", `${server.url}/v1`);
			const requests: CredentialRequestArgs[] = [];
			const reports: CredentialReportArgs[] = [];
			const accounts = ["one", "two", "two"];
			const port = {
				credentialRequest: async (args: CredentialRequestArgs): Promise<CredentialReply> => {
					requests.push(args);
					return credential(args, accounts[requests.length - 1] as string);
				},
				credentialReport: async (args: CredentialReportArgs): Promise<CredentialReply> => {
					reports.push(args);
					// The next lease is requested only after this acknowledgement.
					expect(requests).toHaveLength(reports.length);
					return acknowledge(args, args.outcome.outcome === "success" ? "complete" : "rotate");
				},
			};
			const stream = makeStreamFn(port, {
				provider: "openai-chat",
				model: "openai-chat:gpt-5",
				networkAllow: [],
			});
			const model = resolveModel("openai-chat:gpt-5");
			expect(textOf(await (await stream(model, userContext("first"), {})).result())).toBe("done");
			expect(textOf(await (await stream(model, userContext("next turn"), {})).result())).toBe("done");

			const [first, second, third] = requests;
			expect(first?.attempt).toBe(0);
			expect(first?.provider).toBe("openai-chat");
			expect(Object.keys(first ?? {})).not.toContain("session");
			expect(Object.keys(first ?? {})).not.toContain("affinity_key");
			expect(reports[0]?.outcome.outcome).toBe(failure === 401 ? "auth" : "rate_limit");
			expect(reports[0]?.streamStarted).toBe(false);
			expect(reports[0]?.outcome.retry_after_ms).toBe(failure === 429 ? 7000 : undefined);
			expect(second?.attempt).toBe(1);
			expect(second?.requestId).toBe(first?.requestId as string);
			expect(reports[1]?.outcome.outcome).toBe("success");
			expect(reports[1]?.streamStarted).toBe(true);
			expect(reports[1]?.outcome.usage).toBeUndefined();
			expect(third?.attempt).toBe(0);
			expect(third?.requestId).not.toBe(first?.requestId as string);
			expect(JSON.stringify(reports)).not.toContain(CANARY);
			expect(server.seen.map(request => request.headers.authorization)).toEqual([
				`Bearer ${CANARY}-one`,
				`Bearer ${CANARY}-two`,
				`Bearer ${CANARY}-two`,
			]);
		});
	}

	it("test_denial_has_typed_reason_and_redacts_daemon_error_text", async () => {
		const rpc = new DaemonCredentialRpc({
			credentialRequest: async args => ({
				v: 1,
				type: "credential_error",
				id: "rpc1",
				request_id: args.requestId,
				code: "auth_unavailable",
				reason: CANARY,
				retry_at_ms: 12345,
				generation: 1,
			}),
			credentialReport: async () => undefined,
		});
		const error = await rpc
			.acquire("openai", "openai:gpt-5", "model1", 0, undefined, new AbortController().signal)
			.then(
				() => undefined,
				(failure: unknown) => failure,
			);
		expect(error).toBeInstanceOf(AuthUnavailable);
		expect((error as AuthUnavailable).reason).toBe("auth_unavailable");
		expect((error as AuthUnavailable).retryAtMs).toBe(12345);
		expect(String(error)).not.toContain(CANARY);
	});

	it("test_cancelled_report_is_send_only_and_late_ack_is_discarded", async () => {
		const sent: { args: CredentialReportArgs; await: boolean }[] = [];
		const rpc = new DaemonCredentialRpc({
			credentialRequest: async args => credential(args, "one"),
			credentialReport: async (args, opts) => {
				sent.push({ args, await: opts?.await ?? true });
				return opts?.await === false ? undefined : acknowledge(args, "complete");
			},
		});
		const lease = await rpc.acquire("openai", "openai:gpt-5", "model1", 0, undefined, new AbortController().signal);
		try {
			expect(
				await rpc.report(lease, new ClassifiedOutcome("cancelled"), "model1", {
					wait: false,
				}),
			).toBe(false);
			expect(sent).toHaveLength(1);
			expect(sent[0]?.args.outcome.outcome).toBe("cancelled");
			expect(sent[0]?.await).toBe(false);
			expect(JSON.stringify(sent)).not.toContain(CANARY);
		} finally {
			lease.credential.clear();
		}
	});

	it("test_request_identity_is_logged_per_lease_without_identifiers", async () => {
		const lines: string[] = [];
		const log = spyOn(promptLog, "write").mockImplementation(line => {
			lines.push(line);
		});
		try {
			server = serveProvider((_request, index) =>
				index === 0 ? Response.json({ error: CANARY }, { status: 401 }) : chatStream(),
			);
			setProviderBaseUrlForTests("openai-chat", `${server.url}/v1`);
			const daemon = new FakeCredentialDaemon();
			daemon.secretFor = account => `${CANARY}-${account}`;
			const stream = makeStreamFn(daemon, {
				provider: "openai-chat",
				model: "openai-chat:gpt-5",
				networkAllow: [],
			});
			const model = resolveModel("openai-chat:gpt-5");
			await (await stream(model, userContext("first"), {})).result();
			await (await stream(model, userContext("second"), {})).result();
			const identities = lines.filter(line => line.startsWith("prompt_request_identity "));
			expect(
				identities.map(line => (JSON.parse(line.slice(line.indexOf(" ") + 1)) as { cause: string }).cause),
			).toEqual(["account_first_request", "account_boundary", "same_account"]);
			const printed = identities.join("\n");
			for (const hidden of [CANARY, "fake-account-1", "openai-chat", "gpt-5"]) {
				expect(printed).not.toContain(hidden);
			}
		} finally {
			log.mockRestore();
		}
	});
});
