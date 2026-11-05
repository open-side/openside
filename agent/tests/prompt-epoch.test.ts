// Prompt epochs: stable prompt bytes persist across resumes; diagnostics carry only
// digests. The session/child wiring (which sections the runtime feeds, when it records
// a request) is covered by the runtime and subagent tests.

import { afterEach, beforeEach, describe, expect, it, spyOn } from "bun:test";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import { logRequestIdentity, PromptEpoch, promptLog, stableSections } from "../src/features/prompt-epoch";
import type { ToolSpec } from "../src/protocol/types";

function tools(...names: string[]): ToolSpec[] {
	return names.map(name => ({
		name,
		description: "Daemon probe",
		parameters: { type: "object", properties: { value: { type: "string" } }, additionalProperties: false },
		approval: "read",
		deferrable: true,
	}));
}

function user(text: string, synthetic = false): AgentMessage {
	return { role: "user", content: text, timestamp: 0, ...(synthetic ? { synthetic } : {}) };
}

let lines: string[] = [];
let spy: ReturnType<typeof spyOn>;

beforeEach(() => {
	lines = [];
	spy = spyOn(promptLog, "write").mockImplementation((line: string) => {
		lines.push(line);
	});
});

afterEach(() => {
	spy.mockRestore();
});

interface SectionsReport {
	epoch: string;
	sections: Record<string, { sha256: string; chars: number }>;
	causes: string[];
}

function reports(): SectionsReport[] {
	return lines
		.filter(line => line.startsWith("prompt_sections "))
		.map(line => JSON.parse(line.slice(line.indexOf(" ") + 1)) as SectionsReport);
}

describe("prompt epoch", () => {
	it("test_canonical_tools_and_immutable_copies", () => {
		const source = tools("z", "a");
		const epoch = PromptEpoch.assemble({ base_system: "fixed" }, source);
		expect(epoch.toolsJson).toBe(PromptEpoch.assemble({ base_system: "fixed" }, [...source].reverse()).toolsJson);
		(source[0] as ToolSpec).description = "changed";
		(epoch.tools[0] as ToolSpec).description = "changed";
		epoch.sections.base_system = "changed";
		expect(epoch.tools.map(tool => tool.name)).toEqual(["a", "z"]);
		expect(epoch.tools.every(tool => tool.description === "Daemon probe")).toBe(true);
		expect(epoch.instructions).toEqual(["fixed"]);
		expect(() => PromptEpoch.assemble({}, tools("a", "a"))).toThrow(/Duplicate/);
		expect(() => PromptEpoch.assemble({ other: "x" } as never, [])).toThrow(/Unknown stable prompt section/);
	});

	it("test_resume_preserves_epoch_until_explicit_transition", () => {
		const epoch = PromptEpoch.assemble({ base_system: "old" }, tools("a"));
		const restored = PromptEpoch.fromJSON(JSON.parse(JSON.stringify({ prompt_epoch: epoch })).prompt_epoch);
		const resumed = PromptEpoch.assemble({ base_system: "new" }, tools("b"), restored);
		expect(resumed.toJSON()).toEqual(epoch.toJSON());
		expect(resumed.tailContext({ base_system: "new" })).not.toContain("new");
		const transitioned = PromptEpoch.assemble({ base_system: "new" }, tools("b"), resumed, { transition: true });
		expect(transitioned.id).not.toBe(resumed.id);
		expect(transitioned.instructions).toEqual(["new"]);
		expect(transitioned.tools.map(tool => tool.name)).toEqual(["b"]);
		expect(() => PromptEpoch.fromJSON({ ...epoch.toJSON(), extra: 1 })).toThrow();
		expect(() => PromptEpoch.fromJSON({ id: "x" })).toThrow();
	});

	it("test_section_hashes_and_tail_do_not_leak_content", () => {
		const sections = stableSections(["secret-base", "<available_skills>secret-skill</available_skills>"], {
			persona: "secret-persona",
			profile: "secret-profile",
		});
		const epoch = PromptEpoch.assemble(sections, tools("secret-tool"));
		const messages = [user("secret-first")];
		epoch.recordRequest(messages, { provider: "secret-provider", model: "secret-model" });
		epoch.recordRequest(messages, {
			provider: "secret-provider",
			model: "secret-model",
			latestContext: epoch.tailContext(sections, "secret-tail"),
		});
		const [before, after] = reports() as [SectionsReport, SectionsReport];
		expect(after.causes).toEqual(["latest_context_changed"]);
		for (const key of ["base_system", "persona", "profile", "skill_index", "tools", "first_message"]) {
			expect(before.sections[key]).toEqual(after.sections[key] as { sha256: string; chars: number });
		}
		const changed = PromptEpoch.assemble({ ...sections, persona: "different" }, epoch.tools);
		changed.recordRequest(messages, { provider: "secret-provider", model: "secret-model" });
		const third = reports().at(-1) as SectionsReport;
		expect(
			Object.keys(before.sections).filter(
				key => JSON.stringify(before.sections[key]) !== JSON.stringify(third.sections[key]),
			),
		).toEqual(["persona"]);
		const old = logRequestIdentity("secret-provider", "secret-model", "secret-account-a");
		logRequestIdentity("secret-provider", "secret-model", "secret-account-b", old);
		const text = lines.join("\n");
		expect(text).toContain("account_boundary");
		expect(text).not.toContain("secret-");
	});

	it("test_runtime_resumes_frozen_tools_and_instructions", () => {
		const first = PromptEpoch.assemble(stableSections(["Use daemon tools only."]), tools("z", "a"));
		first.recordRequest([user("hello")], { provider: "test", model: "test" });
		const snapshot = JSON.parse(JSON.stringify(first));
		const original = first.toJSON();
		// A new deployment changes the prompt and tools; the resumed epoch keeps both.
		const current = stableSections(["New deployment must wait."]);
		const resumed = PromptEpoch.assemble(
			current,
			[{ ...(tools("a")[0] as ToolSpec), description: "new schema" }],
			PromptEpoch.fromJSON(snapshot),
		);
		expect(resumed.toJSON()).toEqual(original);
		expect(resumed.tools.map(tool => tool.name)).toEqual(["a", "z"]);
		expect(resumed.instructions).toEqual(first.instructions);
		expect(resumed.instructions.join("\n")).not.toContain("New deployment must wait.");
		const tail = resumed.tailContext(current);
		expect(tail).toContain("stable_sections_pending_next_epoch");
		expect(tail).not.toContain("New deployment must wait.");
		resumed.recordRequest([user("hello"), user("next"), user(tail, true)], {
			provider: "test",
			model: "test",
			latestContext: tail,
		});
		expect(reports()).toHaveLength(2);
		expect(reports()[1]?.causes).toEqual(["epoch_first_request"]);
	});

	it("test_child_epoch_keeps_original_grants_and_logs_requests", () => {
		const child = PromptEpoch.assemble(stableSections(["child prompt"]), tools("probe", "read"));
		expect(child.tools.map(tool => tool.name)).toEqual(["probe", "read"]);
		const restored = PromptEpoch.fromJSON(JSON.parse(JSON.stringify(child.toJSON())));
		const original = restored.toJSON();
		const again = PromptEpoch.assemble(stableSections(["child prompt"]), tools("probe"), restored);
		expect(again.toJSON()).toEqual(original);
		again.recordRequest([user("child task")], { provider: "test", model: "test" });
		expect(reports().at(-1)?.epoch).toBe(original.id);
	});

	it("first message changes are reported against the persisted digest", () => {
		const epoch = PromptEpoch.assemble({ base_system: "fixed" }, []);
		epoch.recordRequest([user("first")], { provider: "p", model: "m" });
		const resumed = PromptEpoch.fromJSON(epoch.toJSON());
		resumed.recordRequest([user("rewritten")], { provider: "p", model: "m" });
		expect(reports()[1]?.causes).toEqual(["epoch_first_request", "first_message_changed"]);
		resumed.recordRequest([user("rewritten")], { provider: "p", model: "other" });
		expect(reports()[2]?.causes).toEqual(["provider_or_model_boundary"]);
	});

	it.each(["browser-agent", "ultrabrowse"])("daemon prompt parts fingerprint only by their text %p", async profile => {
		const open = (await Bun.file(`${import.meta.dir}/../../docs/specs/profiles/${profile}.open.json`).json()) as {
			system_prompt_parts: string[];
			tools: ToolSpec[];
		};
		const parts = open.system_prompt_parts;
		const first = PromptEpoch.assemble(stableSections(parts), open.tools);
		const second = PromptEpoch.assemble(stableSections([...parts]), [...open.tools].reverse());
		expect(second.sectionsJson).toBe(first.sectionsJson);
		expect(second.toolsJson).toBe(first.toolsJson);
		const edited = PromptEpoch.assemble(stableSections([...parts.slice(0, -1), `${parts.at(-1)} `]), open.tools);
		first.recordRequest([user("task")], { provider: "p", model: "m" });
		edited.recordRequest([user("task")], { provider: "p", model: "m" });
		const [before, after] = reports() as [SectionsReport, SectionsReport];
		expect(
			Object.keys(before.sections).filter(
				key => JSON.stringify(before.sections[key]) !== JSON.stringify(after.sections[key]),
			),
		).toEqual(["base_system"]);
	});

	it("digests match the Python sidecar for the same sections", () => {
		// Fixed by running the Python `stable_sections` + `_canonical` + `_digest` on these inputs.
		const sections = stableSections(["base é", "<available_skills>\n- a: b\n</available_skills> tail"], {
			persona: "p",
		});
		const epoch = PromptEpoch.assemble(sections, tools("probe"));
		expect(epoch.sectionsJson).toBe(
			'{"base_system":"base é\\n\\n tail","persona":"p","policy":"","profile":"","skill_index":"<available_skills>\\n- a: b\\n</available_skills>"}',
		);
		expect(epoch.toolsJson).toBe(
			'[{"approval":"read","deferrable":true,"description":"Daemon probe","name":"probe","parameters":{"additionalProperties":false,"properties":{"value":{"type":"string"}},"type":"object"}}]',
		);
		epoch.recordRequest([user("hi")], { provider: "anthropic", model: "claude" });
		const report = reports()[0] as SectionsReport;
		expect(report.sections.base_system).toEqual({
			sha256: new Bun.CryptoHasher("sha256").update("base é\n\n tail").digest("hex"),
			chars: 13,
		});
	});

	it("first message digest matches the Python sidecar for text and multipart content", () => {
		// Python: PromptEpoch.record_request on UserPromptPart("hello") and
		// UserPromptPart(["hi", BinaryContent(data=b"abc", media_type="image/png")]).
		const text = PromptEpoch.assemble({}, []);
		text.recordRequest([user("hello")], { provider: "p", model: "m" });
		expect(text.firstMessage).toEqual({
			sha256: "c7a0f7154e64cd96c617f251dc12c4396b7234c2856ccf4860ab7af537dfcdd9",
			chars: 9,
		});
		const multipart = PromptEpoch.assemble({}, []);
		multipart.recordRequest(
			[
				{
					role: "user",
					content: [
						{ type: "text", text: "hi" },
						{ type: "image", data: "YWJj", mimeType: "image/png" },
					],
					timestamp: 0,
				},
			],
			{ provider: "p", model: "m" },
		);
		expect(multipart.firstMessage).toEqual({
			sha256: "05346f3925935ff16bd813c98e90f97fba6af9660d2b5f9b74a569702be21b77",
			chars: 110,
		});
	});

	it("fromJSON rejects malformed first-message digests", () => {
		const valid = PromptEpoch.assemble({}, []).toJSON();
		const good = { sha256: "a".repeat(64), chars: 3 };
		expect(PromptEpoch.fromJSON({ ...valid, first_message: good }).firstMessage).toEqual(good);
		for (const bad of [
			{ sha256: "A".repeat(64), chars: 3 },
			{ sha256: "a".repeat(63), chars: 3 },
			{ sha256: "a".repeat(64), chars: -1 },
			{ sha256: "a".repeat(64), chars: 1.5 },
			{ sha256: "a".repeat(64), chars: Number.POSITIVE_INFINITY },
			{ sha256: "a".repeat(64), chars: 3, extra: true },
			"digest",
		]) {
			expect(() => PromptEpoch.fromJSON({ ...valid, first_message: bad })).toThrow("Invalid prompt epoch");
		}
	});
});
