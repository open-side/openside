/**
 * The custom pi-agent-core `StreamFn` that owns daemon credential leases (D2).
 *
 * pi-ai's `ApiKeyResolver` only runs on auth-class failures, but the daemon
 * tracks affinity, usage and blocks from every outcome, so this driver owns the
 * per-request attempt loop: lease → `streamSimple` → report. A request is
 * replayed on a new lease only while nothing has reached the agent and only
 * when the daemon answers `refresh`/`rotate`. Provider-internal transient
 * retries stay enabled (D6), bounded by `maxRetryDelayMs`.
 */
import { randomUUID } from "node:crypto";
import type { StreamFn } from "@oh-my-pi/pi-agent-core";
import {
	type Api,
	type AssistantMessage,
	type AssistantMessageEvent,
	type Context,
	type FetchImpl,
	type Model,
	type SimpleStreamOptions,
	streamSimple,
} from "@oh-my-pi/pi-ai";
import { AssistantMessageEventStream } from "@oh-my-pi/pi-ai/utils/event-stream";
import { logRequestIdentity } from "../features/prompt-epoch";
import type { CredentialUsage } from "../protocol/types";
import { applyClaudeCodeVersion } from "./anthropic";
import { type CredentialLease, type CredentialRpcPort, credentialRpcFor } from "./credential-rpc";
import { activeEgress, anthropicDirectFetch, directFetch, Egress, EgressRefused, makeEgressFetch } from "./egress";
import { AuthUnavailable } from "./errors";
import { resolveModel, sanitizedErrorMessage, splitModelId } from "./models";
import { ClassifiedOutcome, classifyResponse } from "./outcomes";
import { type CredentialAttempt, makeCredentialFetch } from "./request-auth";

export interface LeaseContext {
	/** Credential provider: the prefix of `model`. */
	provider: string;
	/** Full `provider:model` identifier sent in `credential_request.model`. */
	model: string;
	subagent?: string;
	networkAllow: string[];
	claudeCodeVersion?: string;
	/** Explicit fallback identifiers, tried in order after a non-auth failure before any output. */
	fallback?: string[];
	/** `open.config.agent.provider_retry_delay_ms`; caps provider-internal retry waits. */
	maxRetryDelayMs?: number;
}

const MAX_ATTEMPTS = 64;
const DEFAULT_MAX_RETRY_DELAY_MS = 5_000;
const DAEMON_OUTCOMES: Record<string, true> = {
	auth: true,
	rate_limit: true,
	quota: true,
	policy: true,
};
/** pi-ai needs a non-empty key to build its client; the real lease is injected per request. */
const PLACEHOLDER_API_KEY = "daemon-managed";

interface Candidate {
	provider: string;
	id: string;
	model: Model<Api>;
}

type AttemptResult =
	| { kind: "visible"; stream: AssistantMessageEventStream }
	| { kind: "retry" }
	| { kind: "failed"; error: AssistantMessage };

type Terminal = Extract<AssistantMessageEvent, { type: "done" | "error" }>;

function streamOf(events: readonly AssistantMessageEvent[]): AssistantMessageEventStream {
	const stream = new AssistantMessageEventStream();
	for (const event of events) stream.push(event);
	stream.end();
	return stream;
}

function emptyMessage(model: Model<Api>, stopReason: "aborted" | "error"): AssistantMessage {
	return {
		role: "assistant",
		content: [],
		api: model.api,
		provider: model.provider,
		model: model.id,
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason,
		errorMessage:
			stopReason === "aborted" ? "Request was aborted" : "Provider request failed; response details redacted",
		timestamp: Date.now(),
	};
}

function sanitized(error: AssistantMessage, streamStarted: boolean): AssistantMessage {
	const { errorClassificationMessage: _dropped, ...rest } = error;
	return { ...rest, errorMessage: sanitizedErrorMessage(error, streamStarted) };
}

function usageOf(message: AssistantMessage): CredentialUsage | undefined {
	const { input, output, cacheRead, cacheWrite } = message.usage;
	if (!input && !output && !cacheRead && !cacheWrite) return undefined;
	return {
		input_tokens: input,
		output_tokens: output,
		cache_read_tokens: cacheRead,
		cache_write_tokens: cacheWrite,
	};
}

/** The failure class of one attempt, from the response the transport saw or the surfaced status. */
function failureOutcome(attempt: CredentialAttempt, error: AssistantMessage): ClassifiedOutcome {
	if (!attempt.responded && error.errorStatus !== undefined) return classifyResponse(error.errorStatus);
	return attempt.outcome.kind === "success" ? new ClassifiedOutcome("transient") : attempt.outcome;
}

function baseFetch(model: Model<Api>): FetchImpl {
	// Mirrors pi-ai's own transport choice so first-party Anthropic keeps its TLS profile.
	return model.provider === "anthropic" && model.api === "anthropic-messages" ? anthropicDirectFetch : directFetch;
}

export function makeStreamFn(port: CredentialRpcPort, ctx: LeaseContext): StreamFn {
	// Invalid allow-list entries fail the session open, not the first request.
	new Egress(ctx.networkAllow);
	const admit = (endpoint: string): void => {
		const guard = activeEgress();
		if (!guard) return;
		for (const entry of ctx.networkAllow) guard.allow(entry);
		guard.allowEndpoint(endpoint);
	};
	applyClaudeCodeVersion(ctx.claudeCodeVersion);
	const rpc = credentialRpcFor(port);
	const maxRetryDelayMs = ctx.maxRetryDelayMs ?? DEFAULT_MAX_RETRY_DELAY_MS;
	const neverAborted = new AbortController().signal;
	// Last account digest per model id, so prompt-cache misses can be told apart from rotations.
	const lastAccount = new Map<string, string>();

	const runAttempt = async (
		candidate: Candidate,
		lease: CredentialLease,
		requestId: string,
		context: Context,
		options: SimpleStreamOptions | undefined,
	): Promise<AttemptResult> => {
		const attempt = lease.credential;
		const model =
			candidate.provider === "anthropic" && candidate.model.api === "anthropic-messages"
				? { ...candidate.model, isOAuth: attempt.kind === "claude_code" }
				: candidate.model;
		// pi transports retry 408/429/5xx on the same key. A pool-level outcome belongs
		// to the daemon, which rotates accounts instead of sleeping on this one.
		const handoff = new AbortController();
		const credentialFetch = makeCredentialFetch(
			attempt,
			model.baseUrl,
			makeEgressFetch(ctx.networkAllow, model.baseUrl, baseFetch(model)),
		);
		const fetch: FetchImpl = async (input, init) => {
			let response: Response;
			try {
				response = await credentialFetch(input, init);
			} catch (error) {
				// A refusal is final for this lease; a transport retry would only repeat it.
				if (error instanceof EgressRefused || error instanceof AuthUnavailable) handoff.abort();
				throw error;
			}
			if (Object.hasOwn(DAEMON_OUTCOMES, attempt.outcome.kind)) handoff.abort();
			return response;
		};
		const report = (outcome: ClassifiedOutcome, streamStarted: boolean, usage?: CredentialUsage) =>
			rpc.report(lease, outcome, requestId, {
				streamStarted,
				usage,
				wait: outcome.kind !== "cancelled",
			});
		const callerAborted = (): boolean => options?.signal?.aborted === true;
		const inner = streamSimple(model, context, {
			...options,
			signal: options?.signal ? AbortSignal.any([options.signal, handoff.signal]) : handoff.signal,
			apiKey: PLACEHOLDER_API_KEY,
			fetch,
			preferWebsockets: false,
			maxRetryDelayMs,
		});
		const events = inner[Symbol.asyncIterator]();
		const buffered: AssistantMessageEvent[] = [];
		let first: AssistantMessageEvent | undefined;
		try {
			for (;;) {
				const next = await events.next();
				if (next.done) break;
				if (next.value.type === "start") {
					buffered.push(next.value);
					continue;
				}
				first = next.value;
				break;
			}
		} catch {
			// A rejected iterator is handled like a terminal error below; an abort
			// there becomes a cancelled report, never a transient failure.
			first = undefined;
		}

		if (first === undefined || first.type === "error") {
			const error =
				first?.type === "error" ? first.error : emptyMessage(model, options?.signal?.aborted ? "aborted" : "error");
			try {
				if (options?.signal?.aborted || (error.stopReason === "aborted" && !handoff.signal.aborted)) {
					void report(new ClassifiedOutcome("cancelled"), false).catch(() => undefined);
					return {
						kind: "visible",
						stream: streamOf([...buffered, { type: "error", reason: "aborted", error }]),
					};
				}
				const retry = await report(failureOutcome(attempt, error), false);
				if (attempt.refusal) throw attempt.refusal;
				return retry ? { kind: "retry" } : { kind: "failed", error: sanitized(error, false) };
			} finally {
				attempt.clear();
			}
		}

		const outer = new AssistantMessageEventStream();
		for (const event of buffered) outer.push(event);
		// The terminal event is held back until the daemon acknowledged the
		// report, so the next request's lease never overtakes this one's outcome.
		const finish = async (terminal: Terminal | undefined): Promise<void> => {
			try {
				if (terminal?.type === "done") {
					await report(new ClassifiedOutcome("success"), true, usageOf(terminal.message));
					outer.push(terminal);
				} else if (terminal?.type === "error" && (terminal.error.stopReason === "aborted" || callerAborted())) {
					void report(new ClassifiedOutcome("cancelled"), true, usageOf(terminal.error)).catch(() => undefined);
					outer.push(terminal);
				} else {
					const error = terminal?.type === "error" ? terminal.error : emptyMessage(model, "error");
					await report(failureOutcome(attempt, error), true, usageOf(error));
					outer.push({
						type: "error",
						reason: "error",
						error: sanitized(error, true),
					});
				}
			} catch {
				// The daemon refused or dropped the report after output was visible; the
				// request can no longer replay, so it ends as an interrupted stream.
				const base =
					terminal === undefined
						? emptyMessage(model, "error")
						: terminal.type === "done"
							? terminal.message
							: terminal.error;
				outer.push({
					type: "error",
					reason: "error",
					error: { ...sanitized(base, true), stopReason: "error" },
				});
			} finally {
				attempt.clear();
				outer.end();
			}
		};
		if (first.type === "done") {
			void finish(first);
			return { kind: "visible", stream: outer };
		}
		outer.push(first);
		void (async () => {
			let terminal: Terminal | undefined;
			try {
				for (;;) {
					const next = await events.next();
					if (next.done) break;
					if (next.value.type === "done" || next.value.type === "error") {
						terminal = next.value;
						break;
					}
					outer.push(next.value);
				}
			} catch {
				// A rejected iterator after an abort is a cancellation, reported below.
				terminal = undefined;
			}
			if (terminal === undefined && callerAborted()) {
				terminal = { type: "error", reason: "aborted", error: emptyMessage(model, "aborted") };
			}
			await finish(terminal);
		})();
		return { kind: "visible", stream: outer };
	};

	const runCandidate = async (
		candidate: Candidate,
		context: Context,
		options: SimpleStreamOptions | undefined,
	): Promise<AttemptResult> => {
		const requestId = randomUUID();
		const signal = options?.signal ?? neverAborted;
		for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
			let lease: CredentialLease;
			try {
				lease = await rpc.acquire(candidate.provider, candidate.id, requestId, attempt, ctx.subagent, signal);
			} catch (error) {
				if (signal.aborted) {
					const aborted = emptyMessage(candidate.model, "aborted");
					return {
						kind: "visible",
						stream: streamOf([{ type: "error", reason: "aborted", error: aborted }]),
					};
				}
				throw error;
			}
			lastAccount.set(
				candidate.id,
				logRequestIdentity(candidate.provider, candidate.id, lease.accountId, lastAccount.get(candidate.id)),
			);
			const result = await runAttempt(candidate, lease, requestId, context, options);
			if (result.kind !== "retry") return result;
		}
		throw new AuthUnavailable(candidate.provider, ["daemon credential rotation attempt limit reached"], {
			reason: "attempt_limit",
		});
	};

	return async (model, context, options) => {
		if (model.api === "mock") return streamSimple(model, context, options);
		const candidates: (() => Candidate)[] = [
			() => ({ provider: ctx.provider, id: ctx.model, model }),
			...(ctx.fallback ?? []).map(id => () => ({
				provider: splitModelId(id).provider,
				id,
				model: resolveModel(id),
			})),
		];
		let failure: AssistantMessage | undefined;
		for (const resolve of candidates) {
			const candidate = resolve();
			admit(candidate.model.baseUrl);
			const result = await runCandidate(candidate, context, options);
			if (result.kind === "visible") return result.stream;
			if (result.kind === "failed") failure = result.error;
		}
		const error = failure ?? emptyMessage(model, "error");
		return streamOf([{ type: "error", reason: "error", error }]);
	};
}
