/** `provider:model` identifiers → pi-catalog models, plus provider error sanitization. */
import type { Api, AssistantMessage, Model } from "@oh-my-pi/pi-ai";
import { getBundledModel, getBundledModels, isGeneratedProvider } from "@oh-my-pi/pi-catalog";
import { buildModel } from "@oh-my-pi/pi-catalog/build";
import { AuthUnavailable } from "./errors";

interface ProviderRoute {
	/** pi-catalog provider id; the credential provider stays the identifier prefix. */
	provider: string;
	api: Api;
	baseUrl: string;
}

/**
 * Identifier prefixes that are not plain pi-catalog providers, or whose default
 * wire for unbundled model ids must be pinned.
 */
const ROUTES: Record<string, ProviderRoute> = {
	anthropic: {
		provider: "anthropic",
		api: "anthropic-messages",
		baseUrl: "https://api.anthropic.com",
	},
	openai: {
		provider: "openai",
		api: "openai-responses",
		baseUrl: "https://api.openai.com/v1",
	},
	"openai-responses": {
		provider: "openai",
		api: "openai-responses",
		baseUrl: "https://api.openai.com/v1",
	},
	"openai-chat": {
		provider: "openai",
		api: "openai-completions",
		baseUrl: "https://api.openai.com/v1",
	},
	"openai-codex": {
		provider: "openai-codex",
		api: "openai-codex-responses",
		baseUrl: "https://chatgpt.com/backend-api",
	},
	google: {
		provider: "google",
		api: "google-generative-ai",
		baseUrl: "https://generativelanguage.googleapis.com/v1beta",
	},
	"github-copilot": {
		provider: "github-copilot",
		api: "openai-completions",
		baseUrl: "https://api.githubcopilot.com",
	},
};

const baseUrlOverrides = new Map<string, string>();

/**
 * Test hook for the e2e preload: points every model of `provider` at a local
 * endpoint. Only `main.ts` calls it, and only with a nonce-bound preload.
 */
export function setProviderBaseUrlForTests(provider: string, endpoint: string): void {
	baseUrlOverrides.set(provider, endpoint);
}

export function splitModelId(id: string): { provider: string; name: string } {
	const separator = id.indexOf(":");
	const provider = separator < 0 ? "" : id.slice(0, separator);
	const name = separator < 0 ? "" : id.slice(separator + 1);
	if (!provider || !name) throw new AuthUnavailable("model", ["expected an explicit provider:model identifier"]);
	return { provider, name };
}

function bundledRoute(provider: string): ProviderRoute | undefined {
	if (!isGeneratedProvider(provider)) return undefined;
	const models = getBundledModels(provider);
	const counts = new Map<Api, number>();
	for (const model of models) counts.set(model.api, (counts.get(model.api) ?? 0) + 1);
	let best: Model<Api> | undefined;
	for (const model of models) {
		if (!best || (counts.get(model.api) ?? 0) > (counts.get(best.api) ?? 0)) best = model;
	}
	return best && { provider, api: best.api, baseUrl: best.baseUrl };
}

/** Whether `provider` can be resolved to a request wire without credentials. */
export function isSupportedProvider(provider: string): boolean {
	return Object.hasOwn(ROUTES, provider) || bundledRoute(provider) !== undefined;
}

/**
 * Resolves `provider:model` to a pi-catalog model. Unknown providers fail
 * closed as `provider_unavailable`. `"script"` is not resolved here: the
 * scripted model needs the session's cursor and steer signal, so the runtime
 * builds it with `scriptedModel` and `makeStreamFn` passes mock models through.
 */
export function resolveModel(id: string): Model<Api> {
	const { provider, name } = splitModelId(id);
	const route = Object.hasOwn(ROUTES, provider) ? ROUTES[provider] : bundledRoute(provider);
	if (!route) {
		throw new AuthUnavailable(provider, ["Provider SDK could not be initialized"], {
			reason: "provider_unavailable",
		});
	}
	const override = baseUrlOverrides.get(provider);
	const bundled = getBundledModel(route.provider, name);
	// Aliases such as `openai-chat` pin a wire; a bundled row on another wire only lends its facts.
	if (bundled && (provider === route.provider || bundled.api === route.api)) {
		return override ? { ...bundled, baseUrl: override } : bundled;
	}
	return buildModel({
		id: name,
		name,
		api: route.api,
		provider: route.provider,
		baseUrl: override ?? bundled?.baseUrl ?? route.baseUrl,
		reasoning: bundled?.reasoning ?? false,
		input: bundled?.input ?? ["text", "image"],
		cost: bundled?.cost ?? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: bundled?.contextWindow ?? 128_000,
		maxTokens: bundled?.maxTokens ?? 16_384,
	});
}

/**
 * Replaces provider-authored error text: response bodies may echo prompts,
 * account identifiers or credentials. Only the HTTP status survives.
 */
export function sanitizedErrorMessage(
	message: Pick<AssistantMessage, "errorStatus"> | undefined,
	streamStarted: boolean,
): string {
	if (streamStarted) return "Provider stream interrupted; details redacted";
	const status = message?.errorStatus;
	return status === undefined
		? "Provider request failed; response details redacted"
		: `status_code: ${status}, body: provider rejected request`;
}
