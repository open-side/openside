import { afterEach, describe, expect, it, spyOn } from "bun:test";
import type { Api, FetchImpl, Model } from "@oh-my-pi/pi-ai";
import { CLAUDE_CODE_IDENTITY } from "../../src/providers/anthropic";
import { AuthUnavailable } from "../../src/providers/errors";
import { resolveModel, setProviderBaseUrlForTests } from "../../src/providers/models";
import { CredentialAttempt, makeCredentialFetch } from "../../src/providers/request-auth";
import { CredentialSecret } from "../../src/providers/secret";
import { makeStreamFn } from "../../src/providers/stream";
import {
	captureLeases,
	chatStream,
	FakeCredentialDaemon,
	type LeaseCapture,
	serveProvider,
	textOf,
	userContext,
} from "./fake-credential-daemon";

const CANARY = "synthetic-per-request-secret-canary";

function attempt(provider = "openai", token = CANARY): CredentialAttempt {
	return new CredentialAttempt(provider, "vault", "api_key", new CredentialSecret(token));
}

function printed(...values: unknown[]): string {
	return values.map(value => `${String(value)} ${JSON.stringify(value)} ${Bun.inspect(value)}`).join(" ");
}

interface Sent {
	url: string;
	headers: Record<string, string>;
}

function recordingFetch(answer: (sent: Sent) => Response | Promise<Response>): {
	fetch: FetchImpl;
	sent: Sent[];
} {
	const sent: Sent[] = [];
	const fetch: FetchImpl = async (input, init) => {
		const headers = init?.headers as Record<string, string>;
		const record = { url: String(input), headers };
		sent.push(record);
		return answer({ url: record.url, headers: { ...headers } });
	};
	return { fetch, sent };
}

const servers: { stop(): void }[] = [];
let leaseCapture: LeaseCapture | undefined;

afterEach(() => {
	for (const server of servers.splice(0)) server.stop();
	leaseCapture?.restore();
	leaseCapture = undefined;
});

function chatModel(baseUrl: string, provider = "openai-chat", id = "openai-chat:gpt-5"): Model<Api> {
	setProviderBaseUrlForTests(provider, `${baseUrl}/v1`);
	return resolveModel(id);
}

describe("request credentials", () => {
	it("test_request_auth_classifies_retry_after_and_scrubs_headers", async () => {
		const used = attempt();
		const { fetch, sent } = recordingFetch(({ headers }) => {
			expect(headers.authorization).toBe(`Bearer ${CANARY}`);
			return Response.json({ error: CANARY }, { status: 429, headers: { "retry-after": "23" } });
		});
		const auth = makeCredentialFetch(used, "https://api.openai.com", fetch);
		const response = await auth("https://api.openai.com/v1/responses", {
			method: "POST",
			headers: { authorization: "Bearer pi-ai-placeholder" },
			body: "{}",
		});
		expect(response.status).toBe(429);
		expect(used.outcome.kind).toBe("rate_limit");
		expect(used.outcome.retryAfter).toBe(23);
		expect(sent[0]?.headers.authorization).toBeUndefined();
		expect(printed(auth, used, used.secret, used.outcome)).not.toContain(CANARY);
		used.clear();
		expect(used.secret.reveal()).toBe("");
	});

	it("test_foreign_origin_and_missing_attempt_never_send_credentials", async () => {
		const { fetch, sent } = recordingFetch(() => {
			throw new Error("Foreign requests must not reach the transport");
		});
		const ended = attempt();
		ended.clear();
		await expect(
			makeCredentialFetch(ended, "https://api.openai.com", fetch)("https://api.openai.com/v1/responses", {}),
		).rejects.toBeInstanceOf(AuthUnavailable);
		const used = attempt();
		const auth = makeCredentialFetch(used, "https://api.openai.com", fetch);
		for (const url of [
			"https://foreign.invalid/path",
			"http://api.openai.com/path",
			"https://api.openai.com:444/path",
		]) {
			const error = await auth(url, {}).then(
				() => undefined,
				(failure: unknown) => failure,
			);
			expect(error).toBeInstanceOf(AuthUnavailable);
			expect(String(error)).not.toContain(CANARY);
		}
		expect(sent).toHaveLength(0);
		used.clear();
	});

	it("test_concurrent_attempts_do_not_share_credentials", async () => {
		const seen: string[] = [];
		const { promise: ready, resolve } = Promise.withResolvers<void>();
		const { fetch } = recordingFetch(async ({ headers }) => {
			seen.push(headers.authorization ?? "");
			if (seen.length === 2) resolve();
			await ready;
			return Response.json({ ok: true });
		});
		const run = async (token: string): Promise<void> => {
			const used = attempt("openai", token);
			await makeCredentialFetch(
				used,
				"https://api.openai.com",
				fetch,
			)("https://api.openai.com/v1/responses", {
				method: "POST",
			});
			expect(used.outcome.kind).toBe("success");
			used.clear();
		};
		await Promise.all([run("fake-account-1"), run("fake-account-2")]);
		expect(new Set(seen)).toEqual(new Set(["Bearer fake-account-1", "Bearer fake-account-2"]));
	});

	it("test_codex_uses_daemon_account_metadata_without_opening_cli_home", async () => {
		const used = new CredentialAttempt(
			"openai-codex",
			"codex-cli",
			"codex_cli",
			new CredentialSecret(CANARY),
			"fake-upstream-account",
		);
		const { fetch, sent } = recordingFetch(({ headers }) => {
			expect(headers["chatgpt-account-id"]).toBe("fake-upstream-account");
			expect(headers.originator).toBe("openside");
			expect(headers.authorization).toBe(`Bearer ${CANARY}`);
			return Response.json({ error: CANARY }, { status: 401 });
		});
		await makeCredentialFetch(
			used,
			"https://chatgpt.com/backend-api",
			fetch,
		)("https://chatgpt.com/backend-api/codex/responses", {
			method: "POST",
			headers: { "chatgpt-account-id": "jwt-derived" },
		});
		expect(used.outcome.kind).toBe("auth");
		expect(sent[0]?.headers["chatgpt-account-id"]).toBeUndefined();
		used.clear();
		const missing = new CredentialAttempt("openai-codex", "codex-cli", "codex_cli", new CredentialSecret(CANARY));
		await expect(
			makeCredentialFetch(
				missing,
				"https://chatgpt.com/backend-api",
				fetch,
			)("https://chatgpt.com/backend-api/codex/responses", {}),
		).rejects.toBeInstanceOf(AuthUnavailable);
	});

	for (const failure of ["rate_limit", "auth", "quota"] as const) {
		it(`test_daemon_rotates_accounts_and_affinity_sticks_across_turns[${failure}]`, async () => {
			leaseCapture = captureLeases();
			const daemon = new FakeCredentialDaemon();
			const server = serveProvider((_request, index) =>
				index === 0
					? Response.json(
							{
								error: {
									code: failure === "quota" ? "insufficient_quota" : CANARY,
								},
							},
							{
								status: failure === "auth" ? 401 : 429,
								headers: { "retry-after": "42" },
							},
						)
					: chatStream(),
			);
			servers.push(server);
			const model = chatModel(server.url);
			const stream = makeStreamFn(daemon, {
				provider: "openai-chat",
				model: "openai-chat:gpt-5",
				networkAllow: [],
			});
			const first = await (await stream(model, userContext("first"), {})).result();
			const second = await (await stream(model, userContext("second"), {})).result();
			expect(textOf(first)).toBe("done");
			expect(textOf(second)).toBe("done");
			expect(server.seen.map(request => request.headers.authorization)).toEqual([
				"Bearer fake-account-1",
				"Bearer fake-account-2",
				"Bearer fake-account-2",
			]);
			expect(daemon.outcomes()).toEqual([failure, "success", "success"]);
			if (failure === "rate_limit") expect(daemon.cooldowns.get("fake-account-1")).toBe(42_000);
			if (failure === "auth") expect(daemon.invalid.has("fake-account-1")).toBe(true);
			expect(new Set(daemon.requests.map(request => request.provider))).toEqual(new Set(["openai-chat"]));
			expect(daemon.requests.map(request => request.attempt)).toEqual([0, 1, 0]);
			expect(leaseCapture.leases.every(lease => lease.credential.secret.reveal() === "")).toBe(true);
			expect(daemon.issued.every(reply => reply.auth.value === "")).toBe(true);
		});
	}

	it("test_exhausted_pool_is_typed_and_never_uses_environment", async () => {
		leaseCapture = captureLeases();
		const previous = process.env.OPENAI_API_KEY;
		process.env.OPENAI_API_KEY = CANARY;
		try {
			const daemon = new FakeCredentialDaemon();
			const server = serveProvider(() =>
				Response.json({ error: CANARY }, { status: 429, headers: { "retry-after": "17" } }),
			);
			servers.push(server);
			const model = chatModel(server.url);
			const stream = makeStreamFn(daemon, {
				provider: "openai-chat",
				model: "openai-chat:gpt-5",
				networkAllow: [],
			});
			const error = await Promise.resolve(stream(model, userContext("first"), {})).then(
				() => undefined,
				(failure: unknown) => failure,
			);
			expect(error).toBeInstanceOf(AuthUnavailable);
			expect((error as AuthUnavailable).reason).toBe("auth_unavailable");
			expect(printed(error, daemon.reports, model)).not.toContain(CANARY);
			expect(server.seen.map(request => request.headers.authorization)).toEqual([
				"Bearer fake-account-1",
				"Bearer fake-account-2",
			]);
			expect(leaseCapture.leases.every(lease => lease.credential.secret.reveal() === "")).toBe(true);
		} finally {
			if (previous === undefined) delete process.env.OPENAI_API_KEY;
			else process.env.OPENAI_API_KEY = previous;
		}
	});

	it("test_retry_limit_bounds_account_rotations", async () => {
		leaseCapture = captureLeases();
		const daemon = new FakeCredentialDaemon(65);
		const server = serveProvider(() => Response.json({ error: CANARY }, { status: 401 }));
		servers.push(server);
		const model = chatModel(server.url);
		const stream = makeStreamFn(daemon, {
			provider: "openai-chat",
			model: "openai-chat:gpt-5",
			networkAllow: [],
		});
		const error = await Promise.resolve(stream(model, userContext("first"), {})).then(
			() => undefined,
			(failure: unknown) => failure,
		);
		expect(error).toBeInstanceOf(AuthUnavailable);
		expect((error as AuthUnavailable).reason).toBe("attempt_limit");
		expect(daemon.reports).toHaveLength(64);
		expect(daemon.requests).toHaveLength(64);
		expect(daemon.requests.map(request => request.attempt)).toEqual(Array.from({ length: 64 }, (_, index) => index));
		expect(leaseCapture.leases.every(lease => lease.credential.secret.reveal() === "")).toBe(true);
	});

	it("test_stream_is_not_replayed_after_becoming_visible", async () => {
		leaseCapture = captureLeases();
		const daemon = new FakeCredentialDaemon();
		let transportCalls = 0;
		const transport = spyOn(globalThis, "fetch").mockImplementation((async () => {
			transportCalls++;
			let sentChunk = false;
			return new Response(
				new ReadableStream({
					pull(controller) {
						if (sentChunk) {
							// The second read fails after visible output, like a dropped connection.
							controller.error(new Error(CANARY));
							return;
						}
						sentChunk = true;
						const chunk = {
							id: "fake-chat",
							object: "chat.completion.chunk",
							created: 1,
							model: "gpt-5",
							choices: [
								{
									index: 0,
									delta: { role: "assistant", content: "partial" },
									finish_reason: null,
								},
							],
						};
						controller.enqueue(new TextEncoder().encode(`data: ${JSON.stringify(chunk)}\n\n`));
					},
				}),
				{ headers: { "content-type": "text/event-stream" } },
			);
		}) as unknown as typeof fetch);
		try {
			const model = chatModel("http://127.0.0.1:9");
			const stream = makeStreamFn(daemon, {
				provider: "openai-chat",
				model: "openai-chat:gpt-5",
				networkAllow: [],
				maxRetryDelayMs: 0,
			});
			const result = await (await stream(model, userContext("first"), {})).result();
			expect(textOf(result)).toBe("partial");
			expect(result.stopReason).toBe("error");
			expect(result.errorMessage).toBe("Provider stream interrupted; details redacted");
			expect(printed(result)).not.toContain(CANARY);
			expect(transportCalls).toBe(1);
			expect(daemon.requests).toHaveLength(1);
			expect(daemon.reports).toHaveLength(1);
			expect(daemon.reports[0]?.streamStarted).toBe(true);
			expect(daemon.reports[0]?.outcome.outcome).toBe("transient");
			expect(leaseCapture.leases[0]?.credential.secret.reveal()).toBe("");
		} finally {
			transport.mockRestore();
		}
	});

	it("test_cancel_reports_without_waiting_for_daemon_ack", async () => {
		leaseCapture = captureLeases();
		const daemon = new FakeCredentialDaemon();
		const { promise: entered, resolve } = Promise.withResolvers<void>();
		const server = serveProvider(() => {
			resolve();
			return new Promise<Response>(() => undefined);
		});
		servers.push(server);
		const model = chatModel(server.url);
		const stream = makeStreamFn(daemon, {
			provider: "openai-chat",
			model: "openai-chat:gpt-5",
			networkAllow: [],
		});
		const controller = new AbortController();
		const pending = Promise.resolve(stream(model, userContext("first"), { signal: controller.signal }));
		await entered;
		controller.abort();
		const result = await (await pending).result();
		expect(result.stopReason).toBe("aborted");
		expect(daemon.reports).toHaveLength(1);
		expect(daemon.reports[0]?.outcome.outcome).toBe("cancelled");
		expect(daemon.reports[0]?.await).toBe(false);
		expect(leaseCapture.leases[0]?.credential.secret.reveal()).toBe("");
	});

	for (const kind of ["api_key", "claude_code"] as const) {
		it(`test_anthropic_shaping_is_selected_by_daemon_auth_kind[${kind}]`, async () => {
			const daemon = new FakeCredentialDaemon(1);
			daemon.kind = kind;
			daemon.secretFor = () => CANARY;
			const server = serveProvider(() => Response.json({ error: { type: "invalid_request_error" } }, { status: 400 }));
			servers.push(server);
			setProviderBaseUrlForTests("anthropic", server.url);
			const model = resolveModel("anthropic:claude-sonnet-5-5");
			const stream = makeStreamFn(daemon, {
				provider: "anthropic",
				model: "anthropic:claude-sonnet-5-5",
				networkAllow: [],
				maxRetryDelayMs: 0,
			});
			await (await stream(model, { systemPrompt: ["original"], ...userContext("hi") }, {})).result();
			const request = server.seen[0];
			expect(request).toBeDefined();
			const payload = JSON.parse(request?.body ?? "{}") as {
				system?: { text: string }[] | string;
			};
			const system = typeof payload.system === "string" ? [{ text: payload.system }] : (payload.system ?? []);
			if (kind === "api_key") {
				expect(request?.headers["x-api-key"]).toBe(CANARY);
				expect(request?.headers.authorization).toBeUndefined();
				expect(system.map(block => block.text)).not.toContain(CLAUDE_CODE_IDENTITY);
				expect(request?.headers["user-agent"] ?? "").not.toStartWith("claude-cli/");
			} else {
				expect(request?.headers.authorization).toBe(`Bearer ${CANARY}`);
				expect(request?.headers["x-api-key"]).toBeUndefined();
				// pi-ai mirrors Claude Code: a billing-header block may precede the identity.
				const texts = system.map(block => block.text);
				expect(texts.slice(0, 2)).toContain(CLAUDE_CODE_IDENTITY);
				expect(texts.indexOf(CLAUDE_CODE_IDENTITY)).toBeLessThan(texts.findIndex(text => text.includes("original")));
				const betas = (request?.headers["anthropic-beta"] ?? "").split(",");
				// pi-ai adds claude-code-20250219 only to tool-bearing (agentic) requests, like Claude Code.
				expect(betas).toContain("oauth-2025-04-20");
				expect(request?.headers["user-agent"]).toMatch(/^claude-cli\/\d+\.\d+\.\d+ \(external, cli\)$/);
				expect(request?.headers["x-app"]).toBe("cli");
			}
			expect(server.seen.every(seen => seen.body.includes(CANARY) === false)).toBe(true);
		});
	}

	it("test_claude_code_version_override_reaches_the_wire", async () => {
		const previous = process.env.PI_AI_CLAUDE_CODE_VERSION;
		try {
			const daemon = new FakeCredentialDaemon(1);
			daemon.kind = "claude_code";
			const server = serveProvider(() => Response.json({ error: { type: "invalid_request_error" } }, { status: 400 }));
			servers.push(server);
			setProviderBaseUrlForTests("anthropic", server.url);
			const model = resolveModel("anthropic:claude-sonnet-5-5");
			const stream = makeStreamFn(daemon, {
				provider: "anthropic",
				model: "anthropic:claude-sonnet-5-5",
				networkAllow: [],
				claudeCodeVersion: "9.8.7",
				maxRetryDelayMs: 0,
			});
			await (await stream(model, userContext("hi"), {})).result();
			expect(server.seen[0]?.headers["user-agent"]).toBe("claude-cli/9.8.7 (external, cli)");
		} finally {
			if (previous === undefined) delete process.env.PI_AI_CLAUDE_CODE_VERSION;
			else process.env.PI_AI_CLAUDE_CODE_VERSION = previous;
		}
	});
});
