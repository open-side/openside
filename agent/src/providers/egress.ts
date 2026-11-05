/**
 * Outbound destination allow-list for the sidecar (D-5, G-4, PP-25).
 *
 * The sidecar may reach loopback, the origin of the provider it is calling, and
 * the user's `network.allow` entries, which the daemon forwards as
 * `open.config.agent.network_allow`. Entry syntax and matching mirror the Rust
 * `openside_policy::Egress`: `host`, `*.domain` (subdomains only), `host:port`
 * or `scheme://host[:port]`; an entry without a port matches every port.
 *
 * Every model request goes through the fetch built here, so each redirect hop
 * is checked before a connection opens: redirects are followed manually.
 */
import http from "node:http";
import https from "node:https";
import net, { isIP } from "node:net";
import tls from "node:tls";
import type { FetchImpl } from "@oh-my-pi/pi-ai";
import { coworkFetch } from "@oh-my-pi/pi-ai/providers/cowork-fetch";
import { canonicalIp, isInternalAddress, isLoopbackAddress } from "./addresses";
import { AUTH_HEADERS, headerRecord } from "./request-auth";

const DEFAULT_PORTS: Record<string, number> = {
	http: 80,
	https: 443,
	ws: 80,
	wss: 443,
};
const MAX_REDIRECTS = 10;

export class EgressRefused extends Error {
	override readonly name = "EgressRefused";
	readonly destination: string;

	constructor(destination: string) {
		super(`outbound connection to ${destination} is not in network.allow`);
		this.destination = destination;
	}
}

export interface EgressEntry {
	scheme: string | undefined;
	host: string;
	port: number | undefined;
}

function invalidEntry(raw: string): Error {
	return new Error(`invalid network.allow entry ${JSON.stringify(raw)}`);
}

function portNumber(value: string, raw: string): number {
	if (!/^\d+$/.test(value) || Number(value) > 65535) throw invalidEntry(raw);
	return Number(value);
}

function splitHostPort(text: string, raw: string): [string, number | undefined] {
	if (text.startsWith("[")) {
		const close = text.indexOf("]");
		if (close < 0) throw invalidEntry(raw);
		const host = text.slice(1, close);
		const after = text.slice(close + 1);
		if (after.startsWith(":")) return [host, portNumber(after.slice(1), raw)];
		if (after) throw invalidEntry(raw);
		return [host, undefined];
	}
	const colon = text.lastIndexOf(":");
	if (colon < 0) return [text, undefined];
	const host = text.slice(0, colon);
	// A bare IPv6 literal has several colons and no port.
	if (host.includes(":")) return [text, undefined];
	return [host, portNumber(text.slice(colon + 1), raw)];
}

export function parseEntry(raw: string): EgressEntry {
	let text = raw.trim().toLowerCase();
	let scheme: string | undefined;
	const separator = text.indexOf("://");
	if (separator >= 0) {
		scheme = text.slice(0, separator);
		text = text.slice(separator + 3);
	}
	if (text.endsWith("/")) text = text.slice(0, -1);
	if (/[/?#@]/.test(text)) throw invalidEntry(raw);
	const [host, port] = splitHostPort(text, raw);
	const body = host.startsWith("*.") ? host.slice(2) : host;
	if (!body || body.includes("*")) throw invalidEntry(raw);
	return { scheme, host, port };
}

function isLoopbackIpv6(host: string): boolean {
	let normalized: string;
	try {
		normalized = new URL(`http://[${host}]/`).hostname;
	} catch {
		return false;
	}
	if (normalized === "[::1]") return true;
	// IPv4-mapped loopback (::ffff:127.0.0.0/104) normalizes to `::ffff:7fxx:xxxx`.
	return /^\[::ffff:7f[0-9a-f]{2}:[0-9a-f]{1,4}\]$/.test(normalized);
}

export function isLoopback(host: string): boolean {
	if (host === "localhost" || host.endsWith(".localhost")) return true;
	const octets = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host);
	if (octets) return octets.slice(1).every(octet => Number(octet) <= 255) && octets[1] === "127";
	return host.includes(":") && isLoopbackIpv6(host);
}

function hostMatches(pattern: string, host: string): boolean {
	if (pattern.startsWith("*.")) {
		const domain = pattern.slice(2);
		if (!host.endsWith(domain)) return false;
		const rest = host.slice(0, host.length - domain.length);
		return rest.length > 1 && rest.endsWith(".");
	}
	return pattern === host;
}

export class Egress {
	readonly #entries: EgressEntry[];

	constructor(entries: Iterable<string> = []) {
		this.#entries = [...entries].map(parseEntry);
	}

	allow(entry: string): void {
		const parsed = parseEntry(entry);
		const known = this.#entries.some(
			item => item.scheme === parsed.scheme && item.host === parsed.host && item.port === parsed.port,
		);
		if (!known) this.#entries.push(parsed);
	}

	/** Admits a configured endpoint (a model base URL) by its exact origin. */
	allowEndpoint(url: string): void {
		const parsed = new URL(url);
		const scheme = parsed.protocol.slice(0, -1);
		const port = parsed.port ? Number(parsed.port) : DEFAULT_PORTS[scheme];
		if (!parsed.hostname || port === undefined) return;
		this.allow(`${scheme}://${parsed.hostname}:${port}`);
	}

	check(scheme: string, host: string, port: number): void {
		const normalizedScheme = scheme.toLowerCase();
		const normalizedHost = host
			.replace(/^\[|\]$/g, "")
			.replace(/\.+$/, "")
			.toLowerCase();
		if (isLoopback(normalizedHost)) return;
		const allowed = this.#entries.some(
			entry =>
				(entry.scheme === undefined || entry.scheme === normalizedScheme) &&
				(entry.port === undefined || entry.port === port) &&
				hostMatches(entry.host.replace(/^\[|\]$/g, ""), normalizedHost),
		);
		if (!allowed) throw new EgressRefused(`${normalizedScheme}://${normalizedHost}:${port}`);
	}

	/** Checks a URL; WHATWG parsing already gives the IDNA (ASCII) host form. */
	checkUrl(url: URL): void {
		const scheme = url.protocol.slice(0, -1);
		const port = url.port ? Number(url.port) : DEFAULT_PORTS[scheme];
		if (!url.hostname || port === undefined) throw new EgressRefused(url.href);
		this.check(scheme, url.hostname, port);
	}

	/**
	 * Checks one resolved address of `host` (DNS rebinding). A name listed
	 * exactly (a network.allow entry or an admitted endpoint) may resolve
	 * anywhere, as may an IP listed literally. A loopback name must stay on
	 * loopback, and a name reached only through a wildcard entry must resolve
	 * to a public address, never loopback, private, link-local (metadata) or
	 * other internal ranges.
	 */
	checkAddress(host: string, address: string, port: number): void {
		const name = host
			.replace(/^\[|\]$/g, "")
			.replace(/\.+$/, "")
			.toLowerCase();
		const ip = canonicalIp(address);
		if (ip === undefined) throw new EgressRefused(`${name} (${address}):${port}`);
		if (isLoopback(name) && isLoopbackAddress(ip)) return;
		const listed = this.#entries.some(
			entry =>
				(entry.port === undefined || entry.port === port) && (entry.host === name || canonicalIp(entry.host) === ip),
		);
		if (listed) return;
		if (!isLoopback(name) && !isInternalAddress(ip)) return;
		throw new EgressRefused(`${name} (${ip}):${port}`);
	}
}

function requestUrl(input: string | URL | Request): URL {
	return new URL(input instanceof Request ? input.url : input);
}

function redirectTarget(response: Response, from: URL): URL | undefined {
	if (![301, 302, 303, 307, 308].includes(response.status)) return undefined;
	const location = response.headers.get("location");
	return location ? new URL(location, from) : undefined;
}

function reported<T>(error: T): T {
	if (error instanceof EgressRefused) process.stderr.write(`openside-agent: ${error.message}\n`);
	return error;
}

function refusing(check: () => void): void {
	try {
		check();
	} catch (error) {
		throw reported(error);
	}
}

function refuseUnlessAllowed(egress: Egress, url: URL): void {
	refusing(() => egress.checkUrl(url));
}

export interface ResolvedAddress {
	address: string;
	family: number;
}

/**
 * Name resolution done before any connection opens. `Bun.dns` shares its
 * cache with Bun's `fetch`, so the checked answer is the one the connection
 * uses. Tests replace `lookup` to simulate rebinding answers.
 */
export const egressResolver = {
	lookup: async (host: string): Promise<ResolvedAddress[]> =>
		(await Bun.dns.lookup(host)).map(({ address, family }) => ({ address, family })),
};

function portOf(scheme: string, port: number | string | undefined): number {
	return port === undefined || port === "" ? (DEFAULT_PORTS[scheme] ?? Number.NaN) : Number(port);
}

/**
 * Resolves `host` and checks every answer (DNS rebinding): an IP literal was
 * already checked by name, so only names resolve here.
 */
async function resolveChecked(egress: Egress, host: string, port: number): Promise<ResolvedAddress[]> {
	const bare = host.replace(/^\[|\]$/g, "");
	if (isIP(bare)) return [{ address: bare, family: isIP(bare) }];
	const addresses = await egressResolver.lookup(bare);
	if (addresses.length === 0) throw reported(new EgressRefused(`${bare}:${port}`));
	for (const { address } of addresses) refusing(() => egress.checkAddress(bare, address, port));
	return addresses;
}

function matchesNoProxy(url: URL): boolean {
	const list = process.env.NO_PROXY ?? process.env.no_proxy ?? "";
	return list
		.split(",")
		.map(entry =>
			entry
				.trim()
				.toLowerCase()
				.replace(/^\*?\./, ""),
		)
		.some(entry => entry === "*" || url.hostname === entry || url.hostname.endsWith(`.${entry}`));
}

/** The proxy a request would dial instead of `url`: `init.proxy`, then Bun's proxy environment. */
function proxyFor(url: URL, request: RequestInit): URL | undefined {
	const explicit: unknown = Reflect.get(request, "proxy");
	if (typeof explicit === "string" && explicit) return new URL(explicit);
	if (isLoopback(url.hostname.replace(/^\[|\]$/g, "")) || matchesNoProxy(url)) return undefined;
	const env =
		url.protocol === "https:"
			? (process.env.HTTPS_PROXY ?? process.env.https_proxy)
			: (process.env.HTTP_PROXY ?? process.env.http_proxy);
	return env ? new URL(env) : undefined;
}

/**
 * Sets `Host` to the hop being dialed, in the position `coworkFetch` uses so
 * pinned requests keep its wire shape. Any earlier `Host` (a caller's, or a
 * previous redirect hop's) is dropped so it always matches the TLS server name.
 */
function withHostHeader(headers: Record<string, string>, host: string): Record<string, string> {
	const ordered: Record<string, string> = {};
	let placed = false;
	for (const [name, value] of Object.entries(headers)) {
		const lower = name.toLowerCase();
		if (lower === "host") continue;
		if (!placed && lower === "accept-encoding") {
			ordered.Host = host;
			placed = true;
		}
		ordered[name] = value;
	}
	if (!placed) ordered.Host = host;
	return ordered;
}

/**
 * Checks the hop by name, then every resolved address of whatever it dials,
 * and returns what to dispatch. A proxied hop goes to the checked proxy, made
 * explicit so no transport falls back to dialing the target. A direct hop to
 * a name is pinned to a checked address: the URL carries the IP while `Host`
 * and the TLS server name keep the hostname, so a second DNS answer can
 * never redirect the connection.
 */
async function checkedHop(egress: Egress, url: URL, request: RequestInit): Promise<[string, RequestInit]> {
	refuseUnlessAllowed(egress, url);
	// Every path sends the current target's Host; the proxy URL is carried separately.
	const headers = withHostHeader(headerRecord(request.headers), url.host);
	const proxy = proxyFor(url, request);
	if (proxy) {
		refuseUnlessAllowed(egress, proxy);
		const proxyName = proxy.hostname.replace(/^\[|\]$/g, "");
		const proxyPort = portOf(proxy.protocol.slice(0, -1), proxy.port);
		if (isIP(proxyName)) return [url.href, { ...request, headers, proxy: proxy.href } as RequestInit];
		// Bun has no server-name option for the proxy connection, so an HTTPS
		// proxy pinned to an IP could not verify its certificate by name.
		if (proxy.protocol === "https:") throw reported(new EgressRefused(`${proxy.origin} (HTTPS proxy by name)`));
		const [first] = await resolveChecked(egress, proxyName, proxyPort);
		if (!first) throw reported(new EgressRefused(proxy.host));
		// The target URL stays as is: the proxy resolves it and receives it in CONNECT or the request line.
		const pinnedProxy = new URL(proxy.href);
		pinnedProxy.hostname = first.family === 6 ? `[${first.address}]` : first.address;
		return [url.href, { ...request, headers, proxy: pinnedProxy.href } as RequestInit];
	}
	const name = url.hostname.replace(/^\[|\]$/g, "");
	if (isIP(name)) return [url.href, { ...request, headers }];
	const [first] = await resolveChecked(egress, name, portOf(url.protocol.slice(0, -1), url.port));
	if (!first) throw reported(new EgressRefused(url.host));
	const pinned = new URL(url.href);
	pinned.hostname = first.family === 6 ? `[${first.address}]` : first.address;
	if (url.protocol !== "https:") return [pinned.href, { ...request, headers }];
	const tlsOptions: unknown = Reflect.get(request, "tls");
	const tlsInit = { ...(typeof tlsOptions === "object" ? tlsOptions : {}), serverName: name };
	return [pinned.href, { ...request, headers, tls: tlsInit } as RequestInit];
}

/**
 * Follows redirects manually so every hop is checked before it connects.
 * Credential headers are dropped on any hop outside `origin` (the first
 * request's origin when omitted).
 */
function checkedFetch(egress: Egress, origin: string | undefined, base: FetchImpl): FetchImpl {
	return async (input, init) => {
		let url = requestUrl(input);
		const home = origin ?? url.origin;
		let request: RequestInit =
			input instanceof Request
				? {
						method: input.method,
						headers: input.headers,
						body: input.body === null ? undefined : await input.arrayBuffer(),
						signal: input.signal,
						...init,
					}
				: { ...init };
		request.headers = headerRecord(request.headers);
		const follow = request.redirect === undefined || request.redirect === "follow";
		for (let hop = 0; ; hop++) {
			if (url.origin !== home) {
				const headers = headerRecord(request.headers);
				for (const name of AUTH_HEADERS) delete headers[name];
				for (const name of Object.keys(headers)) if (name.toLowerCase() === "host") delete headers[name];
				request = { ...request, headers };
			}
			const [target, dispatched] = await checkedHop(egress, url, request);
			if (!follow) return base(target, dispatched);
			const response = await base(target, { ...dispatched, redirect: "manual" });
			const next = redirectTarget(response, url);
			if (next === undefined) return response;
			if (hop >= MAX_REDIRECTS) throw reported(new EgressRefused(next.href));
			await response.body?.cancel();
			const method = (request.method ?? "GET").toUpperCase();
			if (response.status === 303 || ((response.status === 301 || response.status === 302) && method === "POST")) {
				request = { ...request, method: "GET", body: undefined };
			}
			url = next;
		}
	};
}

let rawFetch: typeof fetch | undefined;

/**
 * Bun's own `fetch`, bypassing the process guard. Requests built by
 * `checkedFetch` are already checked and carry a pinned IP, which the guard's
 * name rules would refuse.
 */
export const directFetch: FetchImpl = (input, init) => (rawFetch ?? globalThis.fetch)(input, init);

/**
 * pi-ai's first-party Anthropic transport (it keeps that TLS profile) for the
 * direct HTTPS requests `checkedFetch` produces; anything else it would hand
 * to the guarded global `fetch`, so it goes to `directFetch` here.
 */
export const anthropicDirectFetch: FetchImpl = (input, init) => {
	const url = requestUrl(input);
	const proxied = typeof Reflect.get(init ?? {}, "proxy") === "string";
	return url.protocol === "https:" && !proxied ? coworkFetch(input, init) : directFetch(input, init);
};

/**
 * Builds the fetch every model request uses: loopback, `allow` and the exact
 * `providerOrigin` are reachable; credential headers never leave the provider
 * origin, including across redirects.
 */
export function makeEgressFetch(
	allow: readonly string[],
	providerOrigin: string,
	base: FetchImpl = directFetch,
): FetchImpl {
	const egress = new Egress(allow);
	egress.allowEndpoint(providerOrigin);
	return checkedFetch(egress, new URL(providerOrigin).origin, base);
}

let active: Egress | undefined;

/** The process-wide guard, once `installEgress` ran. */
export function activeEgress(): Egress | undefined {
	return active;
}

function guardedHost(scheme: string, host: string | undefined, port: number | string | undefined): void {
	const guard = active;
	if (!guard) return;
	refusing(() => guard.check(scheme, host ?? "localhost", portOf(scheme, port)));
}

type LookupCallback = (error: Error | null, address?: unknown, family?: number) => void;

/** `dns.lookup`-shaped hook for node sockets: every answer is checked before connecting. */
function guardedLookup(port: number) {
	return (hostname: string, options: unknown, callback?: LookupCallback): void => {
		const done = (typeof options === "function" ? options : callback) as LookupCallback;
		const all = typeof options === "object" && options !== null && Reflect.get(options, "all") === true;
		const guard = active;
		if (!guard) {
			done(new EgressRefused(hostname));
			return;
		}
		resolveChecked(guard, hostname, port).then(
			addresses => {
				const [first] = addresses;
				if (all) done(null, addresses);
				else done(null, first?.address, first?.family);
			},
			(error: Error) => done(error),
		);
	};
}

interface NodeTarget {
	host?: string;
	hostname?: string;
	port?: number | string;
	protocol?: string;
	path?: string;
}

/** `net.connect(port, host, cb)` / `net.connect(options, cb)`: checked by name, resolved through the guard. */
function guardSocketArgs(scheme: string, args: unknown[]): unknown[] {
	const [first, second, ...rest] = args;
	if (typeof first === "number" || (typeof first === "string" && /^\d+$/.test(first))) {
		const host = typeof second === "string" ? second : "localhost";
		guardedHost(scheme, host, first);
		const tail = typeof second === "string" ? rest : [second, ...rest];
		return [{ port: Number(first), host, lookup: guardedLookup(Number(first)) }, ...tail];
	}
	if (typeof first !== "object" || first === null) return args;
	const target: NodeTarget = first;
	if (target.path !== undefined && target.port === undefined) return args;
	guardedHost(scheme, target.hostname ?? target.host, target.port);
	return [{ ...target, lookup: guardedLookup(portOf(scheme, target.port)) }, second, ...rest];
}

/** `http.request(url | options, options?, cb?)`. */
function guardHttpArgs(defaultScheme: string, args: unknown[]): unknown[] {
	const [first, second, ...rest] = args;
	if (typeof first === "string" || first instanceof URL) {
		const url = new URL(first);
		const scheme = url.protocol.slice(0, -1);
		guardedHost(scheme, url.hostname, url.port);
		const lookup = guardedLookup(portOf(scheme, url.port));
		return typeof second === "object" && second !== null
			? [first, { ...second, lookup }, ...rest]
			: [first, { lookup }, second, ...rest];
	}
	if (typeof first !== "object" || first === null) return args;
	const target: NodeTarget = first;
	const scheme = target.protocol ? target.protocol.replace(/:$/, "") : defaultScheme;
	if (
		target.path !== undefined &&
		target.host === undefined &&
		target.hostname === undefined &&
		"socketPath" in target
	) {
		return args;
	}
	guardedHost(scheme, target.hostname ?? target.host?.replace(/:\d+$/, ""), target.port);
	return [{ ...target, lookup: guardedLookup(portOf(scheme, target.port)) }, second, ...rest];
}

function wrapMethod(owner: object, name: string, guard: (args: unknown[]) => unknown[]): void {
	const original: unknown = Reflect.get(owner, name);
	if (typeof original !== "function") return;
	const wrapped = function (this: unknown, ...args: unknown[]) {
		const guarded = guard(args);
		// Keep the caller's arity: trailing `undefined` slots would read as explicit arguments.
		while (guarded.length > 0 && guarded.at(-1) === undefined) guarded.pop();
		return Reflect.apply(original, this, guarded);
	};
	Reflect.set(owner, name, wrapped);
}

/**
 * Enforces the allow-list on every outbound transport reachable through
 * process globals: `fetch`, `WebSocket`, `Bun.connect` and the `node:`
 * net/tls/http/https default exports. Names are resolved and every answer is
 * checked before a connection opens. Loopback and unix sockets (the daemon
 * connection) stay reachable. Later calls replace the allow-list; the
 * transports are patched once.
 *
 * Gap: modules that import `node:net`/`tls`/`http`/`https` as `* as`
 * namespaces hold their own bindings, which these patches never reach. In
 * the omp packages that is pi-ai's `utils/proxy.ts` `connectProxiedSocket`
 * (used only by the cursor provider, which openside does not route) and
 * `cowork-fetch.ts`, which only receives requests `checkedFetch` already
 * checked and pinned.
 */
export function installEgress(allow: Iterable<string>): Egress {
	const first = active === undefined;
	const egress = new Egress(allow);
	active = egress;
	if (!first) return egress;
	const nativeFetch = globalThis.fetch;
	rawFetch = nativeFetch;
	const fetchGuard = (input: string | URL | Request, init?: RequestInit) =>
		checkedFetch(active ?? egress, undefined, nativeFetch)(input, init);
	globalThis.fetch = Object.assign(fetchGuard, { preconnect: nativeFetch.preconnect });
	globalThis.WebSocket = new Proxy(globalThis.WebSocket, {
		construct(target, args, newTarget) {
			const url = new URL(args[0]);
			const scheme = url.protocol.slice(0, -1);
			guardedHost(scheme, url.hostname, url.port);
			// The constructor is synchronous and cannot resolve first: names must be loopback or IP literals.
			const name = url.hostname.replace(/^\[|\]$/g, "");
			if (!isIP(name) && !isLoopback(name)) throw reported(new EgressRefused(`${scheme}://${name}`));
			return Reflect.construct(target, args, newTarget);
		},
	});
	const nativeConnect = Bun.connect;
	Reflect.set(Bun, "connect", async (options: Parameters<typeof Bun.connect>[0]) => {
		if (!("unix" in options) || options.unix === undefined) {
			const target: { hostname?: string; port?: number; tls?: unknown } = options;
			const scheme = target.tls ? "https" : "http";
			guardedHost(scheme, target.hostname, target.port);
			await resolveChecked(active ?? egress, target.hostname ?? "localhost", portOf(scheme, target.port));
		}
		return Reflect.apply(nativeConnect, Bun, [options]);
	});
	for (const name of ["connect", "createConnection"]) {
		wrapMethod(net, name, args => guardSocketArgs("http", args));
	}
	wrapMethod(tls, "connect", args => guardSocketArgs("https", args));
	for (const name of ["request", "get"]) {
		wrapMethod(http, name, args => guardHttpArgs("http", args));
		wrapMethod(https, name, args => guardHttpArgs("https", args));
	}
	return egress;
}
