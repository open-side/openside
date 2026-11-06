import { beforeEach, describe, expect, it } from "bun:test";
import { type Effort, streamSimple } from "@oh-my-pi/pi-ai";
import { createMockModel } from "@oh-my-pi/pi-ai/providers/mock";
import { catalogReport } from "../src/providers/catalog";
import { servedEffort } from "../src/providers/effort";
import { resolveModel } from "../src/providers/models";
import { Session } from "../src/runtime/session";
import type { JsonlConnection } from "../src/transport/jsonl";
import type { RpcRouter } from "../src/transport/rpc";
import { hermeticEnv, withFakeDaemon } from "./helpers/fake-daemon";

interface Row {
	id: string;
	kind: string;
	default_effort: string | null;
}

async function catalog(orchestration: string, models?: Record<string, unknown>): Promise<Row[]> {
	const report = await catalogReport(models, [], orchestration);
	return (report.providers as { models: Row[] }[]).flatMap(provider => provider.models);
}

/** What the session's visible agent is served at on `model` when the chat has no effort. */
async function servedWithoutEffort(model: string, orchestration: string): Promise<string | null> {
	const reply = createMockModel({
		handler:
			orchestration === "front"
				? { content: [{ type: "toolCall", name: "stay_silent", arguments: {} }] }
				: { content: [{ type: "text", text: "ok" }] },
	});
	const served: (Effort | undefined)[] = [];
	const start = (conn: JsonlConnection, rpc: RpcRouter) => {
		const session = new Session(conn, rpc, {
			makeStreamFn: (_port, ctx) => (_model, context, options) => {
				// Only the visible agent reports a served effort; title and summary calls do not.
				if (ctx.onServedEffort !== undefined) {
					served.push(
						servedEffort(
							{ id: ctx.model, model: resolveModel(ctx.model) },
							options?.reasoning,
							ctx.effortFixed === true,
						),
					);
				}
				return streamSimple(reply, context, options);
			},
		});
		return { session, done: session.serve() };
	};
	await withFakeDaemon(start, async daemon => {
		await daemon.open({ model, agent: { orchestration, front_tools: [], title_needed: false } });
		daemon.user();
		await daemon.until("settled");
	});
	expect(served.length).toBeGreaterThan(0);
	return served[0] ?? null;
}

beforeEach(() => hermeticEnv());

describe("default effort", () => {
	// The composer shows the catalog's level for an unpicked effort; it must be what the runtime
	// actually serves, for every curated chat model in either orchestration.
	for (const orchestration of ["front", "single"]) {
		it(`test_catalog_default_effort_matches_the_runtime[${orchestration}]`, async () => {
			const rows = (await catalog(orchestration)).filter(row => row.kind === "chat");
			expect(rows.length).toBeGreaterThan(0);
			for (const row of rows) {
				expect({ id: row.id, effort: await servedWithoutEffort(row.id, orchestration) }).toEqual({
					id: row.id,
					effort: row.default_effort,
				});
			}
		}, 60_000);
	}

	it("test_front_agent_serves_each_curated_model_its_lowest_tier", async () => {
		const efforts = Object.fromEntries((await catalog("front")).map(row => [row.id, row.default_effort]));
		expect(efforts["anthropic:claude-sonnet-5-5"]).toBe("low");
		expect(efforts["openai-codex:gpt-6.1-sol"]).toBe("low");
		expect(efforts["google:gemini-3.1-pro-preview"]).toBe("low");
		expect(efforts["openai:gpt-image-2"]).toBeNull();
	});

	it("test_a_separate_fast_model_leaves_the_chat_model_to_the_provider", async () => {
		const efforts = Object.fromEntries(
			(await catalog("front", { front: "anthropic:claude-haiku-5-5" })).map(row => [row.id, row.default_effort]),
		);
		expect(efforts["anthropic:claude-haiku-5-5"]).toBe("low");
		expect(efforts["anthropic:claude-sonnet-5-5"]).toBeNull();
	});

	it("test_single_orchestration_sends_no_effort", async () => {
		expect((await catalog("single")).map(row => row.default_effort).filter(effort => effort !== null)).toEqual([]);
	});
});
