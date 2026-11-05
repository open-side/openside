// Runs in its own process: the guard patches process globals for good.
import http from "node:http";
import net from "node:net";
import { egressResolver, installEgress } from "../../../src/providers/egress";

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
const answers: Record<string, string> = { "rebind.example": "10.0.0.7", "lan.test": "127.0.0.1" };
const realLookup = egressResolver.lookup;
egressResolver.lookup = async host => {
	const address = answers[host];
	return address ? [{ address, family: 4 }] : realLookup(host);
};
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
console.log(JSON.stringify(results));
