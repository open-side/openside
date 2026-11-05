import { writeSync } from "node:fs";
import type { OAuthCredentials } from "@oh-my-pi/pi-ai";

/**
 * Private stdio protocol of the one-shot OAuth helpers (`auth-login`,
 * `auth-refresh`) the daemon spawns. JSONL, one object per line, at most
 * `MAX_HELPER_LINE_BYTES` bytes; anything else is a protocol error. Only the
 * `credentials` line ever carries token material.
 */

export const MAX_HELPER_LINE_BYTES = 65_536;
const MAX_MANUAL_INPUT_CHARS = 4_096;
const PROVIDER_ID = /^[a-z0-9-]{1,64}$/;

const GOOGLE_OAUTH_ORIGINS = ["https://oauth2.googleapis.com", "https://www.googleapis.com"];
const ANTHROPIC_ORIGINS = ["https://api.anthropic.com"];
const CODEX_ORIGINS = ["https://auth.openai.com"];
const GEMINI_CLI_ORIGINS = [...GOOGLE_OAUTH_ORIGINS, "https://cloudcode-pa.googleapis.com"];
const ANTIGRAVITY_ORIGINS = [...GOOGLE_OAUTH_ORIGINS, "https://daily-cloudcode-pa.googleapis.com"];
const COPILOT_ORIGINS = [
	"https://github.com",
	"https://api.github.com",
	"https://api.githubcopilot.com",
	"https://*.githubcopilot.com",
];

/**
 * First-release pi-ai login ids and the only origins their login may reach
 * (token, userinfo and project endpoints from pi-ai's auth rules and flow
 * modules); loopback stays reachable for the flow's own callback server.
 * Copilot is github.com only: the enterprise prompt is answered empty.
 */
export const LOGIN_ORIGINS: Readonly<Record<string, readonly string[]>> = {
	anthropic: ANTHROPIC_ORIGINS,
	"openai-codex": CODEX_ORIGINS,
	"openai-codex-device": CODEX_ORIGINS,
	"google-gemini-cli": GEMINI_CLI_ORIGINS,
	"google-antigravity": ANTIGRAVITY_ORIGINS,
	"github-copilot": COPILOT_ORIGINS,
};

/** Storage providers a stored grant may be refreshed for; device logins store as `openai-codex`. */
export const REFRESH_ORIGINS: Readonly<Record<string, readonly string[]>> = {
	anthropic: ANTHROPIC_ORIGINS,
	"openai-codex": CODEX_ORIGINS,
	"google-gemini-cli": GEMINI_CLI_ORIGINS,
	"google-antigravity": ANTIGRAVITY_ORIGINS,
	"github-copilot": COPILOT_ORIGINS,
};

/** Stored grant, snake_case; identity fields (email, org, plan) never cross this boundary. */
export interface OAuthPayload {
	access: string;
	refresh: string;
	expires: number;
	project_id?: string;
	account_id?: string;
	enterprise_url?: string;
	api_endpoint?: string;
}

const OPTIONAL_PAYLOAD_FIELDS = ["project_id", "account_id", "enterprise_url", "api_endpoint"] as const;

export type HelperErrorCode = "callback_busy" | "cancelled" | "timeout" | "invalid_grant" | "unsupported" | "failed";

export type HelperInbound =
	| { type: "login"; provider: string }
	| { type: "manual_input"; value: string }
	| { type: "cancel" }
	| { type: "refresh"; provider: string; credentials: OAuthPayload };

export type HelperOutbound =
	| { type: "auth"; url: string; launch_url?: string; instructions?: string; user_code?: string }
	| { type: "manual_input_wanted" }
	| { type: "progress"; stage: "exchanging" }
	| { type: "credentials"; credentials: OAuthPayload }
	| { type: "error"; code: HelperErrorCode };

export interface HelperIo {
	/** Inbound lines without their newline; throws on an oversized, unterminated or non-UTF-8 line. */
	lines: AsyncIterator<string>;
	write(message: HelperOutbound): void;
}

export class HelperProtocolError extends Error {
	override readonly name = "HelperProtocolError";

	constructor() {
		super("Invalid auth helper protocol line");
	}
}

function record(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
	const present = Object.keys(value);
	return present.length === keys.length && present.every(key => keys.includes(key));
}

/** A strict `OAuthPayload`, or undefined; unknown keys are rejected. */
export function parsePayload(value: unknown): OAuthPayload | undefined {
	if (!record(value)) return undefined;
	const allowed: readonly string[] = ["access", "refresh", "expires", ...OPTIONAL_PAYLOAD_FIELDS];
	if (!Object.keys(value).every(key => allowed.includes(key))) return undefined;
	const { access, refresh, expires } = value;
	if (typeof access !== "string" || access === "" || typeof refresh !== "string") return undefined;
	if (typeof expires !== "number" || !Number.isSafeInteger(expires) || expires < 0) return undefined;
	const payload: OAuthPayload = { access, refresh, expires };
	for (const field of OPTIONAL_PAYLOAD_FIELDS) {
		const text = value[field];
		if (text === undefined) continue;
		if (typeof text !== "string") return undefined;
		payload[field] = text;
	}
	return payload;
}

/**
 * pi-ai credentials → stored payload. Only request-relevant fields are kept
 * (OAUTH-5): email, org, plan and region never leave the helper. Returns
 * undefined when the provider produced no usable grant.
 */
export function toPayload(credentials: OAuthCredentials): OAuthPayload | undefined {
	return parsePayload({
		access: credentials.access,
		refresh: credentials.refresh,
		expires: Number.isFinite(credentials.expires) ? Math.max(0, Math.floor(credentials.expires)) : undefined,
		project_id: credentials.projectId || undefined,
		account_id: credentials.accountId || undefined,
		enterprise_url: credentials.enterpriseUrl || undefined,
		api_endpoint: credentials.apiEndpoint || undefined,
	});
}

export function fromPayload(payload: OAuthPayload): OAuthCredentials {
	return {
		access: payload.access,
		refresh: payload.refresh,
		expires: payload.expires,
		projectId: payload.project_id,
		accountId: payload.account_id,
		enterpriseUrl: payload.enterprise_url,
		apiEndpoint: payload.api_endpoint,
	};
}

/** One validated inbound line, or undefined for any protocol error (the line is never echoed). */
export function parseInbound(line: string): HelperInbound | undefined {
	let value: unknown;
	try {
		value = JSON.parse(line);
	} catch {
		return undefined;
	}
	if (!record(value)) return undefined;
	switch (value.type) {
		case "login":
			return exactKeys(value, ["type", "provider"]) &&
				typeof value.provider === "string" &&
				PROVIDER_ID.test(value.provider)
				? { type: "login", provider: value.provider }
				: undefined;
		case "manual_input":
			return exactKeys(value, ["type", "value"]) &&
				typeof value.value === "string" &&
				value.value.length >= 1 &&
				value.value.length <= MAX_MANUAL_INPUT_CHARS
				? { type: "manual_input", value: value.value }
				: undefined;
		case "cancel":
			return exactKeys(value, ["type"]) ? { type: "cancel" } : undefined;
		case "refresh": {
			if (!exactKeys(value, ["type", "provider", "credentials"])) return undefined;
			if (typeof value.provider !== "string" || !PROVIDER_ID.test(value.provider)) return undefined;
			const credentials = parsePayload(value.credentials);
			return credentials ? { type: "refresh", provider: value.provider, credentials } : undefined;
		}
		default:
			return undefined;
	}
}

/** Serializes one outbound line; a line over the cap is a protocol error, never truncated. */
export function encodeLine(message: HelperOutbound): string {
	const line = `${JSON.stringify(message)}\n`;
	if (Buffer.byteLength(line) > MAX_HELPER_LINE_BYTES) throw new HelperProtocolError();
	return line;
}

/** Splits a byte stream into lines of at most `maxBytes` bytes (newline excluded). */
export async function* readLines(
	stream: ReadableStream<Uint8Array>,
	maxBytes = MAX_HELPER_LINE_BYTES,
): AsyncGenerator<string> {
	const decoder = new TextDecoder("utf-8", { fatal: true });
	let pending: Uint8Array[] = [];
	let size = 0;
	for await (const chunk of stream) {
		let start = 0;
		for (let end = chunk.indexOf(10); end !== -1; end = chunk.indexOf(10, start)) {
			const part = chunk.subarray(start, end);
			if (size + part.length > maxBytes) throw new HelperProtocolError();
			const line = pending.length === 0 ? part : Buffer.concat([...pending, part]);
			pending = [];
			size = 0;
			start = end + 1;
			yield decoder.decode(line);
		}
		const rest = chunk.subarray(start);
		size += rest.length;
		if (size > maxBytes) throw new HelperProtocolError();
		// The stream may reuse its chunk buffer, so a partial line is copied.
		if (rest.length > 0) pending.push(rest.slice());
	}
	if (size > 0) throw new HelperProtocolError();
}

/**
 * The daemon's pipes. Writes are synchronous so every line is on stdout before
 * the helper exits, in order.
 */
export function stdioHelperIo(): HelperIo {
	return {
		lines: readLines(Bun.stdin.stream()),
		write: message => {
			const bytes = Buffer.from(encodeLine(message));
			for (let offset = 0; offset < bytes.length; ) offset += writeSync(1, bytes, offset);
		},
	};
}
