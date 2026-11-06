/** Credential-free model index; the daemon owns account availability. */
import type { Effort } from "@oh-my-pi/pi-catalog";
import { FRONT_EFFORT, servedEffort } from "./effort";
import { isSupportedProvider, resolveModel } from "./models";

interface ModelFamily {
	providers: readonly string[];
	pattern: RegExp;
	current: readonly string[];
	tier: "fast" | "reasoning" | "frontier";
	strengths: string;
}

// This is the sole policy table for generations, role hints and routing guidance.
export const FAMILIES: readonly ModelFamily[] = [
	{
		providers: ["anthropic"],
		pattern: /^claude-opus-/,
		current: ["claude-opus-5-5"],
		tier: "frontier",
		strengths: "hard reasoning, complex coding, architecture",
	},
	{
		providers: ["anthropic"],
		pattern: /^claude-sonnet-/,
		current: ["claude-sonnet-5-5"],
		tier: "reasoning",
		strengths: "multi-step research, coding",
	},
	{
		providers: ["anthropic"],
		pattern: /^claude-haiku-/,
		current: ["claude-haiku-5-5"],
		tier: "fast",
		strengths: "quick lookups, simple edits",
	},
	{
		providers: ["openai", "openai-codex"],
		pattern: /(?:^|-)sol(?:$|-)/,
		current: ["gpt-6.1-sol"],
		tier: "reasoning",
		strengths: "multi-step research, coding",
	},
	{
		providers: ["openai", "openai-codex"],
		pattern: /(?:^|-)astra(?:$|-)/,
		current: ["gpt-6-astra"],
		tier: "frontier",
		strengths: "hard reasoning, complex coding, architecture",
	},
	{
		providers: ["openai", "openai-codex"],
		pattern: /(?:^|-)luna(?:$|-)/,
		current: ["gpt-6-luna"],
		tier: "fast",
		strengths: "quick lookups, simple edits",
	},
	{
		providers: ["google"],
		pattern: /(?:^|-)flash(?:$|-)/,
		current: ["gemini-3.8-flash"],
		tier: "fast",
		strengths: "quick lookups, simple browsing",
	},
	{
		providers: ["google"],
		pattern: /(?:^|-)pro(?:$|-)/,
		current: ["gemini-3.1-pro-preview"],
		tier: "reasoning",
		strengths: "multi-step research, coding",
	},
	{
		providers: [],
		pattern: /(?:^|[-_.:/])(mini|nano|lite)(?:$|[-_.:/])/,
		current: [],
		tier: "fast",
		strengths: "quick lookups, simple edits",
	},
	{
		providers: [],
		pattern: /^(?:gpt-[456](?:[.]\d+)?|o[134](?:-preview)?|deepseek-(?:r1|v3)|deepseek-reasoner)$/,
		current: [],
		tier: "reasoning",
		strengths: "multi-step research, coding",
	},
];

export const CURATED: Record<string, string[]> = Object.fromEntries(
	["anthropic", "openai", "google", "openai-codex", "github-copilot"].map(provider => [
		provider,
		FAMILIES.filter(family => family.providers.includes(provider)).flatMap(family => family.current),
	]),
);

/**
 * Curated image generation models per credential provider. Each runs through pi-ai's
 * `generateImage` on the same provider lease as that provider's chat models.
 */
export const IMAGE_MODELS: Record<string, readonly string[]> = {
	openai: ["gpt-image-2"],
	"openai-codex": ["gpt-image-2"],
	google: ["gemini-3.1-flash-image"],
	"google-antigravity": ["gemini-3.1-flash-image"],
};

// Product names where the mechanical rule in `displayName` reads wrong.
const DISPLAY_OVERRIDES: Record<string, string> = {
	"gemini-3.1-pro-preview": "Gemini 3.1 Pro",
};

/** Provider support lookup; tests replace it to simulate an unavailable provider wire. */
export const providerSupport = { isSupported: isSupportedProvider };

export type ModelRole = "fast" | "reasoning";
export type ModelKind = "chat" | "image";

export function metadata(modelId: string): [tier: string, strengths: string, role: ModelRole | null] {
	const name = modelId.slice(modelId.lastIndexOf(":") + 1).toLowerCase();
	if (Object.values(IMAGE_MODELS).some(names => names.includes(name))) return ["reasoning", "image generation", null];
	const family = FAMILIES.find(candidate => candidate.pattern.test(name));
	if (!family) return ["reasoning", "Explicitly configured model; capabilities not curated", null];
	return [family.tier, family.strengths, family.tier === "fast" ? "fast" : "reasoning"];
}

/**
 * Product name for a model id: `claude-sonnet-5-5` → "Claude Sonnet 5.5", `gpt-6.1-sol` →
 * "GPT-6.1 Sol", `gpt-image-2` → "GPT Image 2". Adjacent numeric segments are one version.
 */
export function displayName(modelId: string): string {
	const name = modelId.slice(modelId.lastIndexOf(":") + 1);
	const lower = name.toLowerCase();
	if (Object.hasOwn(DISPLAY_OVERRIDES, lower)) return DISPLAY_OVERRIDES[lower] as string;
	const words: string[] = [];
	for (const segment of name.split(/[-_]/).filter(Boolean)) {
		const previous = words.at(-1);
		if (/^\d+$/.test(segment) && previous !== undefined && /^\d+(\.\d+)*$/.test(previous)) {
			words[words.length - 1] = `${previous}.${segment}`;
		} else {
			words.push(segment);
		}
	}
	let text = "";
	for (const [index, word] of words.entries()) {
		const lowered = word.toLowerCase();
		const shown = lowered === "gpt" ? "GPT" : /^\d/.test(word) ? word : word[0]?.toUpperCase() + word.slice(1);
		// "GPT-6.1": a version directly after the GPT brand keeps its hyphen.
		const joiner = index === 0 ? "" : words[index - 1]?.toLowerCase() === "gpt" && /^\d/.test(word) ? "-" : " ";
		text += joiner + shown;
	}
	return text || name;
}

/** Chat unless the bundled catalog row is an image generation model. */
function kindOf(id: string): ModelKind {
	try {
		return resolveModel(id).kind === "image" ? "image" : "chat";
	} catch {
		return "chat";
	}
}

/**
 * The effort a chat's visible agent is served at on `id` when the person picked none. Only
 * the front agent asks for one; anywhere else the runtime sends no effort and the provider
 * applies a default the runtime cannot see, so the level is unknown (null).
 */
function defaultEffort(id: string, kind: ModelKind, front: boolean): Effort | null {
	if (kind !== "chat" || !front) return null;
	try {
		return servedEffort({ id, model: resolveModel(id) }, FRONT_EFFORT, false) ?? null;
	} catch {
		return null;
	}
}

/**
 * `orchestration` is the daemon's `agent.orchestration`: in `front` the visible agent runs on
 * `models.front` when one is set, otherwise on the chat's own model.
 */
export async function catalogReport(
	models?: Record<string, unknown>,
	providerNames: unknown[] = [],
	orchestration: unknown = "front",
): Promise<Record<string, unknown>> {
	const frontModel = typeof models?.front === "string" ? models.front : undefined;
	const namespaces = new Set(Object.keys(CURATED));
	for (const name of providerNames) if (typeof name === "string") namespaces.add(name);
	const configured = new Map<string, Set<string>>();
	if (models) {
		const selectors: unknown[] = [
			models.default,
			models.front,
			models.standard,
			models.deep,
			models.visual,
			models.image,
			...(Array.isArray(models.fallback) ? models.fallback : []),
		];
		for (const selector of selectors) {
			if (typeof selector !== "string" || !selector.includes(":")) continue;
			const separator = selector.indexOf(":");
			const provider = selector.slice(0, separator);
			const name = selector.slice(separator + 1);
			namespaces.add(provider);
			if (name) {
				const names = configured.get(provider) ?? new Set<string>();
				names.add(name);
				configured.set(provider, names);
			}
		}
	}
	const providers: { provider: string; models: Record<string, unknown>[] }[] = [];
	for (const provider of [...namespaces].sort()) {
		if (!providerSupport.isSupported(provider)) continue;
		const current = new Set([...(CURATED[provider] ?? []), ...(IMAGE_MODELS[provider] ?? [])]);
		const names = [...new Set([...current, ...(configured.get(provider) ?? [])])].sort();
		providers.push({
			provider,
			models: names.map(name => {
				const [tier, strengths, role] = metadata(name);
				const id = `${provider}:${name}`;
				const kind = kindOf(id);
				return {
					id,
					label: name,
					display_name: displayName(name),
					kind,
					recommended_for: role,
					in_catalog: current.has(name),
					tier,
					strengths,
					default_effort: defaultEffort(
						id,
						kind,
						orchestration === "front" && (frontModel === undefined || frontModel === id),
					),
				};
			}),
		});
	}
	return { providers };
}
