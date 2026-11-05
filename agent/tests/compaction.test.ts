// A-3: the task goal stated at turn 1 survives compaction of a 200-turn session.
import { beforeEach, describe, expect, it } from "bun:test";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import type { Context, Message } from "@oh-my-pi/pi-ai";
import { createMockModel } from "@oh-my-pi/pi-ai/providers/mock";
import {
	type CompactionConfig,
	estimatedTokens,
	IdleCompaction,
	type IdleState,
	ReportingCompaction,
	SUMMARY_INSTRUCTIONS,
	SUMMARY_PREFIX,
} from "../src/runtime/compaction";
import { Session, type SessionOptions } from "../src/runtime/session";
import type { JsonlConnection } from "../src/transport/jsonl";
import type { RpcRouter } from "../src/transport/rpc";
import { hermeticEnv, type Wire, withFakeDaemon } from "./helpers/fake-daemon";

const GOAL =
	"GOAL: migrate the billing database to region eu-west-3 before 2026-11-30; " +
	"change ticket OPS-4471; never touch the invoices table.";
const FACTS = ["eu-west-3", "2026-11-30", "OPS-4471", "invoices table"];
const RECALL = "Remind me: what is the task goal, the deadline and the ticket?";
const TURNS = 200;

function session(options: SessionOptions = {}) {
	return (conn: JsonlConnection, rpc: RpcRouter) => {
		const started = new Session(conn, rpc, options);
		return { session: started, done: started.serve() };
	};
}

function userTexts(messages: readonly (AgentMessage | Message)[]): string[] {
	return messages.flatMap(message => {
		if (message.role !== "user") return [];
		if (typeof message.content === "string") return [message.content];
		return [message.content.map(block => (block.type === "text" ? block.text : `[${block.mimeType}]`)).join(" ")];
	});
}

function toolCall(id: string): AgentMessage {
	return assistant([{ type: "toolCall", id, name: "probe", arguments: {} }]);
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
		stopReason: "stop",
		timestamp: 0,
	} as AgentMessage;
}

function user(content: Extract<AgentMessage, { role: "user" }>["content"]): AgentMessage {
	return { role: "user", content, timestamp: 0 };
}

function largeHistory(): AgentMessage[] {
	return [
		user("Keep the goal."),
		toolCall("call"),
		{
			role: "toolResult",
			toolCallId: "call",
			toolName: "probe",
			content: [{ type: "text", text: "x".repeat(20000) }],
			isError: false,
			timestamp: 0,
		},
		assistant([{ type: "text", text: "Evidence read." }]),
		user("Continue."),
		assistant([{ type: "text", text: "Latest answer verbatim." }]),
	];
}

function config(maxTokens: number, keepMessages = 20): CompactionConfig {
	return { maxMessages: undefined, keepMessages, maxTokens };
}

function mockModel() {
	return createMockModel({ handler: { content: ["ok"] } });
}

beforeEach(() => hermeticEnv());

describe("compaction", () => {
	it("test_goal_survives_compaction_of_a_200_turn_session", async () => {
		let summaries = 0;
		const contexts: Context["messages"][] = [];
		const model = createMockModel({
			handler: (context: Context) => {
				contexts.push([...context.messages]);
				const texts = userTexts(context.messages);
				if (!texts.includes(RECALL)) return { content: ["Noted."] };
				const goal = texts.find(text => text.startsWith("GOAL:"));
				return { content: [goal ? `The goal is: ${goal}` : "I no longer know the goal."] };
			},
		});
		const options: SessionOptions = {
			model,
			complete: async request => {
				expect(request.instructions).toBe(SUMMARY_INSTRUCTIONS);
				summaries++;
				// Drops the goal on purpose: only the pinned first message may carry it forward.
				return { text: "The user sent routine status updates.", model: "mock", input: 0, output: 0 };
			},
		};
		let outputs: Wire[] = [];
		await withFakeDaemon(session(options), async daemon => {
			await daemon.open({ agent: { compaction_max_messages: 100, title_needed: false } });
			for (let turn = 1; turn <= TURNS; turn++) {
				const text = turn === 1 ? GOAL : turn === TURNS ? RECALL : `Status update ${turn}: nothing new.`;
				daemon.user({ message_id: `m${turn}`, text });
				const result = await daemon.until("message_result");
				expect(result.message_id).toBe(`m${turn}`);
				expect(result.outcome).toBe("completed");
				await daemon.until("settled");
			}
			outputs = daemon.outputs;
		});
		const finals = outputs
			.filter(item => item.type === "event" && item.event.kind === "assistant.final")
			.map(item => item.event.text as string);
		expect(finals).toHaveLength(TURNS);
		// 200 turns are 400 messages; 100/20 thresholds compact about every 40 turns.
		expect(summaries).toBeGreaterThanOrEqual(4);
		// The daemon is told each time earlier messages left the model context, so it can
		// retire tool-result bases (browser snapshot ids) the model no longer sees.
		const notices = outputs.filter(item => item.type === "context_compacted");
		expect(notices).toHaveLength(summaries);
		expect(notices.every(item => !("subagent" in item))).toBe(true);
		const compacted = contexts.filter(context => userTexts(context).some(text => text.startsWith(SUMMARY_PREFIX)));
		expect(compacted.length).toBeGreaterThan(TURNS / 2);
		expect(Math.max(...contexts.map(context => context.length))).toBeLessThanOrEqual(101);
		for (const context of compacted) expect(userTexts(context)).toContain(GOAL);
		const summaryTexts = userTexts(compacted.at(-1) ?? []).filter(text => text.startsWith(SUMMARY_PREFIX));
		expect(summaryTexts.some(text => text.includes("eu-west-3"))).toBe(false);
		const recall = finals.at(-1) ?? "";
		for (const fact of FACTS) expect(recall).toContain(fact);
	}, 120_000);

	it("test_compaction_notice_names_the_agent_it_belongs_to", async () => {
		await withFakeDaemon(session({ model: mockModel() }), async (daemon, live) => {
			await daemon.open();
			live.compactionReporter("sub-1")();
			live.compactionReporter()();
			const child = await daemon.until("context_compacted");
			const parent = await daemon.until("context_compacted");
			expect(child.subagent).toBe("sub-1");
			expect("subagent" in parent).toBe(false);
		});
	});

	it("test_large_single_result_compacts_and_keeps_final_messages", async () => {
		const messages = largeHistory();
		const seen: AgentMessage[][] = [];
		const cap = new ReportingCompaction(config(1000), async head => {
			seen.push(head);
			return "Evidence summarized.";
		});
		expect(cap.needed(messages)).toBe(true);
		const compacted = await cap.compact(messages);
		expect(compacted.slice(-2)).toEqual(messages.slice(-2));
		expect(compacted.length).toBeLessThan(messages.length);
		expect(compacted.some(message => message.role === "toolResult")).toBe(false);
		expect(
			compacted.some(
				message => message.role === "assistant" && message.content.some(block => block.type === "toolCall"),
			),
		).toBe(false);
		expect(seen).toHaveLength(1);
		expect(seen[0]).toContain(messages[2] as AgentMessage);
	});

	it("test_tool_result_tail_keeps_its_matching_call", async () => {
		const messages = largeHistory().slice(0, 3);
		const cap = new ReportingCompaction(config(1000, 2), async () => "Goal summary.");
		const compacted = await cap.compact(messages);
		expect(compacted).toContain(messages[1] as AgentMessage);
		expect(compacted).toContain(messages[2] as AgentMessage);
	});

	it("test_short_hundred_message_chat_does_not_trigger_default_budget", () => {
		const messages = Array.from({ length: 100 }, () => user("Hello"));
		expect(new ReportingCompaction(config(64000), async () => "").needed(messages)).toBe(false);
		expect(estimatedTokens(largeHistory())).toBeGreaterThan(1000);
	});

	it("test_large_screenshots_use_a_bounded_media_estimate", () => {
		const history = (payload: string) =>
			Array.from({ length: 5 }, () =>
				user([
					{ type: "text", text: "Inspect this page" },
					{ type: "image", data: Buffer.from(payload).toString("base64"), mimeType: "image/png" },
				]),
			);
		const small = history("small");
		const large = history("x".repeat(2_000_000));
		const cap = new ReportingCompaction(config(64000), async () => "");
		expect(estimatedTokens(large)).toBe(estimatedTokens(small));
		expect(estimatedTokens(large)).toBeGreaterThanOrEqual(5 * 1600);
		expect(cap.needed(large)).toBe(false);
		expect(cap.needed([...large, user("x".repeat(200_000))])).toBe(true);
	});

	it("test_replayed_instructions_count_only_the_current_block", () => {
		// pi-agent-core sends the system prompt once per request, never replayed per
		// message, so only the current block counts.
		const messages = Array.from({ length: 100 }, () => user("Hello"));
		const cap = new ReportingCompaction(config(64000), async () => "");
		const policy = ["policy ".repeat(2000)];
		expect(cap.needed(messages, policy)).toBe(false);
		expect(cap.needed(messages, ["x".repeat(200_000)])).toBe(true);
	});

	for (const change of ["owner", "wake", "epoch", "history"] as const) {
		it(`test_idle_summary_never_commits_stale_history[${change}]`, async () => {
			const messages = largeHistory();
			const cap = new ReportingCompaction(config(1000), async () => "");
			const started = Promise.withResolvers<void>();
			const release = Promise.withResolvers<void>();
			cap.idleSummary = async () => {
				started.resolve();
				// A provider may finish even after its request was cancelled.
				await release.promise;
				return [user("summary")];
			};
			const controller = new IdleCompaction();
			let epoch = "epoch-1";
			let wake = false;
			let commits = 0;
			controller.schedule(
				cap,
				messages,
				epoch,
				(): IdleState => [epoch, messages, wake],
				async () => {
					commits++;
				},
			);
			const task = controller.task;
			expect(task).toBeDefined();
			await started.promise;
			if (change === "owner") controller.cancel();
			else if (change === "wake") wake = true;
			else if (change === "epoch") epoch = "epoch-2";
			else messages.push(user("new owner message"));
			release.resolve();
			await task;
			expect(commits).toBe(0);
		});
	}

	it("test_idle_summary_commits_once_and_skips_imminent_wake", async () => {
		const messages = largeHistory();
		const cap = new ReportingCompaction(config(1000), async () => "");
		const summary = [messages[0] as AgentMessage, messages.at(-1) as AgentMessage];
		cap.idleSummary = async () => summary;
		const controller = new IdleCompaction();
		const commits: AgentMessage[][] = [];
		const commit = async (compacted: AgentMessage[]) => {
			commits.push(compacted);
		};
		controller.schedule(cap, messages, "epoch", () => ["epoch", messages, true], commit);
		expect(controller.task).toBeUndefined();
		controller.schedule(cap, messages, "epoch", () => ["epoch", messages, false], commit);
		expect(controller.task).toBeDefined();
		await controller.task;
		expect(commits).toEqual([summary]);
	});

	it("test_session_idle_commit_invalidates_bases_and_snapshots", async () => {
		await withFakeDaemon(session({ model: mockModel() }), async (daemon, live) => {
			await daemon.open({ agent: { compaction_max_tokens: 1000, title_needed: false } });
			daemon.user();
			await daemon.until("settled");
			const agent = live.agent as NonNullable<typeof live.agent>;
			agent.messages = largeHistory();
			const cap = new ReportingCompaction(config(1000), async () => "");
			cap.idleSummary = async messages => [messages[0] as AgentMessage, messages.at(-1) as AgentMessage];
			live.scheduleIdleCompaction(cap);
			await daemon.until("context_compacted");
			await daemon.until("state_snapshot");
			expect(live.messages).toHaveLength(2);
		});
	});

	it("test_session_owner_input_cancels_idle_summary", async () => {
		const started = Promise.withResolvers<void>();
		const cancelled = Promise.withResolvers<void>();
		await withFakeDaemon(session({ model: mockModel() }), async (daemon, live) => {
			await daemon.open({ agent: { compaction_max_tokens: 1000, title_needed: false } });
			daemon.user();
			await daemon.until("settled");
			const agent = live.agent as NonNullable<typeof live.agent>;
			agent.messages = largeHistory();
			const cap = new ReportingCompaction(config(1000), async () => "");
			cap.idleSummary = (_messages, signal) => {
				started.resolve();
				const pending = Promise.withResolvers<AgentMessage[]>();
				signal?.addEventListener("abort", () => {
					cancelled.resolve();
					pending.reject(new Error("aborted"));
				});
				return pending.promise;
			};
			live.scheduleIdleCompaction(cap);
			await started.promise;
			// A larger runtime budget avoids a foreground summary in this cancellation test.
			(agent.options.compaction as ReportingCompaction).maxTokens = 64000;
			daemon.user({ message_id: "owner-update", text: "Owner update" });
			await daemon.until("settled");
			await cancelled.promise;
			expect(userTexts(live.messages)).toContain("Owner update");
			expect(daemon.outputs.some(item => item.type === "context_compacted")).toBe(false);
		});
	});
});
