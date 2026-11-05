import { afterEach, describe, expect, it } from "bun:test";
import type { CredentialErrorRules, CredentialReply } from "../../src/protocol/types";
import type { CredentialRpcPort } from "../../src/providers/credential-rpc";
import { AuthUnavailable } from "../../src/providers/errors";
import { resolveModel, setProviderBaseUrlForTests } from "../../src/providers/models";
import { classifyRelayResponse, RELAY_NOTICES } from "../../src/providers/outcomes";
import { CredentialAttempt, makeCredentialFetch } from "../../src/providers/request-auth";
import { CredentialSecret } from "../../src/providers/secret";
import { makeStreamFn } from "../../src/providers/stream";
import {
	FakeCredentialDaemon,
	type ProviderServer,
	serveProvider,
	textOf,
	userContext,
} from "./fake-credential-daemon";

const RULES: CredentialErrorRules = {
	hint_header: "x-example-hint",
	rules: [
		{
			status: 429,
			hint: "unsupported-client",
			outcome: "unsupported_client",
			message: "Example relay wants another client",
		},
		{ status: 502, body_contains: "sign in again", outcome: "sign_in" },
		{ status: 503, outcome: "capacity" },
	],
};

const servers: ProviderServer[] = [];

afterEach(() => {
	for (const server of servers.splice(0)) server.stop();
});

function serve(handler: Parameters<typeof serveProvider>[0]): ProviderServer {
	const server = serveProvider(handler);
	servers.push(server);
	return server;
}

/** The fake daemon issuing plugin-source leases with `auth` overrides. */
function plugin(daemon: FakeCredentialDaemon, auth: Record<string, unknown>): CredentialRpcPort {
	return {
		credentialRequest: async (args, signal) => {
			const reply: CredentialReply = await daemon.credentialRequest(args, signal);
			if (reply.type === "credential_response") {
				reply.account.source = "plugin";
				reply.account.account_id = "example-relay";
				Object.assign(reply.auth, auth);
			}
			return reply;
		},
		// A one-account daemon stops instead of rotating into an empty pool.
		credentialReport: async (args, options) => {
			const reply = await daemon.credentialReport(args, options);
			if (reply?.type === "credential_reported" && reply.action === "rotate") reply.action = "stop";
			return reply;
		},
	};
}

function anthropicReply(): Response {
	const events = [
		{
			type: "message_start",
			message: {
				id: "msg_test",
				type: "message",
				role: "assistant",
				model: "claude-haiku-5-5",
				content: [],
				stop_reason: null,
				stop_sequence: null,
				usage: { input_tokens: 1, output_tokens: 1 },
			},
		},
		{ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
		{ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "ok" } },
		{ type: "content_block_stop", index: 0 },
		{ type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 1 } },
		{ type: "message_stop" },
	];
	return new Response(events.map(data => `event: ${data.type}\ndata: ${JSON.stringify(data)}\n\n`).join(""), {
		headers: { "content-type": "text/event-stream" },
	});
}

function attempt(provider: string, source: "plugin" | "vault", kind: "claude_code" | "codex_cli"): CredentialAttempt {
	return new CredentialAttempt(provider, source, kind, new CredentialSecret("alice"), undefined, {
		headers: { "x-example-task": "openside:s1", "x-api-key": "ignored" },
		errorRules: RULES,
	});
}

describe("plugin relay classification", () => {
	const classify = (status: number, headers: Record<string, string>, body: unknown, rules = RULES) =>
		classifyRelayResponse(status, new Headers(headers), body, rules);

	it("test_hint_rule_stops_with_manifest_message", () => {
		const result = classify(429, { "x-example-hint": "Unsupported-Client" }, {});
		expect(result.outcome.kind).toBe("policy");
		expect(result.notice).toBe("Example relay wants another client");
	});

	it("test_body_rule_maps_sign_in_to_auth", () => {
		const result = classify(502, {}, "upstream says: Sign in again");
		expect(result.outcome.kind).toBe("auth");
		expect(result.notice).toBe(RELAY_NOTICES.sign_in);
	});

	it("test_status_rule_maps_capacity_with_retry_delay", () => {
		const result = classify(503, { "retry-after": "7" }, {});
		expect(result.outcome.kind).toBe("rate_limit");
		expect(result.outcome.retryAfter).toBe(7);
		expect(result.notice).toBe(RELAY_NOTICES.capacity);
	});

	it("test_unmatched_answers_use_generic_classification", () => {
		expect(classify(429, { "x-example-hint": "other" }, {}).outcome.kind).toBe("rate_limit");
		expect(classify(429, {}, {}).notice).toBe(RELAY_NOTICES.rate_limit);
		expect(classify(502, {}, "bad gateway").outcome.kind).toBe("transient");
		expect(classify(401, {}, {}, undefined).outcome.kind).toBe("auth");
	});
});

describe("plugin relay requests", () => {
	it("test_codex_lease_needs_no_account_id_and_carries_session_headers", async () => {
		const server = serve(() => Response.json({}));
		const lease = attempt("openai-codex", "plugin", "codex_cli");
		await makeCredentialFetch(
			lease,
			server.url,
			fetch,
		)(`${server.url}/codex/responses`, {
			method: "POST",
			headers: { "x-api-key": "pi-ai" },
			body: "{}",
		});
		const [seen] = server.seen;
		expect(seen?.headers.authorization).toBe("Bearer alice");
		expect(seen?.headers["x-example-task"]).toBe("openside:s1");
		expect(seen?.headers["x-api-key"]).toBeUndefined();
		expect(seen?.headers["chatgpt-account-id"]).toBeUndefined();
	});

	it("test_first_party_codex_without_account_id_is_refused", async () => {
		const server = serve(() => Response.json({}));
		const lease = attempt("openai-codex", "vault", "codex_cli");
		await expect(
			makeCredentialFetch(lease, server.url, fetch)(`${server.url}/codex/responses`, {}),
		).rejects.toBeInstanceOf(AuthUnavailable);
		expect(server.seen).toHaveLength(0);
	});

	it("test_claude_code_beta_is_added_for_toolless_requests", async () => {
		const server = serve(() => Response.json({}));
		const lease = attempt("anthropic", "plugin", "claude_code");
		await makeCredentialFetch(
			lease,
			server.url,
			fetch,
		)(`${server.url}/v1/messages`, {
			method: "POST",
			headers: { "anthropic-beta": "oauth-2025-04-20" },
			body: "{}",
		});
		expect(server.seen[0]?.headers["anthropic-beta"]).toBe("claude-code-20250219,oauth-2025-04-20");
	});

	it("test_refusal_sets_outcome_and_notice", async () => {
		const server = serve(
			() => new Response("no", { status: 429, headers: { "x-example-hint": "unsupported-client" } }),
		);
		const lease = attempt("anthropic", "plugin", "claude_code");
		await makeCredentialFetch(lease, server.url, fetch)(`${server.url}/v1/messages`, { method: "POST", body: "{}" });
		expect(lease.outcome.kind).toBe("policy");
		expect(lease.notice).toBe("Example relay wants another client");
	});
});

describe("plugin relay streams", () => {
	it("test_lease_base_url_routes_away_from_the_provider_default", async () => {
		const fallback = serve(() => anthropicReply());
		const relay = serve(() => anthropicReply());
		setProviderBaseUrlForTests("anthropic", fallback.url);
		const daemon = new FakeCredentialDaemon(1);
		daemon.kind = "claude_code";
		daemon.secretFor = () => "alice";
		const id = "anthropic:claude-haiku-5-5";
		const stream = makeStreamFn(
			plugin(daemon, { base_url: relay.url, headers: { "x-example-task": "openside:s1" }, error_rules: RULES }),
			{ provider: "anthropic", model: id, networkAllow: [] },
		);
		const message = await (await stream(resolveModel(id), userContext("reply ok"), {})).result();
		expect(message.stopReason).toBe("stop");
		expect(textOf(message)).toBe("ok");
		expect(fallback.seen).toHaveLength(0);
		expect(relay.seen).toHaveLength(1);
		expect(relay.seen[0]?.headers.authorization).toBe("Bearer alice");
		expect(relay.seen[0]?.headers["x-example-task"]).toBe("openside:s1");
		expect(relay.seen[0]?.headers["anthropic-beta"]).toContain("claude-code-20250219");
		expect(daemon.outcomes()).toEqual(["success"]);
	});

	it("test_relay_refusal_surfaces_the_manifest_message", async () => {
		const relay = serve(() => new Response("{}", { status: 429, headers: { "x-example-hint": "unsupported-client" } }));
		const daemon = new FakeCredentialDaemon(1);
		daemon.kind = "claude_code";
		const id = "anthropic:claude-haiku-5-5";
		const stream = makeStreamFn(plugin(daemon, { base_url: relay.url, error_rules: RULES }), {
			provider: "anthropic",
			model: id,
			networkAllow: [],
		});
		const error = await Promise.resolve(stream(resolveModel(id), userContext("reply ok"), {})).then(
			() => undefined,
			(error: unknown) => error,
		);
		expect(error).toBeInstanceOf(AuthUnavailable);
		expect((error as AuthUnavailable).safeErrors).toEqual(["Example relay wants another client"]);
		expect(relay.seen).toHaveLength(1);
		expect(daemon.outcomes()).toEqual(["policy"]);
	});

	for (const [field, value] of [
		["base_url", "http://user:pw@127.0.0.1:1"],
		["base_url", "http://127.0.0.1:1/?q=1"],
		["headers", { authorization: "x" }],
		["headers", { "x-example-task": "line\nbreak" }],
		["error_rules", { rules: [{ status: 200, outcome: "capacity" }] }],
		["error_rules", { rules: [{ status: 429, outcome: "retry" }] }],
		["error_rules", { rules: [], extra: true }],
		["kind", "password"],
		["upstream_account_id", 42],
		["upstream_account_id", ""],
		["expires_at_ms", -1],
		["expires_at_ms", 1.5],
		["expires_at_ms", "soon"],
	] as const) {
		it(`test_invalid_auth_fields_reject_the_lease[${field}:${JSON.stringify(value)}]`, async () => {
			const relay = serve(() => anthropicReply());
			const daemon = new FakeCredentialDaemon(1);
			daemon.kind = "claude_code";
			const id = "anthropic:claude-haiku-5-5";
			const stream = makeStreamFn(plugin(daemon, { base_url: relay.url, [field]: value }), {
				provider: "anthropic",
				model: id,
				networkAllow: [],
			});
			const error = await Promise.resolve(stream(resolveModel(id), userContext("hi"), {})).then(
				() => undefined,
				(error: unknown) => error,
			);
			expect(error).toBeInstanceOf(AuthUnavailable);
			expect(relay.seen).toHaveLength(0);
		});
	}
});
