import { afterEach, describe, expect, it } from "bun:test";
import { AuthUnavailable } from "../../src/providers/errors";
import { resolveModel, setProviderBaseUrlForTests } from "../../src/providers/models";
import { makeStreamFn } from "../../src/providers/stream";
import {
	chatStream,
	FakeCredentialDaemon,
	type ProviderServer,
	serveProvider,
	textOf,
	userContext,
} from "./fake-credential-daemon";

let server: ProviderServer | undefined;

afterEach(() => server?.stop());

describe("edges", () => {
	for (const authFailure of [true, false]) {
		it(`test_explicit_model_fallback_never_catches_auth_unavailable[${authFailure}]`, async () => {
			const daemon = new FakeCredentialDaemon(authFailure ? 0 : 1);
			const called: string[] = [];
			server = serveProvider(request => {
				const model = (JSON.parse(request.body) as { model: string }).model;
				called.push(model === "gpt-5" ? "primary" : "secondary");
				// 400 is a non-retryable transient failure: no account rotation, no transport retry.
				return model === "gpt-5" ? Response.json({ error: "temporary failure" }, { status: 400 }) : chatStream("pong");
			});
			setProviderBaseUrlForTests("openai-chat", `${server.url}/v1`);
			const stream = makeStreamFn(daemon, {
				provider: "openai-chat",
				model: "openai-chat:gpt-5",
				networkAllow: [],
				fallback: ["openai-chat:gpt-5-mini"],
			});
			const run = Promise.resolve(stream(resolveModel("openai-chat:gpt-5"), userContext("Reply pong."), {}));
			if (authFailure) {
				await expect(run).rejects.toBeInstanceOf(AuthUnavailable);
				expect(daemon.requests.map(request => request.model)).toEqual(["openai-chat:gpt-5"]);
				expect(called).toEqual([]);
			} else {
				expect(textOf(await (await run).result())).toBe("pong");
				expect(called).toEqual(["primary", "secondary"]);
				expect(daemon.requests.map(request => request.model)).toEqual(["openai-chat:gpt-5", "openai-chat:gpt-5-mini"]);
			}
		});
	}
});
