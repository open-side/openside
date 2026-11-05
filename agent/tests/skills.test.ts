// Skill mentions: `/name` loads the skill via the daemon tool before the model.
// The session wiring (when the runtime calls loadMentions and where it splices the
// messages) is covered by the runtime tests; these cases pin the feature module.

import { describe, expect, it } from "bun:test";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import type { ToolResultMessage } from "@oh-my-pi/pi-ai";
import {
	type DoneToResult,
	INDEX_CLOSE,
	INDEX_OPEN,
	indexNames,
	loadMentions,
	type MentionOrigin,
	mentioned,
	type ToolInvoker,
} from "../src/features/skills";
import type { JsonObject, ToolOutcome } from "../src/protocol/types";

const INDEX =
	"Skills are reusable instructions.\n" +
	`${INDEX_OPEN}\n` +
	"- pdf-tools: Extract text: from PDFs\n" +
	"- notes: Keep notes\n" +
	"- repo-helper: [project] From the repository\n" +
	"- bad name: ignored\n" +
	`${INDEX_CLOSE}`;

const ORIGIN: MentionOrigin = { api: "mock", provider: "test", model: "test" };

const toResult: DoneToResult = (toolCallId, toolName, outcome) => ({
	role: "toolResult",
	toolCallId,
	toolName,
	content: [{ type: "text", text: JSON.stringify(outcome.for_model) }],
	details: outcome.for_model,
	isError: false,
	timestamp: 0,
});

function recorder(outcomes: Record<string, ToolOutcome>): {
	calls: Array<{ id: string; name: string; args: JsonObject }>;
	call: ToolInvoker;
} {
	const calls: Array<{ id: string; name: string; args: JsonObject }> = [];
	const call: ToolInvoker = async (id, name, args) => {
		calls.push({ id, name, args });
		return outcomes[String(args.name)] ?? { status: "denied", reason: "skill_not_found" };
	};
	return { calls, call };
}

function randomText(maxSize: number): string {
	const alphabet = [
		"/",
		" ",
		".",
		":",
		"-",
		"(",
		")",
		"'",
		'"',
		"\n",
		"\u3000",
		"a",
		"n",
		"o",
		"t",
		"e",
		"s",
		"pdf",
		"é",
		"😀",
	];
	const length = Math.floor(Math.random() * maxSize);
	let text = "";
	for (let index = 0; index < length; index += 1) {
		text += Math.random() < 0.15 ? "/notes" : alphabet[Math.floor(Math.random() * alphabet.length)];
	}
	return text;
}

describe("skills", () => {
	it("test_index_names_reads_only_the_delimited_block", () => {
		expect(indexNames(["other part", INDEX])).toEqual(["pdf-tools", "notes", "repo-helper"]);
		expect(indexNames(["- pdf-tools: not inside a block"])).toEqual([]);
		expect(indexNames([`${INDEX_OPEN}\n- a: b\n`])).toEqual([]);
	});

	it.each([
		["use /pdf-tools please", ["pdf-tools"]],
		["/pdf-tools", ["pdf-tools"]],
		["(/notes) and /pdf-tools.", ["notes", "pdf-tools"]],
		["/notes /notes /notes", ["notes"]],
		["see /usr/notes/file", []],
		["path /notes/readme", []],
		["a/notes", []],
		["/notes.md", []],
		["/unknown", []],
		["/Notes", []],
		["", []],
	])("test_mentions_are_whole_words_for_known_skills_only %p", (text, expected) => {
		expect(mentioned(text, ["pdf-tools", "notes"])).toEqual(expected);
	});

	it("test_mention_scan_never_raises_and_only_returns_known_names", () => {
		for (let round = 0; round < 500; round += 1) {
			for (const name of mentioned(randomText(300), ["pdf-tools", "notes"])) {
				expect(["pdf-tools", "notes"]).toContain(name);
			}
		}
	});

	it("test_index_names_never_raises", () => {
		for (let round = 0; round < 300; round += 1) {
			const parts = Array.from({ length: Math.floor(Math.random() * 10) }, () =>
				Math.random() < 0.3 ? `${INDEX_OPEN}${randomText(80)}${INDEX_CLOSE}` : randomText(80),
			);
			indexNames(parts);
		}
	});

	it("test_mentioned_skill_is_loaded_through_the_daemon_before_the_model_runs", async () => {
		const body = { name: "pdf-tools", instructions: "# Steps\nRun pdftotext." };
		const { calls, call } = recorder({ "pdf-tools": { status: "done", for_model: body, parts: [] } });
		const loaded = await loadMentions(
			call,
			indexNames(["Use daemon tools only.", INDEX]),
			["Please use /pdf-tools on the attached file"],
			ORIGIN,
			toResult,
		);
		expect(calls).toHaveLength(1);
		expect(calls[0]?.name).toBe("skill");
		expect(calls[0]?.args).toEqual({ name: "pdf-tools" });
		expect(calls[0]?.id.startsWith("skill-mention-")).toBe(true);
		expect(loaded.map(message => message.role)).toEqual(["assistant", "toolResult"]);
		const [turn, returned] = loaded as [AgentMessage, ToolResultMessage];
		if (turn.role !== "assistant") throw new Error("expected an assistant turn");
		expect(turn.stopReason).toBe("toolUse");
		expect(turn.content).toEqual([
			{ type: "toolCall", id: calls[0]?.id as string, name: "skill", arguments: { name: "pdf-tools" } },
		]);
		expect(returned.details).toEqual(body);
		expect(returned.toolCallId).toBe(calls[0]?.id as string);
	});

	it("test_loaded_skill_stays_in_the_persisted_conversation", async () => {
		const { call } = recorder({ notes: { status: "done", for_model: { instructions: "x" }, parts: [] } });
		const loaded = await loadMentions(call, ["notes"], ["/notes"], ORIGIN, toResult);
		const restored = JSON.parse(JSON.stringify(loaded)) as AgentMessage[];
		expect(restored.some(message => message.role === "toolResult" && message.toolName === "skill")).toBe(true);
	});

	it("test_skill_the_daemon_refuses_is_skipped_and_the_message_still_runs", async () => {
		const { calls, call } = recorder({
			"pdf-tools": { status: "denied", reason: "skill_not_found" },
			notes: { status: "error", message: "unreadable" },
		});
		const loaded = await loadMentions(call, ["pdf-tools", "notes"], ["/pdf-tools then /notes"], ORIGIN, toResult);
		expect(calls.map(item => item.args)).toEqual([{ name: "pdf-tools" }, { name: "notes" }]);
		expect(loaded).toEqual([]);
	});

	it("test_unlisted_names_and_messages_without_mentions_cause_no_tool_call", async () => {
		const { calls, call } = recorder({});
		const loaded = await loadMentions(
			call,
			["pdf-tools", "notes"],
			["open /usr/bin/env and /ghost and /notes/readme"],
			ORIGIN,
			toResult,
		);
		expect(calls).toEqual([]);
		expect(loaded).toEqual([]);
	});

	it("test_without_the_skill_tool_mentions_do_nothing", async () => {
		// Without the `skill` tool the runtime has no index names to match against.
		const { calls, call } = recorder({});
		expect(await loadMentions(call, [], ["/pdf-tools"], ORIGIN, toResult)).toEqual([]);
		expect(calls).toEqual([]);
	});

	it("test_a_message_steered_into_a_running_turn_can_mention_a_skill", async () => {
		const { calls, call } = recorder({
			notes: { status: "done", for_model: { instructions: "NOTES BODY" }, parts: [] },
		});
		// The texts accepted before this request: the running message and the steer.
		const loaded = await loadMentions(
			call,
			indexNames([INDEX]),
			["plain request", "also use /notes", "and /notes again"],
			ORIGIN,
			toResult,
		);
		expect(calls.map(item => item.args)).toEqual([{ name: "notes" }]);
		expect(calls[0]?.id.startsWith("skill-mention-")).toBe(true);
		expect(loaded.map(message => message.role)).toEqual(["assistant", "toolResult"]);
		expect((loaded[1] as ToolResultMessage).details).toEqual({ instructions: "NOTES BODY" });
	});
});
