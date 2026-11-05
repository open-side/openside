// Front agent and background Tasks: the front replies while Tasks run.
import { beforeEach, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import type { JsonObject } from "../src/protocol/types.ts";
import { makeStreamFn } from "../src/providers/stream.ts";
import { Session } from "../src/runtime/session.ts";
import { Child } from "../src/runtime/subagents.ts";
import { FRONT_INSTRUCTIONS } from "../src/runtime/team.ts";
import {
	type FakeDaemon,
	hermeticEnv,
	installScript,
	type Wire as Message,
	textResponse,
	toolSpec,
	withFakeDaemon,
} from "./helpers/fake-daemon.ts";

function taskSpec(name: string, properties: JsonObject, required: string[]) {
	return {
		name,
		description: name,
		parameters: { type: "object", properties, required, additionalProperties: false },
		approval: "read",
		deferrable: false,
	};
}

const TOOLS = [
	toolSpec(),
	taskSpec("create_task", { goal: { type: "string" } }, ["goal"]),
	taskSpec("steer_task", { task_id: { type: "string" }, message: { type: "string" } }, ["task_id", "message"]),
	taskSpec("cancel_task", { task_id: { type: "string" } }, ["task_id"]),
	taskSpec("send_message", { text: { type: "string" } }, ["text"]),
];
const FRONT = {
	orchestration: "front",
	front_tools: ["create_task", "steer_task", "cancel_task", "send_message"],
};

function call(name: string, id: string, args: JsonObject = {}) {
	return { type: "tool_call", name, args, id };
}

function startSession(...args: ConstructorParameters<typeof Session>) {
	const session = new Session(...args);
	return { session, done: session.serve() };
}

function admit(callMessage: Message, subagentId: string) {
	expect(callMessage.name).toBe("create_task");
	return {
		status: "done",
		for_model: { subagent_id: subagentId, model: "test:task-model", tools: ["probe"], context: "fresh" },
	};
}

function events(daemon: FakeDaemon, kind: string): Message[] {
	return (daemon.outputs as Message[])
		.filter(item => item.type === "event" && item.event.kind === kind)
		.map(item => item.event);
}

function toolReturns(daemon: FakeDaemon, toolCallId: string): Message[] {
	const messages: AgentMessage[] = JSON.parse(daemon.snapshots().at(-1)!.data).messages;
	return (messages as Message[]).filter(
		message => message.role === "toolResult" && message.toolCallId === toolCallId && !message.isError,
	);
}

beforeEach(() => {
	hermeticEnv();
});

it("test_front_replies_while_the_task_runs_and_settles_once", async () => {
	installScript(
		[
			{ parts: [call("create_task", "start-1", { goal: "probe it" })] },
			{ parts: [call("send_message", "started-notice", { text: "started" })] },
			{ parts: [call("send_message", "quick-answer", { text: "4" })] },
		],
		{ subagents: { "start-1": [{ parts: [call("probe", "probe-1")] }, textResponse("probed")] } },
	);
	await withFakeDaemon(startSession, async daemon => {
		const ready = await daemon.open({ tools: TOOLS, agent: FRONT });
		expect(ready.tools).toContain("probe");
		daemon.user();
		const start = await daemon.until("tool_call");
		daemon.result(start, admit(start, "task-1"));
		let probe: Message | undefined;
		let notice: Message;
		for (;;) {
			notice = await daemon.until("tool_call");
			if (notice.name === "send_message") break;
			expect(notice.name).toBe("probe");
			expect(notice.subagent).toBe("task-1");
			probe = notice;
		}
		expect(notice.args).toEqual({ text: "started" });
		expect(notice.subagent).toBeUndefined();
		daemon.result(notice, { status: "done", for_model: { sent: true } });
		// The front run settles while the Task is still in its tool call.
		await daemon.until("settled");
		probe ??= (daemon.outputs as Message[]).find(item => item.type === "tool_call" && item.name === "probe");
		probe ??= await daemon.until("tool_call");
		expect(probe.subagent).toBe("task-1");
		daemon.user({ message_id: "question", text: "what is 2+2?" });
		const answer = await daemon.until("tool_call");
		expect(answer.name).toBe("send_message");
		expect(answer.args).toEqual({ text: "4" });
		expect(answer.subagent).toBeUndefined();
		daemon.result(answer, { status: "done", for_model: { sent: true } });
		expect(events(daemon, "assistant.final")).toEqual([]);
		expect(events(daemon, "assistant.delta")).toEqual([]);
		expect(events(daemon, "thinking.delta")).toEqual([]);
		await daemon.until("settled");
		daemon.result(probe, { status: "done", for_model: "ok" });
		const ended = () => events(daemon, "subagent.lifecycle").filter(event => event.status !== "running");
		while (ended().length === 0) await daemon.receive();
		expect(ended()).toEqual([
			{ kind: "subagent.lifecycle", subagent_id: "task-1", status: "completed", output: "probed", known_subagents: [] },
		]);
		// The daemon needs the idle signal to dispatch its durable report queue;
		// it withholds public SessionSettled until those reports are complete.
		await daemon.until("settled");
		expect(toolReturns(daemon, "start-1")[0]!.details).toEqual({ task_id: "task-1", status: "running" });
	});
});

it("test_task_steer_reaches_only_its_task", async () => {
	const wait = { parts: [{ type: "wait_steer", timeout_ms: 5000 }, call("probe", "probe")] };
	installScript(
		[
			{
				parts: [call("create_task", "start-1", { goal: "first" }), call("create_task", "start-2", { goal: "second" })],
			},
			{ parts: [call("stay_silent", "started-both")] },
			{
				parts: [call("steer_task", "steer-1", { task_id: "{{result:start-2.task_id}}", message: "answer BETA" })],
			},
			{ parts: [call("stay_silent", "steered")] },
		],
		{ subagents: { "start-1": [wait], "start-2": [wait, { parts: [{ type: "echo" }] }] } },
	);
	await withFakeDaemon(startSession, async daemon => {
		await daemon.open({ tools: TOOLS, agent: FRONT });
		daemon.user();
		const first = await daemon.until("tool_call");
		daemon.result(first, admit(first, "task-1"));
		const second = await daemon.until("tool_call");
		daemon.result(second, admit(second, "task-2"));
		await daemon.until("settled");
		daemon.user({ message_id: "steer", text: "make the second one say BETA" });
		const steer = await daemon.until("tool_call");
		expect(steer.name).toBe("steer_task");
		expect(steer.args.task_id).toBe("task-2");
		daemon.result(steer, { status: "done", for_model: { task_id: "task-2", status: "pending" } });
		let done: Message[];
		for (;;) {
			done = events(daemon, "subagent.lifecycle").filter(event => event.status === "completed");
			if (done.length > 0) break;
			const message = await daemon.receive();
			if (message.type === "tool_call" && message.subagent === "task-2") {
				daemon.result(message, { status: "done", for_model: "ok" });
			}
		}
		expect(done).toEqual([
			{
				kind: "subagent.lifecycle",
				subagent_id: "task-2",
				status: "completed",
				output: "answer BETA",
				known_subagents: ["task-1"],
			},
		]);
		daemon.send("subagent_cancel", { subagent_id: "task-1" });
		while (
			!events(daemon, "subagent.lifecycle").some(
				event => event.subagent_id === "task-1" && event.status === "cancelled",
			)
		) {
			await daemon.receive();
		}
	});
});

it("test_front_exposes_only_its_tools", async () => {
	installScript([{ parts: [call("stay_silent", "silent")] }], { subagents: {} });
	await withFakeDaemon(startSession, async daemon => {
		const ready = await daemon.open({ tools: TOOLS, agent: FRONT });
		expect([...ready.tools].sort()).toEqual(TOOLS.map(tool => tool.name).sort());
		daemon.user();
		await daemon.until("settled");
	});
});

it("test_task_steer_denies_if_child_finishes_after_host_admission", async () => {
	installScript(
		[
			{ parts: [call("create_task", "start", { goal: "finish" })] },
			{ parts: [call("stay_silent", "started")] },
			{ parts: [call("steer_task", "steer", { task_id: "task", message: "late" })] },
			{ parts: [call("stay_silent", "checked")] },
		],
		{ subagents: { start: [{ parts: [call("probe", "probe")] }, textResponse("done")] } },
	);
	await withFakeDaemon(startSession, async daemon => {
		await daemon.open({ tools: TOOLS, agent: FRONT });
		daemon.user();
		const start = await daemon.until("tool_call");
		daemon.result(start, admit(start, "task"));
		const probe = await daemon.until("tool_call");
		if (!(daemon.outputs as Message[]).some(item => item.type === "settled")) await daemon.until("settled");
		daemon.user({ message_id: "steer", text: "late instruction" });
		const steer = await daemon.until("tool_call");
		expect(steer.name).toBe("steer_task");
		daemon.result(probe, { status: "done", for_model: "ok" });
		while (!events(daemon, "subagent.lifecycle").some(event => event.status === "completed")) await daemon.receive();
		daemon.result(steer, { status: "done", for_model: { task_id: "task", status: "pending" } });
		await daemon.until("settled");
		expect(toolReturns(daemon, "steer")[0]!.details).toEqual({ status: "denied", reason: "task_finished" });
		expect(events(daemon, "subagent.progress").some(event => event.progress.kind === "steer_queued")).toBe(false);
		daemon.send("subagent_steer", { subagent_id: "task", text: "late", control_id: "finished" });
		const ack = await daemon.until("subagent_steer_result");
		expect(ack).toEqual({ v: 1, type: "subagent_steer_result", control_id: "finished", delivered: false });
	});
});

// Python patched providers.build_model; here every lease-aware stream function the session
// builds records its context (model id, fallback chain, subagent), delegating to the real one.
function recordingSession(selected: { model: string; fallback: string[]; subagent: string | undefined }[]) {
	return (...[conn, rpc]: ConstructorParameters<typeof Session>) =>
		startSession(conn, rpc, {
			makeStreamFn: (port, ctx) => {
				selected.push({ model: ctx.model, fallback: [...(ctx.fallback ?? [])], subagent: ctx.subagent });
				return makeStreamFn(port, ctx);
			},
		});
}

for (const profile of [null, "ultrabrowse"]) {
	for (const fast of ["openai:gpt-6-luna", null]) {
		it(`test_front_model_never_leaks_into_task_children[${fast ?? "None"}-${profile ?? "None"}]`, async () => {
			const selected: { model: string; fallback: string[]; subagent: string | undefined }[] = [];
			await withFakeDaemon(recordingSession(selected), async (daemon, session) => {
				await daemon.open({
					tools: TOOLS,
					agent: {
						...FRONT,
						...(fast !== null ? { front_model: fast } : {}),
						...(profile !== null ? { profile } : {}),
					},
					model: "anthropic:claude-sonnet-5-5",
				});
				const reasoning = session.spec.model.id;
				session.spec.model.fallback = ["test:captured-fallback"];
				const child = new Child({
					id: "task-model-role",
					parent_tool_call_id: "start-role",
					task: "reason about the result",
					model: reasoning,
					tools: [],
					status: "running",
					messages: [],
					pending_deferred: [],
					steer_buffer: [],
					script_cursor: 0,
					result: null,
					background: true,
					prompt_epoch: null,
				});
				session.subs.childAgent(child);
				const parent = session.agent!.options;
				expect({ reasoning: parent.reasoning as string | undefined, maxTokens: parent.maxTokens }).toEqual({
					reasoning: "minimal",
					maxTokens: 1024,
				});
				expect(child.agent!.options.reasoning).toBeUndefined();
				expect(child.agent!.options.maxTokens).toBeUndefined();
				expect(selected).toEqual([
					{ model: fast ?? reasoning, fallback: [], subagent: undefined },
					{ model: reasoning, fallback: ["test:captured-fallback"], subagent: child.id },
				]);
			});
		});
	}
}

it("test_front_routing_guidance_snapshot", () => {
	const snapshot = readFileSync(join(import.meta.dir, "snapshots", "front-instructions.txt"), "utf8");
	expect(`${FRONT_INSTRUCTIONS}\n`).toBe(snapshot);
});
