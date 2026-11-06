/**
 * Image generation on a daemon credential lease. Like the chat driver it owns the
 * lease → request → report loop, so the daemon's account rotation, usage and blocks see
 * every image request; nothing is replayed once the provider answered with images.
 */
import { randomUUID } from "node:crypto";
import type { Api, FetchImpl, Model, Usage } from "@oh-my-pi/pi-ai";
import { generateImage, type ImageGenerationResult } from "@oh-my-pi/pi-ai/images";
import type { CredentialUsage } from "../protocol/types";
import { CURATED, metadata } from "./catalog";
import { type CredentialLease, type CredentialRpcPort, credentialRpcFor } from "./credential-rpc";
import { activeEgress, directFetch, EgressRefused, makeEgressFetch } from "./egress";
import { AuthUnavailable } from "./errors";
import { resolveModel, splitModelId } from "./models";
import { ClassifiedOutcome, classifyResponse } from "./outcomes";
import { makeCredentialFetch, requestApiKey } from "./request-auth";

const MAX_ATTEMPTS = 16;
/** pi-ai needs a non-empty key to build its client; the real lease is injected per request. */
const PLACEHOLDER_API_KEY = "daemon-managed";
const HOSTED_APIS: readonly string[] = ["openai-responses", "openai-codex-responses"];

export interface ImageJob {
	/** `provider:model` image generation id the daemon admitted. */
	model: string;
	prompt: string;
	aspectRatio?: string;
	count: number;
	subagent?: string;
	networkAllow: string[];
	sessionId?: string;
	signal: AbortSignal;
}

export type GenerateImage = typeof generateImage;

/** Hosted image models run as a Responses tool call carried by the provider's reasoning chat model. */
export function carrierFor(provider: string): Model<Api> | undefined {
	const name = (CURATED[provider] ?? []).find(candidate => metadata(candidate)[2] === "reasoning");
	if (name === undefined) return undefined;
	try {
		return resolveModel(`${provider}:${name}`);
	} catch {
		return undefined;
	}
}

function usageOf(usage: Usage | undefined): CredentialUsage | undefined {
	if (usage === undefined) return undefined;
	const { input, output, cacheRead, cacheWrite } = usage;
	if (!input && !output && !cacheRead && !cacheWrite) return undefined;
	return { input_tokens: input, output_tokens: output, cache_read_tokens: cacheRead, cache_write_tokens: cacheWrite };
}

export async function generateImages(
	port: CredentialRpcPort,
	job: ImageJob,
	generate: GenerateImage = generateImage,
): Promise<ImageGenerationResult> {
	const provider = splitModelId(job.model).provider;
	const model = resolveModel(job.model);
	const carrier = HOSTED_APIS.includes(model.api) ? carrierFor(provider) : undefined;
	if (HOSTED_APIS.includes(model.api) && carrier === undefined) {
		throw new Error("No chat model can carry this image model");
	}
	const endpoint = (carrier ?? model).baseUrl;
	const guard = activeEgress();
	if (guard) {
		for (const entry of job.networkAllow) guard.allow(entry);
		guard.allowEndpoint(endpoint);
	}
	const rpc = credentialRpcFor(port);
	const requestId = randomUUID();
	for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
		const lease: CredentialLease = await rpc.acquire(provider, job.model, requestId, attempt, job.subagent, job.signal);
		const credential = lease.credential;
		const report = (outcome: ClassifiedOutcome, usage?: CredentialUsage) =>
			rpc.report(lease, outcome, requestId, { streamStarted: false, usage, wait: outcome.kind !== "cancelled" });
		try {
			const target = credential.baseUrl ?? endpoint;
			if (credential.baseUrl !== undefined) guard?.allowEndpoint(credential.baseUrl);
			const apiKey = requestApiKey(credential, PLACEHOLDER_API_KEY);
			if (apiKey === undefined) {
				await report(credential.outcome);
				throw credential.refusal;
			}
			const fetch: FetchImpl = makeCredentialFetch(
				credential,
				target,
				makeEgressFetch(job.networkAllow, target, directFetch),
			);
			const routed = <T extends Model<Api>>(value: T): T =>
				credential.baseUrl === undefined ? value : { ...value, baseUrl: credential.baseUrl };
			let result: ImageGenerationResult;
			try {
				result = await generate(
					routed(model),
					{
						prompt: job.prompt,
						count: job.count,
						...(job.aspectRatio !== undefined ? { aspectRatio: job.aspectRatio } : {}),
					},
					{
						apiKey,
						fetch,
						signal: job.signal,
						...(carrier !== undefined ? { carrier: routed(carrier) } : {}),
						...(job.sessionId !== undefined ? { sessionId: job.sessionId } : {}),
					},
				);
			} catch (error) {
				if (job.signal.aborted) {
					void report(new ClassifiedOutcome("cancelled")).catch(() => undefined);
					throw error;
				}
				if (error instanceof EgressRefused || error instanceof AuthUnavailable || credential.refusal) {
					await report(credential.outcome);
					throw credential.refusal ?? error;
				}
				const status = (error as { status?: unknown } | null)?.status;
				const outcome =
					!credential.responded && typeof status === "number"
						? classifyResponse(status)
						: credential.outcome.kind === "success"
							? new ClassifiedOutcome("transient")
							: credential.outcome;
				if (await report(outcome)) continue;
				throw new Error(credential.notice ?? "Image provider request failed");
			}
			await report(new ClassifiedOutcome("success"), usageOf(result.usage));
			return result;
		} finally {
			credential.clear();
		}
	}
	throw new AuthUnavailable(provider, ["daemon credential rotation attempt limit reached"], {
		reason: "attempt_limit",
	});
}
