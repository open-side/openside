import { afterEach, describe, expect, it, spyOn } from "bun:test";
import {
	CURATED,
	catalogReport,
	displayName,
	type ModelRole,
	metadata,
	providerSupport,
} from "../../src/providers/catalog";

interface CatalogModel {
	display_name: string;
	kind: string;
	id: string;
	label: string;
	recommended_for: string | null;
	in_catalog: boolean;
	tier: string;
	strengths: string;
	default_effort: string | null;
}

interface CatalogRow {
	provider: string;
	models: CatalogModel[];
}

const rows = (report: Record<string, unknown>): CatalogRow[] => report.providers as CatalogRow[];

const savedEnv = {
	anthropic: process.env.ANTHROPIC_API_KEY,
	broker: process.env.OPENSIDE_BROKER_TOKEN,
};

afterEach(() => {
	for (const [name, value] of [
		["ANTHROPIC_API_KEY", savedEnv.anthropic],
		["OPENSIDE_BROKER_TOKEN", savedEnv.broker],
	] as const) {
		if (value === undefined) delete process.env[name];
		else process.env[name] = value;
	}
});

describe("catalog", () => {
	it("test_catalog_is_only_a_credential_free_sdk_index", async () => {
		const canary = "synthetic-catalog-secret-canary";
		process.env.ANTHROPIC_API_KEY = canary;
		process.env.OPENSIDE_BROKER_TOKEN = canary;
		const report = await catalogReport();
		expect(Object.keys(report)).toEqual(["providers"]);
		expect(rows(report).map(row => row.provider)).toEqual(Object.keys(CURATED).sort());
		for (const provider of rows(report)) {
			expect(Object.keys(provider).sort()).toEqual(["models", "provider"]);
			for (const model of provider.models) {
				expect(Object.keys(model).sort()).toEqual(
					[
						"default_effort",
						"display_name",
						"id",
						"in_catalog",
						"kind",
						"label",
						"recommended_for",
						"strengths",
						"tier",
					].sort(),
				);
				expect(model.display_name).toBe(displayName(model.label));
				expect(model.in_catalog).toBe(true);
				expect(["fast", "reasoning", "frontier"]).toContain(model.tier);
				expect(model.strengths).toBeTruthy();
				expect(model.recommended_for).toBe(metadata(model.id)[2]);
				expect(model.id).toBe(`${provider.provider}:${model.label}`);
			}
		}
		expect(JSON.stringify(report)).not.toContain(canary);
	});

	it("test_catalog_omits_unavailable_sdk_without_initializing_clients", async () => {
		const original = providerSupport.isSupported;
		const support = spyOn(providerSupport, "isSupported").mockImplementation(provider =>
			provider === "google" ? false : original(provider),
		);
		try {
			const report = await catalogReport();
			expect(rows(report).map(row => row.provider)).not.toContain("google");
		} finally {
			support.mockRestore();
		}
	});

	it("test_model_selectors_and_broker_names_add_supported_sdk_namespaces_only", async () => {
		const report = await catalogReport(
			{
				default: "openrouter:test/model",
				front: "openai-chat:gpt-5",
				fallback: ["unsupported:model"],
			},
			["github-copilot", "openai-responses", "unsupported"],
		);
		const groups = Object.fromEntries(rows(report).map(row => [row.provider, row.models]));
		for (const provider of ["github-copilot", "openrouter", "openai-chat", "openai-responses"]) {
			expect(Object.keys(groups)).toContain(provider);
		}
		expect(Object.keys(groups)).not.toContain("unsupported");
		expect(groups["github-copilot"]).toEqual([]);
		expect(groups.openrouter).toEqual([
			{
				id: "openrouter:test/model",
				label: "test/model",
				display_name: "Test/model",
				kind: "chat",
				recommended_for: null,
				in_catalog: false,
				tier: "reasoning",
				strengths: "Explicitly configured model; capabilities not curated",
				default_effort: null,
			},
		]);
	});

	const roles: [string, ModelRole | null][] = [
		["openai:GPT-6-LUNA", "fast"],
		["claude-haiku-5-5", "fast"],
		["gemini-3.8-flash", "fast"],
		["gpt-5-mini", "fast"],
		["gpt-5-nano", "fast"],
		["gemini-flash-lite", "fast"],
		["gpt-6.1-sol", "reasoning"],
		["claude-sonnet-5-5", "reasoning"],
		["claude-opus-5-5", "reasoning"],
		["gemini-3.1-pro-preview", "reasoning"],
		["gpt-6-astra", "reasoning"],
		["gpt-5", "reasoning"],
		["custom-model", null],
		["professional", null],
	];
	for (const [model, role] of roles) {
		it(`test_curated_role_recommendations[${model}-${role}]`, () => {
			expect(metadata(model)[2]).toBe(role);
		});
	}

	it("test_superseded_generations_are_hidden_unless_explicitly_configured", async () => {
		const removed = ["gpt-5.4-mini", "claude-opus-4-8", "claude-sonnet-4-6", "gemini-2.5-pro"];
		const labels = rows(await catalogReport()).flatMap(row => row.models.map(model => model.label));
		expect(labels.filter(label => removed.includes(label))).toEqual([]);
		const models = Object.fromEntries(
			rows(await catalogReport({ default: "openai:gpt-5.4-mini" })).flatMap(row =>
				row.models.map(model => [model.id, model]),
			),
		);
		expect(models["openai:gpt-5.4-mini"]?.in_catalog).toBe(false);
		expect(models["openai:gpt-6-luna"]?.in_catalog).toBe(true);
		expect(models["openai:gpt-6-astra"]?.tier).toBe("frontier");
	});

	it("test_display_names_read_as_product_names", () => {
		expect(displayName("claude-sonnet-5-5")).toBe("Claude Sonnet 5.5");
		expect(displayName("openai:gpt-6.1-sol")).toBe("GPT-6.1 Sol");
		expect(displayName("gpt-image-2")).toBe("GPT Image 2");
		expect(displayName("gemini-3.1-pro-preview")).toBe("Gemini 3.1 Pro");
		expect(displayName("gemini-3.1-flash-image")).toBe("Gemini 3.1 Flash Image");
	});

	it("test_image_models_are_listed_with_image_kind", async () => {
		const models = Object.fromEntries(
			rows(await catalogReport({ image: "google-antigravity:gemini-3.1-flash-image" }, ["openai-codex"])).flatMap(row =>
				row.models.map(model => [model.id, model as unknown as Record<string, unknown>]),
			),
		);
		expect(models["openai-codex:gpt-image-2"]?.kind).toBe("image");
		expect(models["google:gemini-3.1-flash-image"]?.kind).toBe("image");
		expect(models["google-antigravity:gemini-3.1-flash-image"]?.kind).toBe("image");
		expect(models["openai-codex:gpt-image-2"]?.recommended_for).toBeNull();
		expect(models["anthropic:claude-sonnet-5-5"]?.kind).toBe("chat");
	});
});
