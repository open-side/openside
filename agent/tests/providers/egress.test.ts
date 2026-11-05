import { afterAll, afterEach, beforeAll, describe, expect, it, spyOn } from "bun:test";
import type { FetchImpl } from "@oh-my-pi/pi-ai";
import { streamSimple } from "@oh-my-pi/pi-ai";
import { Egress, EgressRefused, egressResolver, makeEgressFetch, parseEntry } from "../../src/providers/egress";
import { resolveModel } from "../../src/providers/models";
import { type ProviderServer, serveProvider, userContext } from "./fake-credential-daemon";

let loopback: ProviderServer;

let dns: { mockRestore(): void } | undefined;

/** These hosts never resolve; answer with a public address so only the name rules apply. */
function publicDns(): void {
	dns = spyOn(egressResolver, "lookup").mockImplementation(async () => [{ address: "93.184.216.34", family: 4 }]);
}

afterEach(() => {
	dns?.mockRestore();
	dns = undefined;
});

beforeAll(() => {
	loopback = serveProvider(request =>
		new URL(request.url).pathname === "/redirect"
			? new Response(null, {
					status: 302,
					headers: { location: "http://collector.invalid/steal" },
				})
			: new Response(`ok ${request.headers.authorization ?? "none"}`),
	);
});

afterAll(() => loopback.stop());

async function refusal(promise: Promise<unknown>): Promise<unknown> {
	return promise.then(
		() => undefined,
		(error: unknown) => error,
	);
}

describe("egress", () => {
	it("test_entry_syntax_matches_the_rust_egress_list", () => {
		const allow = new Egress(["api.example.com", "*.cdn.example", "proxy.example:8443", "https://only.example"]);
		for (const [scheme, host, port] of [
			["https", "API.example.com.", 443],
			["http", "api.example.com", 8080],
			["https", "a.b.cdn.example", 443],
			["https", "proxy.example", 8443],
			["https", "only.example", 443],
			["http", "localhost", 9],
			["http", "127.0.0.1", 1],
			["http", "[::1]", 1],
			["http", "::ffff:127.0.0.1", 1],
		] as const) {
			allow.check(scheme, host, port);
		}
		for (const [scheme, host, port] of [
			["https", "cdn.example", 443],
			["https", "evilapi.example.com", 443],
			["https", "proxy.example", 443],
			["http", "only.example", 443],
		] as const) {
			expect(() => allow.check(scheme, host, port)).toThrow(EgressRefused);
		}
		for (const invalid of ["", "*", "a.*.example", "https://x/path", "user@x", "x:99999", "*.:1"]) {
			expect(() => parseEntry(invalid)).toThrow();
		}
		// IDNA: a Unicode host is compared in its ASCII form, like the browser and Rust.
		new Egress(["xn--bcher-kva.example"]).checkUrl(new URL("https://bücher.example/"));
	});

	it("test_transports_refuse_unlisted_hosts_and_redirect_hops", async () => {
		const fetch = makeEgressFetch(["allowed.invalid"], "https://provider.invalid");
		expect(await (await fetch(`${loopback.url}/`, {})).text()).toBe("ok none");
		const direct = await refusal(fetch("http://collector.invalid/", {}));
		expect(direct).toBeInstanceOf(EgressRefused);
		expect(String(direct)).toContain("collector.invalid");
		const redirected = await refusal(fetch(`${loopback.url}/redirect`, {}));
		expect(redirected).toBeInstanceOf(EgressRefused);
		expect(String(redirected)).toContain("collector.invalid");
		// The allow-listed host gets past the guard; only name resolution fails then.
		const resolved = await refusal(fetch("http://allowed.invalid/", {}));
		expect(resolved).toBeDefined();
		expect(resolved).not.toBeInstanceOf(EgressRefused);
	});

	it("test_redirect_to_another_origin_drops_credentials", async () => {
		publicDns();
		const sent: Record<string, string>[] = [];
		const base: FetchImpl = async (input, init) => {
			sent.push({ ...(init?.headers as Record<string, string>) });
			return String(input).endsWith("/start")
				? new Response(null, {
						status: 307,
						headers: { location: "https://mirror.example/next" },
					})
				: new Response("ok");
		};
		const fetch = makeEgressFetch(["mirror.example"], "https://provider.invalid", base);
		await fetch("https://provider.invalid/start", {
			headers: { authorization: "Bearer secret", "x-api-key": "secret" },
		});
		expect(sent[0]?.authorization).toBe("Bearer secret");
		expect(sent[1]?.authorization).toBeUndefined();
		expect(sent[1]?.["x-api-key"]).toBeUndefined();
	});

	it("test_provider_sdk_requests_are_refused_outside_the_list", async () => {
		let reached = false;
		const base: FetchImpl = async () => {
			reached = true;
			return new Response("{}");
		};
		const model = {
			...resolveModel("anthropic:claude-sonnet-5-5"),
			baseUrl: "http://collector.invalid",
		};
		const guarded = makeEgressFetch([], "https://api.anthropic.com", base);
		// The SDK retries transport errors; stop after the first refusal like the stream layer does.
		const stop = new AbortController();
		const refusals: unknown[] = [];
		const fetch: FetchImpl = async (input, init) => {
			try {
				return await guarded(input, init);
			} catch (error) {
				refusals.push(error);
				stop.abort();
				throw error;
			}
		};
		const result = await streamSimple(model, userContext("hi"), {
			apiKey: "test-key",
			fetch,
			signal: stop.signal,
			maxRetryDelayMs: 0,
		}).result();
		expect(["error", "aborted"]).toContain(result.stopReason);
		expect(refusals).toHaveLength(1);
		expect(refusals[0]).toBeInstanceOf(EgressRefused);
		expect(reached).toBe(false);
	});

	it("test_built_provider_endpoint_is_admitted", async () => {
		publicDns();
		const allow = new Egress([]);
		const endpoint = new URL("https://provider.invalid/v1/chat/completions");
		expect(() => allow.checkUrl(endpoint)).toThrow(EgressRefused);
		const reached: string[] = [];
		const base: FetchImpl = async input => {
			reached.push(String(input));
			return new Response("ok");
		};
		const fetch = makeEgressFetch([], "https://provider.invalid/v1", base);
		await fetch(endpoint.href, {});
		// The dispatch is pinned to the checked address.
		expect(reached).toEqual(["https://93.184.216.34/v1/chat/completions"]);
		expect(await refusal(fetch("https://other.invalid/v1", {}))).toBeInstanceOf(EgressRefused);
	});
});
