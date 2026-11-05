/**
 * `auth-login`: one interactive pi-ai OAuth login, driven by the daemon over
 * the private helper protocol (`protocol/auth-helper.ts`). No model loop, no
 * tools, no session or auth store: the grant is printed once on the
 * `credentials` line and the process exits. Provider error text never leaves
 * the process; failures map to fixed codes.
 */
import {
	getProviderDefinition,
	type OAuthAuthInfo,
	type OAuthController,
	type OAuthCredentials,
} from "@oh-my-pi/pi-ai";
import { isDefinitiveOAuthFailure, LoginCancelledError, OAuthError } from "@oh-my-pi/pi-ai/error";
import {
	type HelperErrorCode,
	type HelperInbound,
	type HelperIo,
	type HelperOutbound,
	LOGIN_ORIGINS,
	parseInbound,
	stdioHelperIo,
	toPayload,
} from "./protocol/auth-helper";
import { installEgress } from "./providers/egress";

export interface LoginDefinition {
	login?: (ctrl: OAuthController) => Promise<OAuthCredentials | string>;
}

/** Injection seam: tests supply a fake provider definition and never touch the network. */
export interface LoginDeps {
	definition(id: string): LoginDefinition | undefined;
	installEgress(allow: readonly string[]): void;
}

const defaultDeps: LoginDeps = {
	definition: getProviderDefinition,
	installEgress: allow => {
		installEgress(allow);
	},
};

const USER_CODE = /^Enter code:\s*(\S{1,64})$/;
const ADDRESS_IN_USE = /EADDRINUSE|port \d+ is in use/i;
const TIMED_OUT = /TimeoutError|timed out/i;

class UnsupportedFlow extends Error {
	override readonly name = "UnsupportedFlow";
}

function causeChain(error: unknown): unknown[] {
	const chain: unknown[] = [];
	for (let current = error; current !== undefined && chain.length < 8; ) {
		chain.push(current);
		current = current instanceof Error ? current.cause : undefined;
	}
	return chain;
}

/** Fixed code for a failed login; messages are inspected, never forwarded. */
export function classifyLoginError(error: unknown): HelperErrorCode {
	const chain = causeChain(error);
	const busy = chain.some(
		item =>
			(item as { code?: unknown } | null)?.code === "EADDRINUSE" ||
			(item instanceof Error && ADDRESS_IN_USE.test(item.message)),
	);
	if (busy) return "callback_busy";
	if (chain.some(item => item instanceof UnsupportedFlow)) return "unsupported";
	const timedOut = chain.some(
		item =>
			(item instanceof OAuthError && item.kind === "timeout") ||
			(item instanceof Error && item.name === "TimeoutError"),
	);
	if (timedOut) return "timeout";
	// pi-ai's callback wait ends on its own deadline with a cancellation naming the timeout.
	if (error instanceof LoginCancelledError) return TIMED_OUT.test(error.message) ? "timeout" : "cancelled";
	if (error instanceof Error && error.name === "AbortError") return "cancelled";
	if (error instanceof Error && isDefinitiveOAuthFailure(error.message)) return "invalid_grant";
	return "failed";
}

function authEvent(info: OAuthAuthInfo): HelperOutbound {
	const event: HelperOutbound & { type: "auth" } = { type: "auth", url: info.url };
	if (info.launchUrl) event.launch_url = info.launchUrl;
	if (info.instructions) {
		event.instructions = info.instructions;
		const code = USER_CODE.exec(info.instructions.trim())?.[1];
		if (code) event.user_code = code;
	}
	return event;
}

export async function authLogin(io: HelperIo = stdioHelperIo(), deps: LoginDeps = defaultDeps): Promise<number> {
	const fail = (code: HelperErrorCode): number => {
		try {
			io.write({ type: "error", code });
		} catch {
			// Nothing else can be reported; the exit status still says failure.
		}
		return 1;
	};
	let first: HelperInbound | undefined;
	try {
		const next = await io.lines.next();
		if (next.done) return fail("cancelled");
		first = parseInbound(next.value);
	} catch {
		return fail("failed");
	}
	if (first?.type !== "login") return fail("failed");
	const origins = Object.hasOwn(LOGIN_ORIGINS, first.provider) ? LOGIN_ORIGINS[first.provider] : undefined;
	const login = origins && deps.definition(first.provider)?.login;
	if (!origins || !login) return fail("unsupported");
	deps.installEgress(origins);

	const abort = new AbortController();
	const terminal = Promise.withResolvers<HelperErrorCode>();
	let stopped: HelperErrorCode | undefined;
	const stop = (code: HelperErrorCode): void => {
		if (stopped !== undefined) return;
		stopped = code;
		terminal.resolve(code);
		abort.abort();
	};
	const send = (message: HelperOutbound): void => {
		try {
			io.write(message);
		} catch {
			stop("failed");
		}
	};
	let manual: PromiseWithResolvers<string> | undefined;
	let manualRequested = false;

	void (async () => {
		try {
			for (;;) {
				const next = await io.lines.next();
				if (next.done) return stop("cancelled");
				const message = parseInbound(next.value);
				if (message?.type === "cancel") return stop("cancelled");
				if (message?.type !== "manual_input" || !manualRequested) return stop("failed");
				// Input that races a browser callback finds no pending request and is dropped.
				const pending = manual;
				manual = undefined;
				pending?.resolve(message.value);
			}
		} catch {
			stop("failed");
		}
	})();

	const ctrl: OAuthController = {
		signal: abort.signal,
		onAuth: info => send(authEvent(info)),
		onProgress: message => {
			if (message.startsWith("Exchanging")) send({ type: "progress", stage: "exchanging" });
		},
		onManualCodeInput: signal => {
			if (signal?.aborted || abort.signal.aborted) return Promise.reject(new LoginCancelledError());
			manualRequested = true;
			const pending = Promise.withResolvers<string>();
			manual?.reject(new LoginCancelledError());
			manual = pending;
			const withdraw = (): void => {
				if (manual === pending) manual = undefined;
				pending.reject(new LoginCancelledError());
			};
			signal?.addEventListener("abort", withdraw, { once: true });
			abort.signal.addEventListener("abort", withdraw, { once: true });
			send({ type: "manual_input_wanted" });
			return pending.promise;
		},
		onPrompt: async prompt => {
			// Copilot's enterprise-domain prompt: empty selects github.com.
			if (prompt.allowEmpty) return "";
			stop("unsupported");
			throw new UnsupportedFlow();
		},
		onBrowserSession: async () => {
			stop("unsupported");
			throw new UnsupportedFlow();
		},
	};

	const outcome = await Promise.race([
		login(ctrl).then(
			result => ({ result }),
			(error: unknown) => ({ error }),
		),
		terminal.promise.then(code => ({ code })),
	]);
	if (stopped !== undefined) return fail(stopped);
	if ("code" in outcome) return fail(outcome.code);
	if ("error" in outcome) return fail(classifyLoginError(outcome.error));
	// A plain string is an API key, which this registration path never stores.
	const payload = typeof outcome.result === "string" ? undefined : toPayload(outcome.result);
	if (!payload) return fail("failed");
	try {
		io.write({ type: "credentials", credentials: payload });
	} catch {
		return fail("failed");
	}
	return 0;
}
