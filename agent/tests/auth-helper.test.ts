import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { createHash } from "node:crypto";
import { networkInterfaces } from "node:os";
import { join } from "node:path";
import { getProviderDefinition, type OAuthController, type OAuthCredentials } from "@oh-my-pi/pi-ai";
import { LoginCancelledError, OAuthError } from "@oh-my-pi/pi-ai/error";
import { authLogin, type LoginDefinition } from "../src/auth-login";
import { authRefresh } from "../src/auth-refresh";
import {
	type HelperIo,
	type HelperOutbound,
	LOGIN_ORIGINS,
	MAX_HELPER_LINE_BYTES,
	parseInbound,
	REFRESH_ORIGINS,
	readLines,
} from "../src/protocol/auth-helper";

const ACCESS = "synthetic-helper-access-canary";
const REFRESH = "synthetic-helper-refresh-canary";

/** Daemon side of the pipe: queued inbound lines and the helper's outbound messages. */
class FakeIo implements HelperIo {
	readonly out: HelperOutbound[] = [];
	readonly #queue: IteratorResult<string>[] = [];
	#waiting: ((item: IteratorResult<string>) => void) | undefined;
	#watchers: (() => void)[] = [];

	readonly lines: AsyncIterator<string> = {
		next: () => {
			const item = this.#queue.shift();
			if (item) return Promise.resolve(item);
			const { promise, resolve } = Promise.withResolvers<IteratorResult<string>>();
			this.#waiting = resolve;
			return promise;
		},
	};

	constructor(...lines: (string | object)[]) {
		for (const line of lines) this.push(line);
	}

	push(line: string | object): void {
		this.#deliver({ done: false, value: typeof line === "string" ? line : JSON.stringify(line) });
	}

	end(): void {
		this.#deliver({ done: true, value: undefined });
	}

	#deliver(item: IteratorResult<string>): void {
		const waiting = this.#waiting;
		this.#waiting = undefined;
		if (waiting) waiting(item);
		else this.#queue.push(item);
	}

	write(message: HelperOutbound): void {
		this.out.push(message);
		for (const watcher of this.#watchers.splice(0)) watcher();
	}

	async until(type: HelperOutbound["type"]): Promise<void> {
		while (!this.out.some(message => message.type === type)) {
			const { promise, resolve } = Promise.withResolvers<void>();
			this.#watchers.push(resolve);
			await promise;
		}
	}
}

function grant(overrides: Partial<OAuthCredentials> = {}): OAuthCredentials {
	return { access: ACCESS, refresh: REFRESH, expires: 1_900_000_000_000, ...overrides };
}

interface Fake {
	deps: { definition(id: string): LoginDefinition | undefined; installEgress(allow: readonly string[]): void };
	egress: (readonly string[])[];
	asked: string[];
}

function fakeProvider(login: (ctrl: OAuthController) => Promise<OAuthCredentials | string>): Fake {
	const egress: (readonly string[])[] = [];
	const asked: string[] = [];
	return {
		egress,
		asked,
		deps: {
			definition: id => {
				asked.push(id);
				return { login };
			},
			installEgress: allow => {
				egress.push(allow);
			},
		},
	};
}

function hangUntilAborted(ctrl: OAuthController): Promise<never> {
	const { promise, reject } = Promise.withResolvers<never>();
	ctrl.signal?.addEventListener("abort", () => reject(new LoginCancelledError()), { once: true });
	return promise;
}

function stream(...chunks: (string | Uint8Array)[]): ReadableStream<Uint8Array> {
	return new ReadableStream({
		start(controller) {
			for (const chunk of chunks) controller.enqueue(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
			controller.close();
		},
	});
}

async function collect(lines: AsyncIterable<string>): Promise<string[]> {
	const out: string[] = [];
	for await (const line of lines) out.push(line);
	return out;
}

const restores: (() => void)[] = [];
afterEach(() => {
	for (const restore of restores.splice(0)) restore();
});

describe("auth helper protocol", () => {
	it("test_line_validation_is_strict", async () => {
		expect(parseInbound('{"type":"login","provider":"anthropic"}')).toEqual({ type: "login", provider: "anthropic" });
		for (const bad of [
			'{"type":"bogus"}',
			'{"type":"login","provider":"anthropic","extra":1}',
			'{"type":"login","provider":"Anthropic"}',
			'{"type":"manual_input","value":""}',
			'{"type":"cancel","reason":"x"}',
			'{"type":"refresh","provider":"anthropic","credentials":{"access":"a","refresh":"r","expires":1,"email":"x"}}',
			"[]",
			"not json",
		]) {
			expect(parseInbound(bad)).toBeUndefined();
		}
		expect(await collect(readLines(stream('{"a":', '1}\n{"b":2}\n')))).toEqual(['{"a":1}', '{"b":2}']);
		await expect(collect(readLines(stream(`${"x".repeat(MAX_HELPER_LINE_BYTES + 1)}\n`)))).rejects.toThrow();
		await expect(collect(readLines(stream("x".repeat(MAX_HELPER_LINE_BYTES), "y")))).rejects.toThrow();
		await expect(collect(readLines(stream('{"type":"cancel"}')))).rejects.toThrow();
		await expect(collect(readLines(stream(new Uint8Array([0xff, 0x0a]))))).rejects.toThrow();
	});

	it("test_bad_first_line_and_oversize_line_fail_without_login", async () => {
		for (const first of ['{"type":"bogus"}', '{"type":"manual_input","value":"code"}', "{"]) {
			const fake = fakeProvider(async () => grant());
			const io = new FakeIo(first);
			expect(await authLogin(io, fake.deps)).toBe(1);
			expect(io.out).toEqual([{ type: "error", code: "failed" }]);
			expect(fake.asked).toEqual([]);
		}
		const fake = fakeProvider(async () => grant());
		const oversize: HelperIo = {
			lines: readLines(stream(`${"x".repeat(MAX_HELPER_LINE_BYTES + 1)}\n`)),
			write: message => out.push(message),
		};
		const out: HelperOutbound[] = [];
		expect(await authLogin(oversize, fake.deps)).toBe(1);
		expect(out).toEqual([{ type: "error", code: "failed" }]);
	});

	it("test_unknown_provider_is_unsupported", async () => {
		for (const provider of ["cursor", "constructor", "openai"]) {
			const fake = fakeProvider(async () => grant());
			const io = new FakeIo({ type: "login", provider });
			expect(await authLogin(io, fake.deps)).toBe(1);
			expect(io.out).toEqual([{ type: "error", code: "unsupported" }]);
			expect(fake.egress).toEqual([]);
		}
	});

	it("test_auth_event_extracts_user_code_and_strips_identity", async () => {
		let prompted: string | undefined;
		const fake = fakeProvider(async ctrl => {
			prompted = await ctrl.onPrompt?.({ message: "GitHub Enterprise URL/domain", allowEmpty: true });
			ctrl.onAuth?.({ url: "https://github.com/login/device", instructions: "Enter code: ABCD-1234" });
			ctrl.onProgress?.("Enabling models...");
			return grant({
				email: "person@example.test",
				orgId: "org-1",
				orgName: "Org",
				apiEndpoint: "https://api.individual.githubcopilot.com",
				authorizedAt: 5,
			});
		});
		const io = new FakeIo({ type: "login", provider: "github-copilot" });
		expect(await authLogin(io, fake.deps)).toBe(0);
		expect(prompted).toBe("");
		expect(fake.egress).toEqual([LOGIN_ORIGINS["github-copilot"] ?? []]);
		expect(io.out).toEqual([
			{
				type: "auth",
				url: "https://github.com/login/device",
				instructions: "Enter code: ABCD-1234",
				user_code: "ABCD-1234",
			},
			{
				type: "credentials",
				credentials: {
					access: ACCESS,
					refresh: REFRESH,
					expires: 1_900_000_000_000,
					api_endpoint: "https://api.individual.githubcopilot.com",
				},
			},
		]);
	});

	it("test_browser_flow_maps_progress_and_snake_case_fields", async () => {
		const fake = fakeProvider(async ctrl => {
			ctrl.onAuth?.({
				url: "https://accounts.google.com/o/oauth2/v2/auth?x=1",
				launchUrl: "http://127.0.0.1:8085/launch",
			});
			ctrl.onProgress?.("Waiting for browser authentication...");
			ctrl.onProgress?.("Exchanging authorization code for tokens...");
			return grant({ projectId: "proj-1", accountId: "acct-1", enterpriseUrl: "ghe.example" });
		});
		const io = new FakeIo({ type: "login", provider: "google-gemini-cli" });
		expect(await authLogin(io, fake.deps)).toBe(0);
		expect(io.out).toEqual([
			{
				type: "auth",
				url: "https://accounts.google.com/o/oauth2/v2/auth?x=1",
				launch_url: "http://127.0.0.1:8085/launch",
			},
			{ type: "progress", stage: "exchanging" },
			{
				type: "credentials",
				credentials: {
					access: ACCESS,
					refresh: REFRESH,
					expires: 1_900_000_000_000,
					project_id: "proj-1",
					account_id: "acct-1",
					enterprise_url: "ghe.example",
				},
			},
		]);
	});

	it("test_manual_input_round_trip", async () => {
		const fake = fakeProvider(async ctrl => {
			ctrl.onAuth?.({ url: "https://claude.ai/oauth/authorize?x=1" });
			const code = await ctrl.onManualCodeInput?.(new AbortController().signal);
			return grant({ access: `${ACCESS}-${code}` });
		});
		const io = new FakeIo({ type: "login", provider: "anthropic" });
		const done = authLogin(io, fake.deps);
		await io.until("manual_input_wanted");
		io.push({ type: "manual_input", value: "pasted-code" });
		expect(await done).toBe(0);
		expect(io.out.map(message => message.type)).toEqual(["auth", "manual_input_wanted", "credentials"]);
		expect(io.out.at(-1)).toMatchObject({ credentials: { access: `${ACCESS}-pasted-code` } });
	});

	it("test_manual_input_before_request_is_a_protocol_error", async () => {
		const fake = fakeProvider(hangUntilAborted);
		const io = new FakeIo({ type: "login", provider: "anthropic" }, { type: "manual_input", value: "early" });
		expect(await authLogin(io, fake.deps)).toBe(1);
		expect(io.out).toEqual([{ type: "error", code: "failed" }]);
	});

	it("test_manual_request_rejects_on_cancel", async () => {
		let manual: Promise<string> | undefined;
		const fake = fakeProvider(async ctrl => {
			manual = ctrl.onManualCodeInput?.(new AbortController().signal) ?? Promise.reject(new Error("no manual input"));
			return manual.then(() => grant());
		});
		const io = new FakeIo({ type: "login", provider: "anthropic" });
		const done = authLogin(io, fake.deps);
		await io.until("manual_input_wanted");
		io.push({ type: "cancel" });
		expect(await done).toBe(1);
		expect(io.out.at(-1)).toEqual({ type: "error", code: "cancelled" });
		await expect(manual).rejects.toBeInstanceOf(LoginCancelledError);
	});

	it("test_cancel_and_stdin_eof_cancel_even_a_stuck_flow", async () => {
		const aborted: boolean[] = [];
		const stuck = fakeProvider(ctrl => {
			ctrl.signal?.addEventListener("abort", () => aborted.push(true));
			return new Promise(() => undefined);
		});
		const cancelled = new FakeIo({ type: "login", provider: "openai-codex" }, { type: "cancel" });
		expect(await authLogin(cancelled, stuck.deps)).toBe(1);
		expect(cancelled.out).toEqual([{ type: "error", code: "cancelled" }]);
		expect(aborted).toEqual([true]);

		const eof = new FakeIo({ type: "login", provider: "openai-codex-device" });
		eof.end();
		expect(await authLogin(eof, fakeProvider(hangUntilAborted).deps)).toBe(1);
		expect(eof.out).toEqual([{ type: "error", code: "cancelled" }]);

		const empty = new FakeIo();
		empty.end();
		expect(await authLogin(empty, fakeProvider(async () => grant()).deps)).toBe(1);
		expect(empty.out).toEqual([{ type: "error", code: "cancelled" }]);
	});

	it("test_login_errors_map_to_fixed_codes", async () => {
		const busyCause = Object.assign(new Error("listen failed"), { code: "EADDRINUSE" });
		const cases: [unknown, string][] = [
			[new Error(`OAuth callback port 1455 is in use. ${ACCESS}`, { cause: busyCause }), "callback_busy"],
			[new OAuthError(`Timed out waiting for ${ACCESS}`, { kind: "timeout" }), "timeout"],
			[new LoginCancelledError("OAuth callback cancelled: TimeoutError: The operation timed out."), "timeout"],
			[new LoginCancelledError(), "cancelled"],
			[
				new OAuthError(`anthropic token exchange failed: 400 {"error":"invalid_grant","x":"${ACCESS}"}`, {
					kind: "token-exchange",
					status: 400,
				}),
				"invalid_grant",
			],
			[new Error(`provider exploded ${ACCESS}`), "failed"],
		];
		for (const [error, code] of cases) {
			const io = new FakeIo({ type: "login", provider: "openai-codex" });
			expect(
				await authLogin(
					io,
					fakeProvider(async () => {
						throw error;
					}).deps,
				),
			).toBe(1);
			expect(io.out).toEqual([{ type: "error", code } as HelperOutbound]);
		}
	});

	it("test_unsupported_prompts_and_api_key_results", async () => {
		const prompt = new FakeIo({ type: "login", provider: "anthropic" });
		const asking = fakeProvider(async ctrl => {
			await ctrl.onPrompt?.({ message: "Paste your secret" });
			return grant();
		});
		expect(await authLogin(prompt, asking.deps)).toBe(1);
		expect(prompt.out).toEqual([{ type: "error", code: "unsupported" }]);

		const browser = new FakeIo({ type: "login", provider: "anthropic" });
		const session = fakeProvider(async ctrl => {
			await ctrl.onBrowserSession?.({ url: "https://example.test", cookieNames: ["sid"] });
			return grant();
		});
		expect(await authLogin(browser, session.deps)).toBe(1);
		expect(browser.out).toEqual([{ type: "error", code: "unsupported" }]);

		const apiKey = new FakeIo({ type: "login", provider: "anthropic" });
		expect(await authLogin(apiKey, fakeProvider(async () => ACCESS).deps)).toBe(1);
		expect(apiKey.out).toEqual([{ type: "error", code: "failed" }]);
	});

	it("test_no_token_text_reaches_stderr", async () => {
		const written: string[] = [];
		const stderr = spyOn(process.stderr, "write").mockImplementation(((chunk: unknown) => {
			written.push(String(chunk));
			return true;
		}) as typeof process.stderr.write);
		const consoleError = spyOn(console, "error").mockImplementation((...args: unknown[]) => {
			written.push(args.map(String).join(" "));
		});
		restores.push(
			() => stderr.mockRestore(),
			() => consoleError.mockRestore(),
		);
		const ok = new FakeIo({ type: "login", provider: "anthropic" });
		expect(await authLogin(ok, fakeProvider(async () => grant()).deps)).toBe(0);
		const failing = new FakeIo({ type: "login", provider: "anthropic" });
		const thrower = fakeProvider(async () => {
			throw new OAuthError(`token exchange failed: 500 ${ACCESS} ${REFRESH}`);
		});
		expect(await authLogin(failing, thrower.deps)).toBe(1);
		const refresh = new FakeIo({
			type: "refresh",
			provider: "anthropic",
			credentials: { access: ACCESS, refresh: REFRESH, expires: 1 },
		});
		expect(
			await authRefresh(refresh, {
				refresh: async () => {
					throw new Error(`refresh failed ${ACCESS} ${REFRESH}`);
				},
				installEgress: () => undefined,
			}),
		).toBe(1);
		expect(written.join("")).not.toContain(ACCESS);
		expect(written.join("")).not.toContain(REFRESH);
		expect(JSON.stringify([failing.out, refresh.out])).not.toContain(ACCESS);
	});
});

describe("auth refresh helper", () => {
	const request = (provider: string, credentials: object) => new FakeIo({ type: "refresh", provider, credentials });

	it("test_refresh_preserves_omitted_fields_and_installs_provider_egress", async () => {
		const seen: OAuthCredentials[] = [];
		const egress: (readonly string[])[] = [];
		const io = request("google-gemini-cli", {
			access: "old-access",
			refresh: REFRESH,
			expires: 10,
			project_id: "proj-1",
		});
		const code = await authRefresh(io, {
			refresh: async (provider, credentials) => {
				expect(provider).toBe("google-gemini-cli");
				seen.push(credentials);
				return { access: ACCESS, refresh: "", expires: 2_000, email: "person@example.test" };
			},
			installEgress: allow => {
				egress.push(allow);
			},
		});
		expect(code).toBe(0);
		expect(seen[0]).toMatchObject({ access: "old-access", refresh: REFRESH, projectId: "proj-1" });
		expect(egress).toEqual([REFRESH_ORIGINS["google-gemini-cli"] ?? []]);
		expect(io.out).toEqual([
			{ type: "credentials", credentials: { access: ACCESS, refresh: REFRESH, expires: 2_000, project_id: "proj-1" } },
		]);
	});

	it("test_refresh_errors_are_invalid_grant_or_failed", async () => {
		const cases: [unknown, string][] = [
			[
				new OAuthError(`openai-codex token refresh failed: 400 {"error":"invalid_grant"} ${ACCESS}`, {
					kind: "token-refresh",
					status: 400,
				}),
				"invalid_grant",
			],
			[new OAuthError("openai-codex token refresh failed: 503 unavailable", { kind: "token-refresh" }), "failed"],
			[new OAuthError("Timed out waiting for https://auth.openai.com/oauth/token", { kind: "timeout" }), "failed"],
		];
		for (const [error, code] of cases) {
			const io = request("openai-codex", { access: "a", refresh: "r", expires: 1, account_id: "acct" });
			const result = await authRefresh(io, {
				refresh: async () => {
					throw error;
				},
				installEgress: () => undefined,
			});
			expect(result).toBe(1);
			expect(io.out).toEqual([{ type: "error", code } as HelperOutbound]);
		}
	});

	it("test_refresh_rejects_unknown_providers_and_bad_requests", async () => {
		const never = {
			refresh: async (): Promise<OAuthCredentials> => {
				throw new Error("must not refresh");
			},
			installEgress: () => {
				throw new Error("must not install egress");
			},
		};
		for (const io of [
			request("openai-codex-device", { access: "a", refresh: "r", expires: 1 }),
			request("constructor", { access: "a", refresh: "r", expires: 1 }),
			request("anthropic", { access: "", refresh: "r", expires: 1 }),
			new FakeIo({ type: "login", provider: "anthropic" }),
		]) {
			expect(await authRefresh(io, never)).toBe(1);
			expect(io.out).toEqual([{ type: "error", code: "failed" }]);
		}
	});
});

/** Whether a fresh process (no egress guard) can open a TCP connection to `hostname:port`. */
async function reachable(hostname: string, port: number): Promise<boolean> {
	const probe = `try { const s = await Bun.connect({ hostname: ${JSON.stringify(hostname)}, port: ${port}, socket: { data() {} } }); s.end(); console.log("open"); } catch { console.log("closed"); }`;
	const child = Bun.spawn([process.execPath, "-e", probe], { stdout: "pipe", stderr: "ignore" });
	return (await new Response(child.stdout).text()).trim() === "open";
}

/** This host's own addresses outside loopback (link-local IPv6 needs a scope id and is skipped). */
function interfaceAddresses(): string[] {
	return Object.values(networkInterfaces())
		.flat()
		.filter(entry => entry !== undefined && !entry.internal && !entry.address.startsWith("fe80"))
		.map(entry => entry?.address ?? "");
}

function formFields(body: unknown): Record<string, string> {
	const text = typeof body === "string" ? body : body instanceof URLSearchParams ? body.toString() : "";
	try {
		return JSON.parse(text) as Record<string, string>;
	} catch {
		return Object.fromEntries(new URLSearchParams(text));
	}
}

describe("auth helper callback listener", () => {
	// pi-ai's real login and callback server; only the token exchange is stubbed, so nothing leaves the host.
	for (const [provider, redirectHost, pkce] of [
		["anthropic", "localhost", true],
		["openai-codex", "localhost", true],
		["google-gemini-cli", "127.0.0.1", false],
		["google-antigravity", "127.0.0.1", true],
	] as const) {
		it(`test_callback_listener_is_loopback_only_and_single_use[${provider}]`, async () => {
			const realFetch = globalThis.fetch;
			const exchanges: { url: string; body: Record<string, string> }[] = [];
			const stub = spyOn(globalThis, "fetch").mockImplementation((async (
				input: string | URL | Request,
				init?: RequestInit,
			) => {
				exchanges.push({ url: String(input instanceof Request ? input.url : input), body: formFields(init?.body) });
				throw new Error("stubbed token exchange");
			}) as unknown as typeof fetch);
			restores.push(() => stub.mockRestore());
			const io = new FakeIo({ type: "login", provider });
			const done = authLogin(io, { definition: getProviderDefinition, installEgress: () => undefined });
			const started = await Promise.race([io.until("auth").then(() => "auth"), done.then(() => "exited")]);
			expect(started, `${provider} callback port was busy on this host: ${JSON.stringify(io.out)}`).toBe("auth");
			const auth = io.out.find(message => message.type === "auth");
			if (auth?.type !== "auth") throw new Error("no auth event");
			const authorize = new URL(auth.url);
			const redirect = new URL(authorize.searchParams.get("redirect_uri") ?? "");
			// The provider's registered redirect form is advertised unchanged.
			expect(redirect.protocol).toBe("http:");
			expect(redirect.hostname).toBe(redirectHost);
			const port = Number(redirect.port);

			expect(await reachable("127.0.0.1", port)).toBe(true);
			for (const address of interfaceAddresses()) {
				expect(await reachable(address, port), `listener reachable on ${address}`).toBe(false);
			}

			const callback = `http://127.0.0.1:${port}${redirect.pathname}`;
			const state = authorize.searchParams.get("state") ?? "";
			expect(state).toMatch(/^[0-9a-f]{32}$/);
			expect((await realFetch(`${callback}?code=forged&state=not-the-state`)).status).toBe(500);
			expect(io.out.some(message => message.type === "progress")).toBe(false);
			expect(exchanges).toEqual([]);

			expect((await realFetch(`${callback}?code=synthetic-code&state=${state}`)).status).toBe(200);
			expect(await done).toBe(1);
			expect(io.out.slice(-2)).toEqual([
				{ type: "progress", stage: "exchanging" },
				{ type: "error", code: "failed" },
			]);
			expect(exchanges).toHaveLength(1);
			const [exchange] = exchanges;
			expect(LOGIN_ORIGINS[provider]).toContain(new URL(exchange?.url ?? "").origin);
			expect(exchange?.body.code).toBe("synthetic-code");
			const challenge = authorize.searchParams.get("code_challenge");
			if (pkce) {
				const verifier = exchange?.body.code_verifier ?? "";
				expect(authorize.searchParams.get("code_challenge_method")).toBe("S256");
				expect(challenge).toBe(createHash("sha256").update(verifier).digest("base64url"));
				expect(auth.url).not.toContain(verifier);
			} else {
				expect(challenge).toBeNull();
				expect(exchange?.body.code_verifier).toBeUndefined();
			}
			// The listener closed when the flow settled: a replayed callback has nowhere to land.
			expect(await reachable("127.0.0.1", port)).toBe(false);
		});
	}
});

describe("auth helper egress", () => {
	it("test_login_and_refresh_cannot_reach_unlisted_origins", async () => {
		// The real guard patches process globals for good, so it runs in a child process.
		const child = Bun.spawn([process.execPath, join(import.meta.dir, "fixtures/auth-helper-egress-child.ts")], {
			stdout: "pipe",
			stderr: "pipe",
			env: { ...process.env, NO_PROXY: "*" },
		});
		const [stdout, stderr, code] = await Promise.all([
			new Response(child.stdout).text(),
			new Response(child.stderr).text(),
			child.exited,
		]);
		expect(code).toBe(0);
		const result = JSON.parse(stdout.trim().split("\n").at(-1) ?? "{}");
		// The allowed token origin is dialed pinned to its checked address; the collector never is.
		expect(result).toEqual({
			login: { code: 1, out: [{ type: "error", code: "failed" }], reached: ["https://93.184.216.34/oauth/token"] },
			refresh: { code: 1, out: [{ type: "error", code: "failed" }], reached: ["https://93.184.216.34/v1/oauth/token"] },
		});
		expect(stderr).toContain("outbound connection to https://collector.invalid:443 is not in network.allow");
	});
});
