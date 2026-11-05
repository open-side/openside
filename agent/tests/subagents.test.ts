// Sidecar-owned subagents: parent-session tickets, restart, cancel, steer, fork.
import { beforeEach, expect, it } from "bun:test";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import { type JsonObject, STATE_FORMAT } from "../src/protocol/types.ts";
import { userMessageTexts } from "../src/runtime/content.ts";
import { Session } from "../src/runtime/session.ts";
import type { SubagentState } from "../src/runtime/state.ts";
import { Child } from "../src/runtime/subagents.ts";
import {
	type FakeDaemon,
	hermeticEnv,
	installScript,
	type Wire as Message,
	textResponse,
	toolSpec,
	withFakeDaemon,
} from "./helpers/fake-daemon.ts";

function startSession(...args: ConstructorParameters<typeof Session>) {
	const session = new Session(...args);
	return { session, done: session.serve() };
}

type SnapshotData = {
	messages: AgentMessage[];
	pending_delegates: Message[];
	subagents: [SubagentState, ...SubagentState[]];
};

function delegateSpec() {
	return {
		name: "delegate",
		description: "Delegate a task to a subagent",
		parameters: {
			type: "object",
			properties: {
				task: { type: "string" },
				model: { type: "string" },
				tools: { type: "array", items: { type: "string" } },
				context: { type: "string", enum: ["fresh", "fork"] },
			},
			required: ["task"],
			additionalProperties: false,
		},
		approval: "read",
		deferrable: true,
	};
}

function call(name: string, id: string, args: JsonObject = {}) {
	return { type: "tool_call", name, args, id };
}

function admission(callMessage: Message, subagentId = "sub-1", context = "fresh") {
	expect(callMessage.name).toBe("delegate");
	expect(callMessage.subagent).toBeUndefined();
	return {
		status: "done",
		for_model: { subagent_id: subagentId, model: "test:child-model", tools: ["probe"], context },
	};
}

function snapshotData(daemon: FakeDaemon): SnapshotData {
	return JSON.parse(daemon.snapshots().at(-1)!.data);
}

function delegateReturns(daemon: FakeDaemon, callId = "del-1"): [Message, ...Message[]] {
	const returns = (snapshotData(daemon).messages as Message[]).filter(
		message => message.role === "toolResult" && message.toolCallId === callId,
	);
	// Callers index [0] only after asserting a return exists (or expect one); a missing
	// return then fails on the property read.
	return returns as [Message, ...Message[]];
}

function lifecycle(daemon: FakeDaemon, subagentId = "sub-1"): string[] {
	return (daemon.outputs as Message[])
		.filter(
			item =>
				item.type === "event" && item.event.kind === "subagent.lifecycle" && item.event.subagent_id === subagentId,
		)
		.map(item => item.event.status);
}

function userPrompts(messages: AgentMessage[]): string[] {
	return messages
		.filter(message => message.role === "user")
		.flatMap(message => userMessageTexts(message as Parameters<typeof userMessageTexts>[0]));
}

const PARENT = [{ parts: [call("delegate", "del-1", { task: "inspect the fixture" })] }, textResponse("parent done")];
const CHILD = { "del-1": [{ parts: [call("probe", "child-1")] }, textResponse("child done")] };

/** Delegate, then defer the child's probe on an approval ticket. */
async function deferChild(daemon: FakeDaemon) {
	await daemon.open({ tools: [toolSpec(), delegateSpec()] });
	await daemon.user();
	const delegate = await daemon.until("tool_call");
	await daemon.result(delegate, admission(delegate));
	const probe: Message = await daemon.until("tool_call");
	expect(probe.subagent).toBe("sub-1");
	expect(probe.tool_call_id).toBe("child-1");
	await daemon.result(probe, { status: "deferred", kind: "approval", ticket_id: "ticket-1" });
	const deferred: Message = await daemon.until("deferred");
	expect(deferred.calls).toEqual([{ tool_call_id: "child-1", ticket_id: "ticket-1", kind: "approval" }]);
	await daemon.until("settled");
	expect(
		(daemon.outputs as Message[]).filter(item => item.type === "message_result").map(item => item.outcome),
	).toEqual(["deferred"]);
	return daemon.snapshots().at(-1)!;
}

async function approveAndFinish(daemon: FakeDaemon) {
	await daemon.send("ticket_resolved", {
		ticket_id: "ticket-1",
		tool_call_id: "child-1",
		resolution: { status: "approved" },
	});
	const repeated: Message = await daemon.until("tool_call");
	expect(repeated.subagent).toBe("sub-1");
	expect(repeated.approval_ticket).toBe("ticket-1");
	await daemon.result(repeated, { status: "done", for_model: "probed" });
	const final: Message = await daemon.until("event", { kind: "assistant.final" });
	expect(final.event.text).toBe("parent done");
	await daemon.until("settled");
}

beforeEach(() => {
	hermeticEnv();
});

it("test_child_approval_is_parent_ticket_and_resumes_child_once", async () => {
	installScript(PARENT, { subagents: CHILD });
	await withFakeDaemon(startSession, async daemon => {
		const snapshot = await deferChild(daemon);
		const saved: SnapshotData = JSON.parse(snapshot.data);
		expect(saved.pending_delegates.map(item => item.tool_call_id)).toEqual(["del-1"]);
		expect(saved.subagents[0].status).toBe("waiting_human");
		expect(saved.subagents[0].model).toBe("test:child-model");
		expect(lifecycle(daemon)).toEqual(["running", "waiting_human"]);
		await approveAndFinish(daemon);
		expect(lifecycle(daemon)).toEqual(["running", "waiting_human", "running", "completed"]);
		const returns = delegateReturns(daemon);
		expect(returns).toHaveLength(1);
		expect(returns[0].details).toEqual({ status: "completed", subagent_id: "sub-1", output: "child done" });
		const child = snapshotData(daemon).subagents[0];
		expect(child.status).toBe("completed");
		expect(child.pending_deferred).toEqual([]);
		await daemon.send("ticket_resolved", {
			ticket_id: "ticket-1",
			tool_call_id: "child-1",
			resolution: { status: "approved" },
		});
		expect(((await daemon.until("error")) as Message).code).toBe("stale_ticket");
	});
});

it("test_child_wait_survives_restart_and_completes_once", async () => {
	installScript(PARENT, { subagents: CHILD });
	const snapshot = await withFakeDaemon(startSession, deferChild);
	await withFakeDaemon(startSession, async daemon => {
		const ready: Message = await daemon.open({ tools: [toolSpec(), delegateSpec()], resume_state: snapshot });
		expect(ready.pending_deferred).toEqual([{ tool_call_id: "child-1", ticket_id: "ticket-1", kind: "approval" }]);
		await approveAndFinish(daemon);
		expect(delegateReturns(daemon)).toHaveLength(1);
		expect(
			(daemon.outputs as Message[])
				.filter(item => item.type === "message_result")
				.map(item => [item.message_id, item.outcome]),
		).toEqual([["message-1", "completed"]]);
	});
});

it("test_cancel_waiting_child_returns_cancelled_to_parent", async () => {
	installScript(PARENT, { subagents: CHILD });
	await withFakeDaemon(startSession, async daemon => {
		await deferChild(daemon);
		await daemon.send("subagent_cancel", { subagent_id: "sub-1" });
		const final: Message = await daemon.until("event", { kind: "assistant.final" });
		expect(final.event.text).toBe("parent done");
		await daemon.until("settled");
		expect(lifecycle(daemon).at(-1)).toBe("cancelled");
		expect(delegateReturns(daemon)[0].details).toEqual({ status: "cancelled", subagent_id: "sub-1" });
		expect((daemon.outputs as Message[]).some(item => item.type === "tool_call" && item.approval_ticket)).toBe(false);
		// Cancelling a finished child is harmless and reports no error.
		await daemon.send("subagent_cancel", { subagent_id: "sub-1" });
		await daemon.send("subagent_steer", { subagent_id: "sub-1", text: "late", control_id: "late" });
		expect(((await daemon.until("subagent_steer_result")) as Message).delivered).toBe(false);
		await daemon.user({ message_id: "after", text: "after cancel" });
		await daemon.until("settled");
		expect((daemon.outputs as Message[]).some(item => item.type === "error")).toBe(false);
	});
});

it("test_cancel_running_child_returns_cancelled", async () => {
	installScript(PARENT, { subagents: CHILD });
	await withFakeDaemon(startSession, async daemon => {
		await daemon.open({ tools: [toolSpec(), delegateSpec()] });
		await daemon.user();
		const delegate = await daemon.until("tool_call");
		await daemon.result(delegate, admission(delegate));
		const childCall = await daemon.until("tool_call");
		await daemon.send("subagent_cancel", { subagent_id: "sub-1" });
		await daemon.until("event", { kind: "assistant.final" });
		await daemon.until("settled");
		expect(lifecycle(daemon)).toEqual(["running", "cancelled"]);
		expect(delegateReturns(daemon)[0].details.status).toBe("cancelled");
		// The daemon still finishes the abandoned host call; that reply is harmless.
		await daemon.result(childCall, { status: "done", for_model: "late" });
		await daemon.user({ message_id: "after", text: "after cancel" });
		await daemon.until("settled");
		expect((daemon.outputs as Message[]).filter(item => item.type === "error")).toEqual([]);
		// A reply to an RPC that never existed is still a protocol error.
		await daemon.send("tool_result", { rpc_id: "never-sent", outcome: { status: "done", for_model: "x" } });
		expect(((await daemon.until("error")) as Message).code).toBe("bad_request");
	});
});

it("test_cancel_running_child_waits_for_host_tool_cleanup", async () => {
	installScript(PARENT, { subagents: CHILD });
	const order: string[] = [];
	let cancelled = false;
	const cleanupStarted = Promise.withResolvers<void>();
	const release = Promise.withResolvers<void>();
	await withFakeDaemon(
		(connection, rpc) => {
			const toolCall = rpc.toolCall.bind(rpc);
			// The child's host call unwinds only when the test releases it, like a proxy
			// still releasing a daemon RPC; terminal reporting must wait for it.
			rpc.toolCall = async (args, signal) => {
				if (args.subagent !== "sub-1") return toolCall(args, signal);
				try {
					return await toolCall(args, signal);
				} finally {
					if (signal.aborted) {
						cleanupStarted.resolve();
						await release.promise;
						order.push("cleanup");
					}
				}
			};
			const send = rpc.send.bind(rpc);
			rpc.send = message => {
				if (cancelled && message.type === "state_snapshot") order.push("snapshot");
				if (message.type === "event" && message.event.kind === "subagent.lifecycle") {
					order.push(`lifecycle:${message.event.status}`);
				}
				send(message);
			};
			return startSession(connection, rpc);
		},
		async daemon => {
			await daemon.open({ tools: [toolSpec(), delegateSpec()] });
			await daemon.user();
			const delegate = await daemon.until("tool_call");
			await daemon.result(delegate, admission(delegate));
			await daemon.until("tool_call");
			cancelled = true;
			await daemon.send("subagent_cancel", { subagent_id: "sub-1" });
			await cleanupStarted.promise;
			for (let turn = 0; turn < 20; turn++) await new Promise(setImmediate);
			expect(order).toEqual(["lifecycle:running"]);
			release.resolve();
			await daemon.until("event", { kind: "assistant.final" });
			await daemon.until("settled");
			expect(order.slice(0, 3)).toEqual(["lifecycle:running", "cleanup", "lifecycle:cancelled"]);
			expect(order.indexOf("snapshot")).toBeGreaterThan(order.indexOf("cleanup"));
			// The cancelled child's saved history answers its abandoned call.
			const child = snapshotData(daemon).subagents[0];
			const answered = (child.messages as Message[]).filter(
				message => message.role === "toolResult" && message.toolCallId === "child-1",
			);
			expect(answered).toHaveLength(1);
		},
	);
});

it("test_steer_injects_at_child_model_request_boundary", async () => {
	const child = {
		"del-1": [
			{ parts: [{ type: "wait_steer", timeout_ms: 5000 }, call("probe", "child-1")] },
			textResponse("child steered"),
		],
	};
	installScript(PARENT, { subagents: child });
	await withFakeDaemon(startSession, async daemon => {
		await daemon.open({ tools: [toolSpec(), delegateSpec()] });
		await daemon.user();
		const delegate = await daemon.until("tool_call");
		await daemon.result(delegate, admission(delegate));
		await daemon.until("event", { kind: "subagent.progress" });
		await daemon.send("subagent_steer", { subagent_id: "sub-1", text: "focus on X", control_id: "live" });
		const probe = await daemon.until("tool_call");
		await daemon.result(probe, { status: "done", for_model: "probed" });
		await daemon.until("settled");
		const state = snapshotData(daemon).subagents[0];
		expect(userPrompts(state.messages)).toEqual(["inspect the fixture", "focus on X"]);
		const progress = (daemon.outputs as Message[])
			.filter(item => item.type === "event" && item.event.kind === "subagent.progress")
			.map(item => item.event.progress.kind);
		expect(progress).toContain("steer_queued");
		expect(progress).toContain("steer_injected");
		expect(delegateReturns(daemon)[0].details.output).toBe("child steered");
		expect(userPrompts(snapshotData(daemon).messages)).not.toContain("focus on X");
	});
});

it("test_fork_context_excludes_delegating_batch", async () => {
	const parent = [
		{ parts: [call("probe", "parent-1")] },
		{ parts: [call("delegate", "del-1", { task: "forked", context: "fork" }), call("probe", "parent-2")] },
		textResponse("parent done"),
	];
	installScript(parent, { subagents: { "del-1": [textResponse("child done")] } });
	await withFakeDaemon(startSession, async daemon => {
		await daemon.open({ tools: [toolSpec(), delegateSpec()] });
		await daemon.user();
		const first = await daemon.until("tool_call");
		await daemon.result(first, { status: "done", for_model: "one" });
		const calls: Record<string, Message> = {};
		while (Object.keys(calls).length < 2) {
			const message: Message = await daemon.until("tool_call");
			calls[message.name] = message;
		}
		await daemon.result(calls.probe!, { status: "done", for_model: "two" });
		await daemon.result(calls.delegate!, admission(calls.delegate!, "sub-1", "fork"));
		await daemon.until("settled");
		const child = snapshotData(daemon).subagents[0];
		const ids = new Set<string>();
		for (const message of child.messages as Message[]) {
			if (message.role === "toolResult") ids.add(message.toolCallId);
			if (message.role === "assistant") {
				for (const part of message.content) if (part.type === "toolCall") ids.add(part.id);
			}
		}
		expect(ids.has("parent-1")).toBe(true);
		expect(ids.has("del-1")).toBe(false);
		expect(ids.has("parent-2")).toBe(false);
	});
});

it("test_child_failure_keeps_redacted_exception_and_inventory", async () => {
	installScript([textResponse("done")], { subagents: {} });
	await withFakeDaemon(startSession, async (daemon, session) => {
		await daemon.open({ tools: [toolSpec(), delegateSpec()] });
		const subs = session.subs;
		const child = new Child({
			id: "child",
			parent_tool_call_id: "call",
			task: "work",
			model: "test:child",
			tools: ["probe"],
			status: "running",
			messages: [],
			pending_deferred: [],
			steer_buffer: [],
			script_cursor: 0,
			result: null,
			background: false,
			prompt_epoch: null,
		});
		subs.children.set(child.id, child);
		subs.childAgent = () => {
			throw new TypeError("private-canary");
		};
		let redacted: unknown;
		const rpc = session.rpc;
		const toolCall = rpc.toolCall.bind(rpc);
		rpc.toolCall = async (args, signal) => {
			if (args.name !== "__redact_diagnostic") return toolCall(args, signal);
			redacted = args.arguments;
			return { status: "done", for_model: "TypeError: [REDACTED]", parts: [] };
		};
		subs.childStopped = async () => {};
		await subs.driveChild(child, "work", undefined);
		expect(redacted).toEqual({ diagnostic: "TypeError: private-canary" });
		const result = child.state.result as JsonObject;
		expect(result.error_code).toBe("child_run_failed");
		expect(result.message).toBe("TypeError: [REDACTED]");
		await daemon.until("event");
		const terminal: Message = await daemon.until("event");
		expect(terminal.event.diagnostic).toBe("TypeError: [REDACTED]");
		expect(terminal.event.known_subagents).toEqual([]);
		expect(JSON.stringify(terminal)).not.toContain("private-canary");
	});
});

it("test_restore_corrupt_child_never_resumes_pending_actions", async () => {
	installScript([textResponse("done")], { subagents: {} });
	const child: SubagentState = {
		id: "child-[REDACTED]",
		parent_tool_call_id: "call",
		task: "work",
		model: "test:child",
		tools: ["probe"],
		status: "failed",
		messages: [],
		pending_deferred: [],
		steer_buffer: [],
		script_cursor: 0,
		result: { status: "failed", message: "Subagent run failed" },
		background: false,
		prompt_epoch: null,
	};
	const waiting: SubagentState = {
		...structuredClone(child),
		id: "waiting-[REDACTED]",
		status: "waiting_human",
		pending_deferred: [{ tool_call_id: "pending", ticket_id: "ticket", kind: "external" }],
	};
	await withFakeDaemon(startSession, async (daemon, session) => {
		await daemon.open({
			tools: [toolSpec(), delegateSpec()],
			resume_state: { format: STATE_FORMAT, data: JSON.stringify({ messages: [], subagents: [child, waiting] }) },
		});
		for (const stored of [child, waiting]) {
			const restored = session.subs.children.get(stored.id)!;
			expect(restored.state.status).toBe("failed");
			expect((restored.state.result as JsonObject).error_code).toBe("corrupt_child_identity");
			expect(restored.pending.size).toBe(0);
			expect(restored.active).toBe(false);
		}
		for (const stored of [child, waiting]) {
			const repaired = await daemon.until("event", { kind: "subagent.lifecycle" });
			expect(repaired.event).toMatchObject({
				subagent_id: stored.id,
				status: "failed",
				error_code: "corrupt_child_identity",
			});
		}
	});
});
