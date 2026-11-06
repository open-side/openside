// Runs in its own process: the guard patches process globals for good.
import http from "node:http";
import https from "node:https";
import net from "node:net";
import tls from "node:tls";
import { anthropicDirectFetch, egressResolver, installEgress, makeEgressFetch } from "../../../src/providers/egress";

const outcome = async (run: () => unknown): Promise<string> => {
	try {
		await run();
		return "allowed";
	} catch (error) {
		return error instanceof Error ? error.name : "error";
	}
};

/** Node sockets report refusals from the guarded lookup as an `error` event. */
function socketOutcome(
	open: () => { once(event: "error", listener: (error: Error) => void): unknown },
): Promise<string> {
	const { promise, resolve } = Promise.withResolvers<string>();
	try {
		open().once("error", error => resolve(error.name));
	} catch (error) {
		resolve(error instanceof Error ? error.name : "error");
	}
	return promise;
}

const server = Bun.serve({
	hostname: "127.0.0.1",
	port: 0,
	fetch: request => new Response(`host=${request.headers.get("host")}`),
});
// anthropic.test answers with a TEST-NET address and anthropic6.test with an IPv6
// documentation address; mapped.example answers an IPv4-mapped form of a private address
// through a wildcard entry.
const answers: Record<string, { address: string; family: 4 | 6 }> = {
	"rebind.example": { address: "10.0.0.7", family: 4 },
	"lan.test": { address: "127.0.0.1", family: 4 },
	"anthropic.test": { address: "192.0.2.1", family: 4 },
	"anthropic6.test": { address: "2001:db8::1", family: 6 },
	"mapped.example": { address: "::ffff:10.0.0.7", family: 6 },
};
const realLookup = egressResolver.lookup;
egressResolver.lookup = async host => {
	const answer = answers[host];
	return answer ? [answer] : realLookup(host);
};
const pinnedAnthropic = (origin: string, url = `${origin}/v1/messages`, allow: string[] = []) =>
	makeEgressFetch(
		allow,
		origin,
		anthropicDirectFetch,
	)(url, {
		method: "POST",
		headers: {},
		body: "{}",
	});
// The TLS dial beneath the guard (`coworkFetch` reaches it through `https.request`'s agent):
// installed before `installEgress` wraps it, it sees only what the guard let through. It
// records the exact target and throws instead of opening a socket, so a pinned case proves it
// reached the checked address and nothing else.
const dials: string[] = [];
Reflect.set(tls, "connect", (target: { host?: string; hostname?: string; port?: number | string }) => {
	dials.push(`${target.host ?? target.hostname}:${target.port}`);
	throw Object.assign(new Error("dial recorded"), { name: "DialRecorded" });
});

// Only a wildcard admits rebind.example, so its internal answers are refused;
// lan.test is listed exactly, so the guard dials its loopback answer.
const guard = installEgress(["allowed.invalid", "*.example", "lan.test"]);
const results: Record<string, string> = {
	fetchRefused: await outcome(() => fetch("https://collector.invalid/steal")),
	fetchLoopback: await outcome(async () => (await fetch(`http://127.0.0.1:${server.port}/`)).text()),
	// The allow-listed host passes the guard; only name resolution fails.
	fetchAllowed: await outcome(() => fetch("http://allowed.invalid/")),
	webSocket: await outcome(() => new WebSocket("wss://collector.invalid/socket")),
	bunConnect: await outcome(() => Bun.connect({ hostname: "collector.invalid", port: 443, socket: { data() {} } })),
	netConnect: await outcome(() => net.connect(443, "collector.invalid")),
	fetchRebind: await outcome(() => fetch("https://rebind.example/")),
	bunConnectRebind: await outcome(() => Bun.connect({ hostname: "rebind.example", port: 443, socket: { data() {} } })),
	netConnectRebind: await socketOutcome(() => net.connect({ host: "rebind.example", port: 443 })),
	httpRebind: await socketOutcome(() => http.get("http://rebind.example/")),
	webSocketRebind: await outcome(() => new WebSocket("wss://rebind.example/socket")),
	fetchPinned: await (async () => {
		try {
			return await (await fetch(`http://lan.test:${server.port}/`)).text();
		} catch (error) {
			return error instanceof Error ? error.name : "error";
		}
	})(),
	anthropicPinned: await outcome(() => pinnedAnthropic("https://anthropic.test")),
	anthropicPinnedV6: await outcome(() => pinnedAnthropic("https://anthropic6.test")),
	// A wildcard-admitted name whose answer is a mapped private address never gets pinned.
	anthropicMappedPrivate: await outcome(() =>
		pinnedAnthropic("https://api.anthropic.com", "https://mapped.example/v1/messages", ["*.example"]),
	),
	// The exemption covers only the pinned dispatch, never a later direct dial of that address
	// or another spelling of it.
	httpsLiteral: await socketOutcome(() => https.get("https://192.0.2.1/")),
	httpsLiteralV6: await socketOutcome(() => https.get("https://[2001:db8::1]/")),
	httpsMapped: await socketOutcome(() => https.get("https://[::ffff:192.0.2.1]/")),
	httpsMappedPrivate: await socketOutcome(() => https.get("https://[::ffff:10.0.0.7]/")),
};
guard.allowEndpoint("https://collector.invalid/v1");
results.fetchAdmitted = await outcome(async () => {
	try {
		await fetch("https://collector.invalid/v1");
	} catch (error) {
		if (error instanceof Error && error.name === "EgressRefused") throw error;
	}
});
server.stop(true);
results.dials = dials.join(" ");
console.log(JSON.stringify(results));
