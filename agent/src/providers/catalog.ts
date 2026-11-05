/** Credential-free model index; the daemon owns account availability. */
import { isSupportedProvider } from "./models";

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

/** Provider support lookup; tests replace it to simulate an unavailable provider wire. */
export const providerSupport = { isSupported: isSupportedProvider };

export type ModelRole = "fast" | "reasoning";

export function metadata(modelId: string): [tier: string, strengths: string, role: ModelRole | null] {
	const name = modelId.slice(modelId.lastIndexOf(":") + 1).toLowerCase();
	const family = FAMILIES.find(candidate => candidate.pattern.test(name));
	if (!family) return ["reasoning", "Explicitly configured model; capabilities not curated", null];
	return [family.tier, family.strengths, family.tier === "fast" ? "fast" : "reasoning"];
}

export async function catalogReport(
	models?: Record<string, unknown>,
	providerNames: unknown[] = [],
): Promise<Record<string, unknown>> {
	const namespaces = new Set(Object.keys(CURATED));
	for (const name of providerNames) if (typeof name === "string") namespaces.add(name);
	const configured = new Map<string, Set<string>>();
	if (models) {
		const selectors: unknown[] = [
			models.default,
			models.front,
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
		const current = new Set(CURATED[provider] ?? []);
		const names = [...new Set([...current, ...(configured.get(provider) ?? [])])].sort();
		providers.push({
			provider,
			models: names.map(name => {
				const [tier, strengths, role] = metadata(name);
				return {
					id: `${provider}:${name}`,
					label: name,
					recommended_for: role,
					in_catalog: current.has(name),
					tier,
					strengths,
				};
			}),
		});
	}
	return { providers };
}
