import { afterAll, afterEach, beforeAll, describe, expect, it, spyOn } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FetchImpl } from "@oh-my-pi/pi-ai";
import {
	anthropicDirectFetch,
	Egress,
	EgressRefused,
	egressResolver,
	makeEgressFetch,
	type ResolvedAddress,
} from "../../src/providers/egress";
import { type ProviderServer, serveProvider } from "./fake-credential-daemon";

const PUBLIC = "93.184.216.34";

interface Restorable {
	mockRestore(): void;
}

interface Dispatch {
	url: string;
	init: RequestInit | undefined;
}

/** Answers per host; each lookup takes the next answer, the last one repeats. */
function resolving(answers: Record<string, string[][] | string[]>): { lookups: string[] } {
	const record = { lookups: [] as string[] };
	const counts: Record<string, number> = {};
	resolver = spyOn(egressResolver, "lookup").mockImplementation(async (host: string): Promise<ResolvedAddress[]> => {
		record.lookups.push(host);
		const entry = answers[host];
		if (!entry) throw new Error(`ENOTFOUND ${host}`);
		const sequence = entry.every(item => Array.isArray(item)) ? (entry as string[][]) : [entry as string[]];
		const index = Math.min(counts[host] ?? 0, sequence.length - 1);
		counts[host] = (counts[host] ?? 0) + 1;
		return (sequence[index] ?? []).map(address => ({ address, family: address.includes(":") ? 6 : 4 }));
	});
	return record;
}

async function failure(promise: Promise<unknown>): Promise<unknown> {
	return promise.then(
		() => undefined,
		(error: unknown) => error,
	);
}

let resolver: Restorable | undefined;
let transport: Restorable | undefined;
const savedProxy = process.env.HTTPS_PROXY;

afterEach(() => {
	resolver?.mockRestore();
	transport?.mockRestore();
	resolver = undefined;
	transport = undefined;
	if (savedProxy === undefined) delete process.env.HTTPS_PROXY;
	else process.env.HTTPS_PROXY = savedProxy;
});

/** Records dispatches that reach Bun's fetch (the direct and fallback transports). */
function recordDispatches(): { calls: Dispatch[] } {
	const record = { calls: [] as Dispatch[] };
	transport = spyOn(globalThis, "fetch").mockImplementation((async (
		input: string | URL | Request,
		init?: RequestInit,
	) => {
		record.calls.push({ url: String(input instanceof Request ? input.url : input), init });
		return new Response("proxied");
	}) as unknown as typeof fetch);
	return record;
}

function stubBase(): { base: FetchImpl; calls: Dispatch[] } {
	const calls: Dispatch[] = [];
	return {
		calls,
		base: async (input, init) => {
			calls.push({ url: String(input), init });
			return new Response("ok");
		},
	};
}

describe("egress addresses", () => {
	it("test_wildcard_names_must_resolve_to_public_addresses", () => {
		const egress = new Egress(["*.example", "10.1.2.3"]);
		egress.checkAddress("https", "rebind.example", PUBLIC, 443);
		egress.checkAddress("https", "rebind.example", "2606:2800:220:1:248:1893:25c8:1946", 443);
		egress.checkAddress("https", "rebind.example", "10.1.2.3", 443);
		egress.checkAddress("http", "localhost", "127.0.0.1", 1);
		egress.checkAddress("http", "localhost", "::1", 1);
		for (const internal of [
			"10.0.0.5",
			"127.0.0.1",
			"169.254.169.254",
			"172.16.0.1",
			"192.168.1.1",
			"100.64.0.1",
			"0.0.0.0",
			"224.0.0.1",
			"::",
			"::1",
			"::ffff:127.0.0.1",
			"::ffff:10.0.0.1",
			"64:ff9b::a00:1",
			"fd00::1",
			"fe80::1",
		]) {
			expect(() => egress.checkAddress("https", "rebind.example", internal, 443)).toThrow(EgressRefused);
		}
		expect(() => egress.checkAddress("http", "localhost", PUBLIC, 1)).toThrow(EgressRefused);
	});

	it("test_exact_names_may_resolve_to_internal_addresses", () => {
		const egress = new Egress(["ollama.lan", "gpu.lan:8000"]);
		egress.allowEndpoint("http://models.lan:11434/v1");
		egress.checkAddress("http", "ollama.lan", "192.168.1.20", 11434);
		egress.checkAddress("https", "gpu.lan", "10.0.0.8", 8000);
		egress.checkAddress("http", "models.lan", "192.168.1.21", 11434);
		// The exact entry covers its own port only.
		expect(() => egress.checkAddress("https", "gpu.lan", "10.0.0.8", 80)).toThrow(EgressRefused);
		expect(() => egress.checkAddress("http", "models.lan", "192.168.1.21", 80)).toThrow(EgressRefused);
	});

	it("test_exact_entry_admits_internal_answers_only_for_its_scheme_and_port", () => {
		const egress = new Egress(["https://lan.example", "10.9.9.9:8443", "*.example"]);
		egress.allowEndpoint("https://relay.example:8443/v1");
		egress.checkAddress("https", "lan.example", "10.0.0.2", 443);
		egress.checkAddress("https", "lan.example", "10.0.0.2", 9443);
		egress.checkAddress("https", "relay.example", "10.0.0.3", 8443);
		egress.checkAddress("http", "any.example", "10.9.9.9", 8443);
		// A wildcard admits these names by URL, so only the exact entry could vouch for an internal answer.
		for (const [scheme, host, address, port] of [
			["http", "lan.example", "10.0.0.2", 443],
			["http", "relay.example", "10.0.0.3", 8443],
			["https", "relay.example", "10.0.0.3", 443],
			["https", "any.example", "10.9.9.9", 443],
		] as const) {
			expect(() => egress.checkAddress(scheme, host, address, port)).toThrow(EgressRefused);
		}
	});

	it("test_allow_listed_name_resolving_to_a_disallowed_ip_is_refused", async () => {
		resolving({
			"rebind.example": ["10.0.0.5"],
			"mixed.example": [PUBLIC, "169.254.169.254"],
			"public.example": [PUBLIC],
			"ollama.lan": ["192.168.1.20"],
		});
		const { base, calls } = stubBase();
		const fetch = makeEgressFetch(["*.example"], "http://ollama.lan:11434", base);
		for (const url of ["https://rebind.example/", "https://mixed.example/"]) {
			expect(await failure(fetch(url, {}))).toBeInstanceOf(EgressRefused);
		}
		expect(calls).toEqual([]);
		await fetch("https://public.example/", {});
		await fetch("http://ollama.lan:11434/api/chat", {});
		expect(calls.map(call => call.url)).toEqual([`https://${PUBLIC}/`, "http://192.168.1.20:11434/api/chat"]);
	});

	it("test_endpoint_scheme_does_not_vouch_for_another_scheme_on_the_same_host", async () => {
		resolving({ "relay.example": ["10.0.0.3"] });
		const { base, calls } = stubBase();
		// The wildcard lets the other schemes and ports pass the URL check; the address check must still refuse them.
		const fetch = makeEgressFetch(["*.example"], "https://relay.example:8443", base);
		for (const url of ["http://relay.example:8443/", "https://relay.example/", "http://relay.example/"]) {
			expect(await failure(fetch(url, {}))).toBeInstanceOf(EgressRefused);
		}
		expect(calls).toEqual([]);
		await fetch("https://relay.example:8443/v1", {});
		expect(calls.map(call => call.url)).toEqual(["https://10.0.0.3:8443/v1"]);
	});

	it("test_every_hop_dials_its_checked_address", async () => {
		const dns = resolving({
			"provider.example": [PUBLIC],
			// The first answer is public, the next one rebinds to the metadata service.
			"cdn.example": [[PUBLIC], ["169.254.169.254"]],
		});
		const calls: Dispatch[] = [];
		const base: FetchImpl = async (input, init) => {
			calls.push({ url: String(input), init });
			return calls.length === 1
				? new Response(null, { status: 302, headers: { location: "https://cdn.example/a" } })
				: calls.length === 2
					? new Response(null, { status: 302, headers: { location: "https://cdn.example/b" } })
					: new Response("unexpected");
		};
		const fetch = makeEgressFetch(["*.example"], "https://provider.example", base);
		expect(
			await failure(fetch("https://provider.example/start", { headers: { authorization: "Bearer k" } })),
		).toBeInstanceOf(EgressRefused);
		expect(dns.lookups).toEqual(["provider.example", "cdn.example", "cdn.example"]);
		expect(calls.map(call => call.url)).toEqual([`https://${PUBLIC}/start`, `https://${PUBLIC}/a`]);
		const [first, second] = calls.map(call => call.init);
		expect(first?.headers).toEqual({ authorization: "Bearer k", Host: "provider.example" });
		expect(Reflect.get(first ?? {}, "tls")).toEqual({ serverName: "provider.example" });
		expect(second?.headers).toEqual({ Host: "cdn.example" });
		expect(Reflect.get(second ?? {}, "tls")).toEqual({ serverName: "cdn.example" });
	});
});

describe("anthropic transport", () => {
	let dir: string;
	let cert: string;
	let tlsServer: ReturnType<typeof Bun.serve> | undefined;
	let plain: ProviderServer;
	const hosts: string[] = [];

	beforeAll(() => {
		dir = mkdtempSync(join(tmpdir(), "openside-egress-"));
		const made = Bun.spawnSync([
			"openssl",
			"req",
			"-x509",
			"-newkey",
			"rsa:2048",
			"-nodes",
			"-days",
			"1",
			"-subj",
			"/CN=provider.test",
			"-addext",
			"subjectAltName=DNS:provider.test",
			"-keyout",
			join(dir, "key.pem"),
			"-out",
			join(dir, "cert.pem"),
		]);
		expect(made.exitCode).toBe(0);
		cert = readFileSync(join(dir, "cert.pem"), "utf8");
		tlsServer = Bun.serve({
			hostname: "127.0.0.1",
			port: 0,
			tls: { cert, key: readFileSync(join(dir, "key.pem"), "utf8") },
			fetch(request) {
				hosts.push(request.headers.get("host") ?? "");
				return new Response("pinned");
			},
		});
		plain = serveProvider(
			() => new Response(null, { status: 302, headers: { location: "http://collector.invalid/" } }),
		);
	});

	afterAll(() => {
		tlsServer?.stop(true);
		plain.stop();
		rmSync(dir, { recursive: true, force: true });
	});

	/** The transport chain `stream.ts` builds for first-party Anthropic. */
	function anthropicFetch(allow: string[], origin: string): FetchImpl {
		return makeEgressFetch(allow, origin, anthropicDirectFetch);
	}

	it("test_direct_https_path_connects_only_to_the_checked_address", async () => {
		resolving({ "provider.test": ["127.0.0.1"] });
		const origin = `https://provider.test:${tlsServer?.port}`;
		// An exact endpoint may resolve to an internal address; the connection uses that answer.
		const response = await anthropicFetch([], origin)(`${origin}/v1/messages`, {
			method: "GET",
			headers: { "accept-encoding": "identity" },
			tls: { ca: cert },
		} as RequestInit);
		expect(await response.text()).toBe("pinned");
		expect(hosts.at(-1)).toBe(`provider.test:${tlsServer?.port}`);
	});

	it("test_direct_https_path_refuses_rebinding_before_connecting", async () => {
		resolving({ "rebind.example": ["10.0.0.9"] });
		const dispatched = recordDispatches();
		const fetch = anthropicFetch(["*.example"], "https://api.anthropic.com");
		expect(
			await failure(fetch("https://rebind.example/v1/messages", { method: "POST", headers: {}, body: "{}" })),
		).toBeInstanceOf(EgressRefused);
		expect(dispatched.calls).toEqual([]);
	});

	it("test_request_object_and_non_record_headers_are_checked", async () => {
		resolving({ "rebind.example": ["10.0.0.9"] });
		const dispatched = recordDispatches();
		const fetch = anthropicFetch(["*.example"], "https://api.anthropic.com");
		const request = new Request("https://collector.invalid/steal", { method: "POST", body: "{}" });
		expect(await failure(fetch(request, undefined))).toBeInstanceOf(EgressRefused);
		expect(
			await failure(fetch("https://rebind.example/v1/messages", { headers: new Headers({ "x-test": "1" }) })),
		).toBeInstanceOf(EgressRefused);
		expect(dispatched.calls).toEqual([]);
	});

	it("test_fallback_redirect_hops_are_checked", async () => {
		const fetch = anthropicFetch([], plain.url);
		const error = await failure(fetch(`${plain.url}/redirect`, { method: "GET", headers: {} }));
		expect(error).toBeInstanceOf(EgressRefused);
		expect(String(error)).toContain("collector.invalid");
	});

	it("test_proxy_destination_is_checked", async () => {
		resolving({ "api.anthropic.com": [PUBLIC] });
		const dispatched = recordDispatches();
		const fetch = anthropicFetch([], "https://api.anthropic.com");
		const explicit = await failure(
			fetch("https://api.anthropic.com/v1/messages", {
				method: "POST",
				headers: {},
				body: "{}",
				proxy: "http://proxy.invalid:3128",
			} as RequestInit),
		);
		expect(explicit).toBeInstanceOf(EgressRefused);
		expect(String(explicit)).toContain("proxy.invalid");
		process.env.HTTPS_PROXY = "http://proxy.invalid:3128";
		expect(
			await failure(fetch("https://api.anthropic.com/v1/messages", { method: "POST", headers: {}, body: "{}" })),
		).toBeInstanceOf(EgressRefused);
		expect(dispatched.calls).toEqual([]);
	});

	it("test_validated_env_proxy_is_used_instead_of_dialing_the_target", async () => {
		resolving({ "api.anthropic.com": [PUBLIC] });
		const dispatched = recordDispatches();
		process.env.HTTPS_PROXY = "http://127.0.0.1:9";
		const response = await anthropicFetch([], "https://api.anthropic.com")("https://api.anthropic.com/v1/messages", {
			method: "POST",
			headers: {},
			body: "{}",
		});
		expect(await response.text()).toBe("proxied");
		expect(dispatched.calls).toHaveLength(1);
		expect(dispatched.calls[0]?.url).toBe("https://api.anthropic.com/v1/messages");
		expect(Reflect.get(dispatched.calls[0]?.init ?? {}, "proxy")).toBe("http://127.0.0.1:9/");
	});
});

describe("proxies and redirects", () => {
	const savedHttpProxy = process.env.HTTP_PROXY;
	const savedNoProxy = process.env.NO_PROXY;

	afterEach(() => {
		if (savedHttpProxy === undefined) delete process.env.HTTP_PROXY;
		else process.env.HTTP_PROXY = savedHttpProxy;
		if (savedNoProxy === undefined) delete process.env.NO_PROXY;
		else process.env.NO_PROXY = savedNoProxy;
	});

	it("test_cross_origin_redirect_sends_the_new_host_and_server_name", async () => {
		resolving({ "provider.example": [PUBLIC], "mirror.example": ["93.184.216.35"] });
		const calls: Dispatch[] = [];
		const base: FetchImpl = async (input, init) => {
			calls.push({ url: String(input), init });
			return calls.length === 1
				? new Response(null, { status: 307, headers: { location: "https://mirror.example/next" } })
				: new Response("ok");
		};
		const fetch = makeEgressFetch(["mirror.example"], "https://provider.example", base);
		await fetch("https://provider.example/start", {
			headers: { host: "stale.example", HOST: "other.example", "x-test": "1" },
		});
		expect(calls.map(call => call.url)).toEqual([`https://${PUBLIC}/start`, "https://93.184.216.35/next"]);
		expect(calls[0]?.init?.headers).toEqual({ "x-test": "1", Host: "provider.example" });
		expect(calls[1]?.init?.headers).toEqual({ "x-test": "1", Host: "mirror.example" });
		expect(Reflect.get(calls[1]?.init ?? {}, "tls")).toEqual({ serverName: "mirror.example" });
	});

	it("test_proxy_dns_rebinding_is_refused", async () => {
		resolving({ "api.anthropic.com": [PUBLIC], "proxy.corp.example": ["169.254.169.254"] });
		const { base, calls } = stubBase();
		process.env.HTTPS_PROXY = "http://proxy.corp.example:3128";
		delete process.env.NO_PROXY;
		const fetch = makeEgressFetch(["*.corp.example"], "https://api.anthropic.com", base);
		expect(await failure(fetch("https://api.anthropic.com/v1/messages", {}))).toBeInstanceOf(EgressRefused);
		// A name cannot pin an HTTPS proxy, whose certificate would then go unchecked.
		process.env.HTTPS_PROXY = "https://proxy.corp.example:3128";
		expect(await failure(fetch("https://api.anthropic.com/v1/messages", {}))).toBeInstanceOf(EgressRefused);
		expect(calls).toEqual([]);
	});

	it("test_proxied_hop_dials_the_pinned_proxy_address", async () => {
		resolving({ "api.anthropic.com": [PUBLIC], "proxy.lan": ["10.0.0.3"] });
		const { base, calls } = stubBase();
		process.env.HTTPS_PROXY = "http://user:pass@proxy.lan:3128";
		delete process.env.NO_PROXY;
		const fetch = makeEgressFetch(["proxy.lan:3128"], "https://api.anthropic.com", base);
		await fetch("https://api.anthropic.com/v1/messages", {});
		expect(calls.map(call => call.url)).toEqual(["https://api.anthropic.com/v1/messages"]);
		expect(Reflect.get(calls[0]?.init ?? {}, "proxy")).toBe("http://user:pass@10.0.0.3:3128/");
	});

	it("test_proxied_and_literal_hops_send_the_current_host", async () => {
		resolving({ "api.anthropic.com": [PUBLIC], "proxy.lan": ["10.0.0.3"] });
		const { base, calls } = stubBase();
		process.env.HTTPS_PROXY = "http://proxy.lan:3128";
		delete process.env.NO_PROXY;
		const fetch = makeEgressFetch(["proxy.lan:3128", "10.9.9.9"], "https://api.anthropic.com", base);
		await fetch("https://api.anthropic.com/v1/messages", { headers: { Host: "stale.example", "x-test": "1" } });
		expect(calls[0]?.init?.headers).toEqual({ "x-test": "1", Host: "api.anthropic.com" });
		expect(Reflect.get(calls[0]?.init ?? {}, "proxy")).toBe("http://10.0.0.3:3128/");
		delete process.env.HTTPS_PROXY;
		await fetch("https://10.9.9.9:8443/v1", { headers: { host: "stale.example" } });
		expect(calls[1]?.url).toBe("https://10.9.9.9:8443/v1");
		expect(calls[1]?.init?.headers).toEqual({ Host: "10.9.9.9:8443" });
	});

	it("test_proxy_connection_reaches_the_checked_address", async () => {
		const seen: string[] = [];
		const proxy = Bun.serve({
			hostname: "127.0.0.1",
			port: 0,
			fetch(request) {
				seen.push(request.url);
				return new Response("via proxy");
			},
		});
		try {
			resolving({ "proxy.test": ["127.0.0.1"] });
			process.env.HTTP_PROXY = `http://proxy.test:${proxy.port}`;
			delete process.env.NO_PROXY;
			const fetch = makeEgressFetch(["proxy.test"], "http://upstream.example");
			const response = await fetch("http://upstream.example/x", {});
			expect(await response.text()).toBe("via proxy");
			expect(seen).toEqual(["http://upstream.example/x"]);
		} finally {
			proxy.stop(true);
		}
	});
});
