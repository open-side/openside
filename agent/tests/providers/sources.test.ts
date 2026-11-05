import { afterEach, describe, expect, it } from "bun:test";
import { AuthUnavailable } from "../../src/providers/errors";
import { resolveModel, setProviderBaseUrlForTests } from "../../src/providers/models";
import { makeStreamFn } from "../../src/providers/stream";
import { FakeCredentialDaemon, type ProviderServer, serveProvider, userContext } from "./fake-credential-daemon";

const CANARY = "synthetic-build-secret-canary";
const ENV_NAMES = [
	"OPENAI_API_KEY",
	"ANTHROPIC_API_KEY",
	"OPENAI_ADMIN_KEY",
	"OPENAI_WEBHOOK_SECRET",
	"OPENAI_ORG_ID",
	"OPENAI_PROJECT_ID",
	"ANTHROPIC_AUTH_TOKEN",
	"ANTHROPIC_WEBHOOK_SIGNING_KEY",
];
const saved = Object.fromEntries(ENV_NAMES.map(name => [name, process.env[name]]));
let server: ProviderServer | undefined;

afterEach(() => {
	server?.stop();
	server = undefined;
	for (const name of ENV_NAMES) {
		const value = saved[name];
		if (value === undefined) delete process.env[name];
		else process.env[name] = value;
	}
});

describe("sources", () => {
	for (const identifier of [
		"anthropic:claude-sonnet-5-5",
		"openai:gpt-6.1-sol",
		"google:gemini-3.8-flash",
		"openai-codex:gpt-6.1-sol",
		"github-copilot:gpt-5",
	]) {
		it(`test_model_build_uses_placeholder_sdk_clients_and_no_source_reads[${identifier}]`, () => {
			for (const name of ENV_NAMES) process.env[name] = CANARY;
			const daemon = new FakeCredentialDaemon();
			const model = resolveModel(identifier);
			makeStreamFn(daemon, {
				provider: identifier.split(":")[0] as string,
				model: identifier,
				networkAllow: [],
			});
			expect(model.id).toBe(identifier.slice(identifier.indexOf(":") + 1));
			expect(`${JSON.stringify(model)} ${Bun.inspect(model)}`).not.toContain(CANARY);
			expect(daemon.requests).toEqual([]);
		});
	}

	it("test_only_explicit_fallback_models_are_constructed", async () => {
		const daemon = new FakeCredentialDaemon();
		server = serveProvider(() => Response.json({ error: "temporary failure" }, { status: 400 }));
		setProviderBaseUrlForTests("anthropic", server.url);
		setProviderBaseUrlForTests("openai", `${server.url}/v1`);
		const stream = makeStreamFn(daemon, {
			provider: "anthropic",
			model: "anthropic:claude-sonnet-5-5",
			networkAllow: [],
			fallback: ["openai:gpt-6.1-sol"],
			maxRetryDelayMs: 0,
		});
		const result = await (await stream(resolveModel("anthropic:claude-sonnet-5-5"), userContext("hi"), {})).result();
		expect(result.stopReason).toBe("error");
		expect(daemon.requests.map(request => request.provider)).toEqual(["anthropic", "openai"]);

		const denied = new FakeCredentialDaemon(0);
		const refused = makeStreamFn(denied, {
			provider: "anthropic",
			model: "anthropic:claude-sonnet-5-5",
			networkAllow: [],
			fallback: ["openai:gpt-6.1-sol"],
		});
		await expect(
			Promise.resolve(refused(resolveModel("anthropic:claude-sonnet-5-5"), userContext("hi"), {})),
		).rejects.toBeInstanceOf(AuthUnavailable);
		expect(denied.requests.map(request => request.provider)).toEqual(["anthropic"]);
	});

	it("test_invalid_identifier_fails_closed", () => {
		expect(() => resolveModel("invalid")).toThrow(AuthUnavailable);
		expect(() => resolveModel("unsupported:model")).toThrow(AuthUnavailable);
	});
});
