// Wire-level stand-in for the daemon's credential owner and a loopback provider.
import type { AssistantMessage, Context } from "@oh-my-pi/pi-ai";
import type { CredentialKind, CredentialReply, CredentialResponse, OutcomeWire } from "../../src/protocol/types";
import type { CredentialRpc } from "../../src/providers/credential-rpc";
import { type CredentialLease, wrapCredentialRpcForTests } from "../../src/providers/credential-rpc";
import type { CredentialReportArgs, CredentialRequestArgs } from "../../src/transport/rpc";

export interface ReportRecord extends CredentialReportArgs {
	await: boolean;
}

/** Affinity, cooldown and invalidation like the daemon pool, keyed by one fake session. */
export class FakeCredentialDaemon {
	readonly accounts: string[];
	readonly invalid = new Set<string>();
	readonly cooldowns = new Map<string, number | undefined>();
	readonly affinity = new Map<string, string>();
	readonly requests: CredentialRequestArgs[] = [];
	readonly reports: ReportRecord[] = [];
	readonly issued: CredentialResponse[] = [];
	kind: CredentialKind = "api_key";
	upstreamAccountId: string | undefined;
	secretFor: (account: string) => string = account => account;
	#accountByLease = new Map<string, string>();

	constructor(count = 2) {
		this.accounts = Array.from({ length: count }, (_, index) => `fake-account-${index + 1}`);
	}

	async credentialRequest(args: CredentialRequestArgs, _signal: AbortSignal): Promise<CredentialReply> {
		this.requests.push(args);
		const candidates = this.accounts.filter(account => !this.invalid.has(account) && !this.cooldowns.has(account));
		const id = `rpc-${this.requests.length}`;
		if (candidates.length === 0) {
			return {
				v: 1,
				type: "credential_error",
				id,
				request_id: args.requestId,
				code: "auth_unavailable",
				reason: "all accounts exhausted",
				generation: 1,
			};
		}
		const key = `fake-session\u0000${args.provider}`;
		let account = this.affinity.get(key);
		if (account === undefined || !candidates.includes(account)) account = candidates[0] as string;
		this.affinity.set(key, account);
		const leaseId = `lease-${this.requests.length}`;
		this.#accountByLease.set(leaseId, account);
		const reply: CredentialResponse = {
			v: 1,
			type: "credential_response",
			id,
			request_id: args.requestId,
			lease_id: leaseId,
			generation: 1,
			account: { source: "vault", account_id: account },
			auth: { kind: this.kind, value: this.secretFor(account) },
		};
		if (this.upstreamAccountId !== undefined) reply.auth.upstream_account_id = this.upstreamAccountId;
		this.issued.push(reply);
		return reply;
	}

	accountOf(report: ReportRecord): string | undefined {
		return this.#accountByLease.get(report.lease.lease_id);
	}

	async credentialReport(
		args: CredentialReportArgs,
		opts: { await: boolean } = { await: true },
	): Promise<CredentialReply | undefined> {
		const record = { ...args, await: opts.await };
		this.reports.push(record);
		const account = this.accountOf(record) ?? "";
		const kind = args.outcome.outcome;
		if (kind === "auth") this.invalid.add(account);
		else if (kind === "rate_limit" || kind === "quota" || kind === "policy") {
			this.cooldowns.set(account, args.outcome.retry_after_ms);
		}
		if (!opts.await) return undefined;
		const rotate = !args.streamStarted && ["auth", "rate_limit", "quota", "policy"].includes(kind);
		return {
			v: 1,
			type: "credential_reported",
			id: `ack-${this.reports.length}`,
			request_id: args.requestId,
			lease_id: args.lease.lease_id,
			generation: 2,
			action: rotate ? "rotate" : "complete",
		};
	}

	outcomes(): OutcomeWire["outcome"][] {
		return this.reports.map(report => report.outcome.outcome);
	}
}

export interface LeaseCapture {
	leases: CredentialLease[];
	restore(): void;
}

/** Captures every lease the sidecar holds so tests can prove each secret was dropped. */
export function captureLeases(): LeaseCapture {
	const leases: CredentialLease[] = [];
	wrapCredentialRpcForTests(
		(rpc): CredentialRpc => ({
			acquire: async (...args) => {
				const lease = await rpc.acquire(...args);
				leases.push(lease);
				return lease;
			},
			report: (...args) => rpc.report(...args),
		}),
	);
	return { leases, restore: () => wrapCredentialRpcForTests(rpc => rpc) };
}

export interface CapturedRequest {
	url: string;
	headers: Record<string, string>;
	body: string;
}

export interface ProviderServer {
	url: string;
	seen: CapturedRequest[];
	stop(): void;
}

/** Loopback provider: `handler` answers each request; every request is recorded. */
export function serveProvider(
	handler: (request: CapturedRequest, index: number) => Response | Promise<Response>,
): ProviderServer {
	const seen: CapturedRequest[] = [];
	const server = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		idleTimeout: 0,
		async fetch(request) {
			const headers: Record<string, string> = {};
			request.headers.forEach((value, name) => {
				headers[name] = value;
			});
			const captured = {
				url: request.url,
				headers,
				body: await request.text(),
			};
			seen.push(captured);
			return handler(captured, seen.length - 1);
		},
	});
	return {
		url: `http://127.0.0.1:${server.port}`,
		seen,
		stop: () => server.stop(true),
	};
}

export function chatStream(text = "done"): Response {
	const chunks = [
		{
			id: "fake-chat",
			object: "chat.completion.chunk",
			created: 1,
			model: "gpt-5",
			choices: [
				{
					index: 0,
					delta: { role: "assistant", content: text },
					finish_reason: null,
				},
			],
		},
		{
			id: "fake-chat",
			object: "chat.completion.chunk",
			created: 1,
			model: "gpt-5",
			choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
		},
	];
	return new Response(`${chunks.map(chunk => `data: ${JSON.stringify(chunk)}\n\n`).join("")}data: [DONE]\n\n`, {
		headers: { "content-type": "text/event-stream" },
	});
}

export function userContext(text: string): Context {
	return { messages: [{ role: "user", content: text, timestamp: Date.now() }] };
}

export function textOf(message: AssistantMessage): string {
	return message.content.map(block => (block.type === "text" ? block.text : "")).join("");
}
