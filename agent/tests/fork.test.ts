// Property tests for the forked-subagent history copy (PP-24 ③, A-5 ⑤).
import { describe, expect, it } from "bun:test";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import { answerId, forkHistory } from "../src/runtime/fork";

type Fate = "return" | "retry" | "missing" | "late";
// Each step: a user prompt, a tool batch with chosen answered calls, or a stray
// result for a call id that never appears (an orphan).
type Step = ["prompt", string] | ["batch", Fate[]] | ["orphan", number] | ["text", string];

// Deterministic generator so a failing example reproduces from its seed.
function rng(seed: number): () => number {
	let state = seed >>> 0;
	return () => {
		state = (state + 0x6d2b79f5) >>> 0;
		let t = state;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

function steps(random: () => number, maxSize: number): Step[] {
	const int = (lo: number, hi: number) => lo + Math.floor(random() * (hi - lo + 1));
	const text = () => Array.from({ length: int(0, 5) }, () => String.fromCharCode(int(32, 0x24f))).join("");
	const fates: Fate[] = ["return", "retry", "missing", "late"];
	return Array.from({ length: int(0, maxSize) }, (): Step => {
		switch (int(0, 3)) {
			case 0:
				return ["prompt", text()];
			case 1:
				return ["batch", Array.from({ length: int(1, 3) }, () => fates[int(0, 3)] as Fate)];
			case 2:
				return ["orphan", int(0, 3)];
			default:
				return ["text", text()];
		}
	});
}

function result(id: string, text: string, isError = false): AgentMessage {
	return {
		role: "toolResult",
		toolCallId: id,
		toolName: "probe",
		content: [{ type: "text", text }],
		isError,
		timestamp: 0,
	};
}

function assistant(content: Extract<AgentMessage, { role: "assistant" }>["content"]): AgentMessage {
	return {
		role: "assistant",
		content,
		api: "mock",
		provider: "mock",
		model: "mock",
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: "toolUse",
		timestamp: 0,
	} as AgentMessage;
}

function build(plan: Step[]): AgentMessage[] {
	const messages: AgentMessage[] = [];
	let late: AgentMessage[] = [];
	let counter = 0;
	for (const [kind, value] of plan) {
		if (kind === "prompt") {
			messages.push({ role: "user", content: value, timestamp: 0 }, ...late);
			late = [];
		} else if (kind === "text") {
			messages.push(assistant([{ type: "text", text: value }]));
		} else if (kind === "orphan") {
			messages.push(result(`x${value}`, "stray"));
		} else {
			const calls: Extract<AgentMessage, { role: "assistant" }>["content"] = [];
			const answers: AgentMessage[] = [];
			for (const fate of value) {
				counter++;
				const id = `c${counter}`;
				calls.push({ type: "toolCall", id, name: "probe", arguments: {} });
				if (fate === "return") answers.push(result(id, "ok"));
				else if (fate === "retry") answers.push(result(id, "bad", true));
				else if (fate === "late") late.push(result(id, "late"));
			}
			messages.push(assistant(calls), ...answers);
		}
	}
	return messages;
}

function wellFormed(messages: AgentMessage[]): boolean {
	const issued = new Set<string>();
	const answered = new Set<string>();
	for (const message of messages) {
		if (message.role === "assistant") {
			for (const block of message.content) if (block.type === "toolCall") issued.add(block.id);
		}
		const answer = answerId(message);
		if (answer !== undefined) {
			if (!issued.has(answer)) return false;
			answered.add(answer);
		}
	}
	return issued.size === answered.size && [...issued].every(id => answered.has(id));
}

function examples(count: number, maxSize: number, body: (history: AgentMessage[], seed: number) => void): void {
	for (let seed = 1; seed <= count; seed++) body(build(steps(rng(seed), maxSize)), seed);
}

describe("fork", () => {
	it("test_fork_is_well_formed_prefix", () => {
		examples(300, 12, (history, seed) => {
			const forked = forkHistory(history);
			expect(wellFormed(forked), `seed ${seed}`).toBe(true);
			if (wellFormed(history)) expect(forked, `seed ${seed}`).toEqual(history);
			// Kept messages appear verbatim and in input order; only orphaned results go.
			let cursor = 0;
			for (const message of forked) {
				while (cursor < history.length && !Bun.deepEquals(history[cursor], message)) cursor++;
				expect(cursor < history.length, `seed ${seed}`).toBe(true);
				cursor++;
			}
		});
	});

	it("test_fork_copy_is_independent", () => {
		examples(100, 8, (history, seed) => {
			const before = structuredClone(history);
			for (const message of forkHistory(history)) {
				if ("content" in message && Array.isArray(message.content)) message.content.length = 0;
			}
			expect(history, `seed ${seed}`).toEqual(before);
		});
	});

	it("test_delegating_batch_and_its_siblings_are_excluded", () => {
		const history = build([
			["prompt", "start"],
			["batch", ["return"]],
			["batch", ["missing", "return"]],
		]);
		const ids = new Set<string>();
		for (const message of forkHistory(history)) {
			const answer = answerId(message);
			if (answer !== undefined) ids.add(answer);
			if (message.role === "assistant") {
				for (const block of message.content) if (block.type === "toolCall") ids.add(block.id);
			}
		}
		expect(ids.has("c1")).toBe(true);
		expect(ids.has("c2")).toBe(false);
		expect(ids.has("c3")).toBe(false);
	});
});
