import { beforeEach, describe, expect, it } from "bun:test";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import { type Effort, streamSimple } from "@oh-my-pi/pi-ai";
import { createMockModel } from "@oh-my-pi/pi-ai/providers/mock";
import { AuthUnavailable, EffortUnsupported } from "../src/providers/errors";
import { Session, type SessionOptions } from "../src/runtime/session";
import { Child } from "../src/runtime/subagents.ts";
import type { JsonlConnection } from "../src/transport/jsonl";
import type { RpcRouter } from "../src/transport/rpc";
import {
	type FakeDaemon,
	hermeticEnv,
	installScript,
	textResponse,
	toolResponse,
	toolSpec,
	type Wire,
	withFakeDaemon,
} from "./helpers/fake-daemon";

function session(options: SessionOptions = {}) {
	return (conn: JsonlConnection, rpc: RpcRouter) => {
		const started = new Session(conn, rpc, options);
		return { session: started, done: started.serve() };
	};
}

function stateMessages(daemon: FakeDaemon): AgentMessage[] {
	const last = daemon.snapshots().at(-1) as Wire;
	return JSON.parse(last.data).messages;
}

function userTexts(daemon: FakeDaemon): string[] {
	return stateMessages(daemon).flatMap(message => {
		if (message.role !== "user") return [];
		if (typeof message.content === "string") return [message.content];
		return message.content.flatMap(block => (block.type === "text" ? [block.text] : []));
	});
}

function toolResults(daemon: FakeDaemon) {
	return stateMessages(daemon).flatMap(message => (message.role === "toolResult" ? [message] : []));
}

function of(daemon: FakeDaemon, type: string): Wire[] {
	return daemon.outputs.filter(item => item.type === type);
}

function resultIds(daemon: FakeDaemon): string[] {
	return of(daemon, "message_result").map(item => item.message_id);
}

function eventKinds(daemon: FakeDaemon): string[] {
	return of(daemon, "event").map(item => item.event.kind);
}

beforeEach(() => hermeticEnv());

describe("runtime", () => {
	for (const outcome of [
		{ status: "done", for_model: { value: "daemon result" } },
		{ status: "denied", reason: "blocked", hint: "Use the allowed alternative" },
		{ status: "error", message: "Tool unavailable" },
	] as Wire[]) {
		it(`test_reverse_rpc_outcomes[${outcome.status}]`, async () => {
			installScript([toolResponse(), textResponse()]);
			await withFakeDaemon(session(), async daemon => {
				await daemon.open({ tools: [toolSpec()] });
				daemon.user();
				const call = await daemon.until("tool_call");
				expect(call.tool_call_id).toBe("call-1");
				daemon.result(call, outcome);
				await daemon.until("settled");
				const results = toolResults(daemon);
				expect(results).toHaveLength(1);
				const result = results[0] as Wire;
				if (outcome.status === "done") {
					expect(result.details).toEqual(outcome.for_model);
					expect(result.isError).toBe(false);
				} else {
					expect(result.isError).toBe(true);
					const text = result.content.map((block: Wire) => block.text).join("");
					expect(text).toContain(outcome.reason ?? outcome.message);
					if (outcome.hint) expect(text).toContain(outcome.hint);
				}
				expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["completed"]);
				const kinds = eventKinds(daemon);
				expect(kinds.filter(kind => kind === "turn.started")).toHaveLength(2);
				expect(kinds.filter(kind => kind === "turn.ended")).toHaveLength(2);
				expect(kinds).toContain("assistant.delta");
				expect(kinds).toContain("usage");
				expect(kinds.some(kind => kind.startsWith("tool."))).toBe(false);
			});
		});
	}

	it("parallel_failures_of_one_tool_count_once_per_step", async () => {
		const calls = ["a", "b", "c", "d"];
		installScript([
			{ parts: calls.map(id => ({ type: "tool_call", name: "probe", args: {}, id })) },
			textResponse("public answer"),
		]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			for (const _ of calls) daemon.result(await daemon.until("tool_call"), { status: "denied", reason: "blocked" });
			await daemon.until("settled");
			expect(toolResults(daemon).every(item => item.isError)).toBe(true);
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["completed"]);
		});
	});

	it("tool_failing_in_more_steps_than_retries_fails_the_run", async () => {
		// Default tool_retries is 3: the fourth consecutive failing step exceeds it.
		installScript([1, 2, 3, 4].map(step => toolResponse("probe", `call-${step}`)).concat([textResponse()]));
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			for (let step = 0; step < 4; step++) {
				daemon.result(await daemon.until("tool_call"), { status: "error", message: "Tool unavailable" });
			}
			await daemon.until("settled");
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["failed"]);
		});
	});

	it("test_external_deferred_restart_exactly_once", async () => {
		installScript([toolResponse(), textResponse("resumed")]);
		let snapshot: Wire | undefined;
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			const call = await daemon.until("tool_call");
			daemon.result(call, { status: "deferred", kind: "external", ticket_id: "ticket-1" });
			await daemon.until("settled");
			snapshot = daemon.snapshots().at(-1);
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["deferred"]);
		});
		await withFakeDaemon(session(), async daemon => {
			const ready = await daemon.open({ tools: [toolSpec()], resume_state: snapshot });
			expect(ready.pending_deferred).toEqual([{ tool_call_id: "call-1", ticket_id: "ticket-1", kind: "external" }]);
			const fields = {
				tool_call_id: "call-1",
				ticket_id: "ticket-1",
				resolution: { status: "result", for_model: { status: "filled" } },
			};
			daemon.send("ticket_resolved", fields);
			const final = await daemon.until("event", { kind: "assistant.final" });
			expect(final.event.text).toBe("resumed");
			await daemon.until("settled");
			const results = toolResults(daemon);
			expect(results).toHaveLength(1);
			expect(results[0]?.toolCallId).toBe("call-1");
			expect(results[0]?.details).toEqual({ status: "filled" });
			expect(of(daemon, "tool_call")).toHaveLength(0);
			expect(of(daemon, "message_result").map(item => [item.message_id, item.outcome])).toEqual([
				["message-1", "completed"],
			]);
			daemon.send("ticket_resolved", fields);
			expect((await daemon.until("error")).code).toBe("stale_ticket");
		});
	});

	for (const kind of ["approval", "external"]) {
		it(`test_restore_waits_for_daemon_and_accepts_interrupted_resolution[${kind}]`, async () => {
			installScript([toolResponse(), textResponse("checked")]);
			let snapshot: Wire = {};
			await withFakeDaemon(session(), async daemon => {
				await daemon.open({ tools: [toolSpec()] });
				daemon.user();
				const call = await daemon.until("tool_call");
				daemon.result(call, { status: "deferred", kind, ticket_id: "ticket-1" });
				await daemon.until("settled");
				snapshot = daemon.snapshots().at(-1) as Wire;
			});
			const saved = JSON.parse(snapshot.data);
			saved.pending_deferred[0].resolution =
				kind === "approval" ? { status: "approved" } : { status: "result", for_model: { status: "filled" } };
			snapshot = { ...snapshot, data: JSON.stringify(saved) };
			const interrupted = {
				status: "interrupted",
				message: "The previous resumption was interrupted; verify state before retrying.",
			};
			const resolution =
				kind === "approval"
					? { status: "denied", reason: "interrupted_after_resume" }
					: { status: "result", for_model: interrupted };
			await withFakeDaemon(session(), async daemon => {
				const ready = await daemon.open({ tools: [toolSpec()], resume_state: snapshot });
				expect(ready.pending_deferred).toEqual([{ tool_call_id: "call-1", ticket_id: "ticket-1", kind }]);
				daemon.send("ticket_resolved", { ticket_id: "ticket-1", tool_call_id: "call-1", resolution });
				await daemon.until("settled");
				const results = toolResults(daemon);
				expect(results).toHaveLength(1);
				expect(results[0]?.toolCallId).toBe("call-1");
				if (kind === "approval") {
					expect(results[0]?.isError).toBe(true);
					expect(JSON.stringify(results[0]?.content)).toContain("interrupted_after_resume");
				} else {
					expect(results[0]?.details).toEqual(interrupted);
				}
				expect(daemon.outputs.some(item => item.type === "tool_call" || item.type === "error")).toBe(false);
				expect(of(daemon, "message_result").map(item => [item.message_id, item.outcome])).toEqual([
					["message-1", "completed"],
				]);
				daemon.send("ticket_resolved", { ticket_id: "ticket-1", tool_call_id: "call-1", resolution });
				expect((await daemon.until("error")).code).toBe("stale_ticket");
			});
		});
	}

	it("test_all_pending_calls_resolve_before_resume", async () => {
		const first = { parts: [toolResponse("probe", "call-1").parts[0], toolResponse("probe", "call-2").parts[0]] };
		installScript([first, textResponse()]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			const calls = [await daemon.until("tool_call"), await daemon.until("tool_call")];
			for (const call of calls) {
				daemon.result(call, { status: "deferred", kind: "external", ticket_id: `ticket-${call.tool_call_id}` });
			}
			await daemon.until("settled");
			for (const [index, call] of calls.entries()) {
				const fields = {
					ticket_id: `ticket-${call.tool_call_id}`,
					tool_call_id: call.tool_call_id,
					resolution: { status: "result", for_model: index },
				};
				daemon.send("ticket_resolved", fields);
				if (index === 0) {
					daemon.send("ticket_resolved", fields);
					expect((await daemon.until("error")).code).toBe("stale_ticket");
				}
			}
			await daemon.until("settled");
			expect(eventKinds(daemon).filter(kind => kind === "assistant.final")).toHaveLength(1);
		});
	});

	for (const approved of [true, false]) {
		it(`test_approval_resume_and_denial[${approved}]`, async () => {
			installScript([toolResponse(), textResponse()]);
			await withFakeDaemon(session(), async daemon => {
				await daemon.open({ tools: [toolSpec()] });
				daemon.user();
				const call = await daemon.until("tool_call");
				daemon.result(call, { status: "deferred", kind: "approval", ticket_id: "approval-1" });
				await daemon.until("settled");
				const resolution = approved ? { status: "approved" } : { status: "denied", reason: "Approval rejected" };
				daemon.send("ticket_resolved", { ticket_id: "approval-1", tool_call_id: "call-1", resolution });
				if (approved) {
					const repeated = await daemon.until("tool_call");
					expect(repeated.approval_ticket).toBe("approval-1");
					expect(repeated.tool_call_id).toBe("call-1");
					daemon.result(repeated, { status: "denied", reason: "Policy still denies", hint: null });
				}
				await daemon.until("settled");
				const results = toolResults(daemon);
				const text = JSON.stringify(results.map(item => item.content));
				expect(results.every(item => item.isError)).toBe(true);
				expect(text).toContain(approved ? "Policy still denies" : "Approval rejected");
			});
		});
	}

	for (const deferred of [false, true]) {
		it(`test_steer_during_tool_request_boundary[${deferred}]`, async () => {
			installScript([toolResponse(), textResponse()]);
			await withFakeDaemon(session(), async daemon => {
				await daemon.open({ tools: [toolSpec()] });
				daemon.user();
				const call = await daemon.until("tool_call");
				daemon.user({ message_id: "steer-1", text: "steered", mode: "steer" });
				daemon.result(
					call,
					deferred
						? { status: "deferred", kind: "external", ticket_id: "ticket-1" }
						: { status: "done", for_model: "ok" },
				);
				if (deferred) {
					await daemon.until("settled");
					expect(of(daemon, "message_accepted").some(item => item.message_id === "steer-1")).toBe(false);
					expect(userTexts(daemon)).not.toContain("steered");
					daemon.send("ticket_resolved", {
						ticket_id: "ticket-1",
						tool_call_id: "call-1",
						resolution: { status: "result", for_model: "ok" },
					});
				}
				await daemon.until("settled");
				expect(userTexts(daemon).filter(text => text === "steered")).toHaveLength(1);
				expect(of(daemon, "message_accepted").filter(item => item.message_id === "steer-1")).toHaveLength(1);
				expect(of(daemon, "message_result").filter(item => item.message_id === "steer-1")).toHaveLength(1);
			});
		});
	}

	it("test_idle_non_owner_burst_batches_and_settles_every_id", async () => {
		installScript([textResponse("batch complete")]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open();
			for (const index of [0, 1, 2])
				daemon.user({ message_id: `report-${index}`, text: `report ${index}`, mode: "queue" });
			await daemon.until("settled");
			expect(userTexts(daemon)).toEqual(["report 0", "report 1", "report 2"]);
			expect(of(daemon, "message_result").map(item => [item.message_id, item.outcome])).toEqual(
				[0, 1, 2].map(index => [`report-${index}`, "completed"]),
			);
			expect(eventKinds(daemon).filter(kind => kind === "turn.started")).toHaveLength(1);
		});
	});

	for (const [count, size, turns] of [
		[33, 1, 2],
		[3, 600_000, 3],
	] as const) {
		it(`test_non_owner_batches_bound_count_and_bytes[${count}-${size}-${turns}]`, async () => {
			installScript(Array.from({ length: turns }, () => textResponse("batch complete")));
			await withFakeDaemon(session(), async daemon => {
				await daemon.open({ agent: { compaction_max_tokens: 2_000_000 } });
				for (let index = 0; index < count; index++) {
					daemon.user({ message_id: `bounded-${index}`, text: "x".repeat(size), mode: "queue" });
				}
				await daemon.until("settled", { timeoutMs: 20_000 });
				expect(of(daemon, "message_result")).toHaveLength(count);
				expect(eventKinds(daemon).filter(kind => kind === "turn.started")).toHaveLength(turns);
			});
		});
	}

	it("test_owner_bypasses_idle_report_batch", async () => {
		installScript([textResponse("owner"), textResponse("reports")]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open();
			daemon.user({ message_id: "report", text: "queued report", mode: "queue" });
			daemon.user({ message_id: "owner", text: "owner update", mode: "steer" });
			await daemon.until("settled");
			expect(of(daemon, "message_accepted").map(item => item.message_id)).toEqual(["owner", "report"]);
			expect(userTexts(daemon)).toEqual(["owner update", "queued report"]);
		});
	});

	it("test_owner_steers_before_report_without_replaying_mutation", async () => {
		installScript([toolResponse(), textResponse("owner updated"), textResponse("report read")]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user({ message_id: "initial", text: "perform action", mode: "steer" });
			const call = await daemon.until("tool_call");
			daemon.user({ message_id: "report", text: "routine report", mode: "queue" });
			daemon.user({ message_id: "owner", text: "changed requirement", mode: "steer" });
			daemon.result(call, { status: "done", for_model: "effect committed" });
			await daemon.until("settled");
			expect(of(daemon, "message_accepted").map(item => item.message_id)).toEqual(["initial", "owner", "report"]);
			expect(of(daemon, "tool_call")).toHaveLength(1);
			expect(resultIds(daemon)).toEqual(["initial", "owner", "report"]);
		});
	});

	it("test_queue_processed_in_new_run", async () => {
		installScript([toolResponse(), textResponse("first"), textResponse("second")]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			const call = await daemon.until("tool_call");
			daemon.user({ message_id: "queued", text: "queued text", mode: "queue" });
			daemon.result(call, { status: "done", for_model: "ok" });
			await daemon.until("settled");
			expect(resultIds(daemon)).toEqual(["message-1", "queued"]);
			const snapshots = daemon.snapshots();
			expect(snapshots).toHaveLength(2);
			expect(JSON.stringify(JSON.parse(snapshots[0]?.data).messages)).not.toContain("queued text");
			expect(userTexts(daemon).filter(text => text === "queued text")).toHaveLength(1);
		});
	});

	it("test_abort_mid_tool_preserves_undelivered_steer", async () => {
		installScript([toolResponse(), textResponse("after abort")]);
		let interrupted: Wire | undefined;
		await withFakeDaemon(session(), async (daemon, live) => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			await daemon.until("tool_call");
			daemon.user({ message_id: "pending", text: "undelivered", mode: "steer" });
			daemon.send("abort");
			await daemon.until("settled");
			expect(live.rpc.pendingCount).toBe(0);
			expect(live.parentRunning).toBe(false);
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["aborted"]);
			expect(userTexts(daemon)).not.toContain("undelivered");
			interrupted = daemon.snapshots().at(-1);
			expect(JSON.parse(interrupted?.data).steer_buffer[0].message_id).toBe("pending");
			daemon.user({ message_id: "new", text: "restart" });
			await daemon.until("settled");
			expect(userTexts(daemon).filter(text => text === "undelivered")).toHaveLength(1);
		});
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()], resume_state: interrupted });
			daemon.user({ message_id: "after-restart", text: "reconcile interrupted work" });
			await daemon.until("settled");
			expect(userTexts(daemon)).not.toContain("undelivered");
			expect(of(daemon, "tool_call")).toHaveLength(0);
		});
	});

	for (const midrun of [false, true]) {
		it(`test_auth_unavailable_is_fatal[${midrun}]`, async () => {
			const options: SessionOptions = midrun
				? {
						model: createMockModel({ handler: { content: ["unused"] } }),
						streamFn: async () => {
							throw new AuthUnavailable("test", ["synthetic unavailable"]);
						},
					}
				: {};
			await withFakeDaemon(session(options), async daemon => {
				let error: Wire;
				if (midrun) {
					await daemon.open();
					daemon.user();
					error = await daemon.receive();
					while (error.type !== "error") error = await daemon.receive();
				} else {
					daemon.send("open", {
						session: "auth",
						model: { id: "unknown-provider:model", fallback: [] },
						tools: [],
						system_prompt_parts: [],
						config: { credentials: {}, agent: {} },
					});
					error = await daemon.receive();
				}
				expect(error.code).toBe("auth_unavailable");
				expect(error.fatal).toBe(true);
			});
		});
	}

	for (const [name, options, reason] of [
		[
			"thrown",
			{
				model: createMockModel({ handler: { content: ["unused"] } }),
				streamFn: async () => {
					throw new TypeError("fetch https://relay.invalid/v1?key=sk-secret failed");
				},
			},
			"Model run failed (TypeError)",
		],
		[
			"provider",
			{ model: createMockModel({ handler: { throw: "status_code: 429, body: provider rejected request" } }) },
			"Model run failed (status_code: 429, body: provider rejected request)",
		],
		[
			"effort",
			{
				model: createMockModel({ handler: { content: ["unused"] } }),
				streamFn: async () => {
					throw new EffortUnsupported("xhigh", "anthropic:claude-haiku-5-5", ["low", "medium", "high"]);
				},
			},
			"Model run failed (effort xhigh is not supported by anthropic:claude-haiku-5-5; supported: low, medium, high)",
		],
		[
			"bounded",
			{ model: createMockModel({ handler: { throw: "x".repeat(600) } }) },
			`Model run failed (${"x".repeat(493)}…`,
		],
	] as const) {
		it(`test_failed_run_reports_a_sanitized_reason[${name}]`, async () => {
			await withFakeDaemon(session(options as SessionOptions), async daemon => {
				await daemon.open();
				daemon.user();
				await daemon.until("settled");
				const [result] = of(daemon, "message_result");
				expect(result?.outcome).toBe("failed");
				expect(result?.reason).toBe(reason);
				const error = of(daemon, "error").find(item => item.code === "model_error");
				expect(error?.message).toBe(`${reason}; check provider configuration`);
				expect(JSON.stringify(daemon.outputs)).not.toContain("sk-secret");
			});
		});
	}

	for (const [name, agent, fixed, reasoning] of [
		["runtime", { orchestration: "front", front_tools: [] }, undefined, "minimal"],
		["person", { orchestration: "front", front_tools: [], effort: "xhigh" }, true, "xhigh"],
	] as const) {
		it(`test_effort_origin_reaches_the_stream_and_usage[${name}]`, async () => {
			const silent = createMockModel({
				handler: { content: [{ type: "toolCall", name: "stay_silent", arguments: {} }] },
			});
			const seen: { fixed: boolean | undefined; reasoning: string | undefined }[] = [];
			const options: SessionOptions = {
				makeStreamFn: (_port, ctx) => (_model, context, streamOptions) => {
					if (ctx.onServedEffort === undefined) return streamSimple(silent, context, streamOptions);
					seen.push({ fixed: ctx.effortFixed, reasoning: streamOptions?.reasoning });
					ctx.onServedEffort("low" as Effort);
					return streamSimple(silent, context, streamOptions);
				},
			};
			await withFakeDaemon(session(options), async daemon => {
				await daemon.open({ model: "anthropic:claude-haiku-5-5", agent: { ...agent, title_needed: false } });
				daemon.user();
				await daemon.until("settled");
				expect(seen[0]).toEqual({ fixed, reasoning });
				const usage = of(daemon, "event").find(item => item.event.kind === "usage");
				expect(usage?.event.effort).toBe("low");
				// The parent agent's usage carries its model's window for the chat's context meter.
				expect(usage?.event.context_window).toBeGreaterThan(0);
			});
		});
	}

	it("test_canary_not_in_snapshot_events_or_logs", async () => {
		const canary = "vault-like-canary-do-not-disclose-893242";
		process.env.OPENSIDE_BROKER_TOKEN = canary;
		installScript([textResponse("public answer")]);
		const written: string[] = [];
		const original = process.stderr.write.bind(process.stderr);
		process.stderr.write = ((chunk: string | Uint8Array) => {
			written.push(String(chunk));
			return true;
		}) as typeof process.stderr.write;
		try {
			await withFakeDaemon(session(), async daemon => {
				await daemon.open();
				daemon.user();
				await daemon.until("settled");
				expect(JSON.stringify(daemon.outputs)).not.toContain(canary);
			});
		} finally {
			process.stderr.write = original;
		}
		expect(written.join("")).not.toContain(canary);
	});

	it("test_steer_after_last_model_request_starts_new_run", async () => {
		const first = { parts: [{ type: "wait_steer" }, { type: "text", text: "first" }] };
		installScript([first, textResponse("second")], { delayMs: 5000 });
		await withFakeDaemon(session(), async daemon => {
			await daemon.open();
			daemon.user();
			await daemon.until("event", { kind: "turn.started" });
			daemon.user({ message_id: "steer", text: "late steer", mode: "steer" });
			await daemon.until("settled");
			expect(resultIds(daemon)).toEqual(["message-1", "steer"]);
			expect(daemon.snapshots()).toHaveLength(2);
		});
	}, 20_000);

	it("test_snapshot_ignores_retired_pins_and_restores_script_cursor", async () => {
		installScript([textResponse("first"), textResponse("second")]);
		let snapshot: Wire = {};
		await withFakeDaemon(session(), async daemon => {
			await daemon.open();
			daemon.user();
			await daemon.until("settled");
			snapshot = daemon.snapshots().at(-1) as Wire;
			const old = JSON.parse(snapshot.data);
			expect("pins" in old).toBe(false);
			old.pins = '{"test-session":{"synthetic":{"credential_id":17,"last_request_at":25}}}';
			snapshot = { ...snapshot, data: JSON.stringify(old) };
		});
		await withFakeDaemon(session(), async (daemon, live) => {
			await daemon.open({ resume_state: snapshot });
			expect("pins" in live).toBe(false);
			daemon.user({ message_id: "continued" });
			const final = await daemon.until("event", { kind: "assistant.final" });
			expect(final.event.text).toBe("second");
			await daemon.until("settled");
			const state = JSON.parse(daemon.snapshots().at(-1)?.data);
			expect(state.script_cursor).toBe(2);
			expect("pins" in state).toBe(false);
		});
	});

	it("test_shutdown_mid_tool_aborts_snapshots_then_bye", async () => {
		installScript([toolResponse()]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			await daemon.until("tool_call");
			daemon.send("shutdown");
			await daemon.until("bye");
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["aborted"]);
			expect(daemon.outputs.at(-2)?.type).toBe("state_snapshot");
			expect(daemon.outputs.at(-1)?.type).toBe("bye");
		});
	});

	it("test_thinking_stream_is_forwarded_without_tool_lifecycle", async () => {
		const model = createMockModel({
			handler: { content: [{ type: "thinking", thinking: "public reasoning continuation" }, "public answer"] },
		});
		await withFakeDaemon(session({ model }), async daemon => {
			await daemon.open();
			daemon.user();
			await daemon.until("settled");
			const events = of(daemon, "event").map(item => item.event);
			const joined = (kind: string) =>
				events
					.filter(event => event.kind === kind)
					.map(event => event.text)
					.join("");
			expect(joined("thinking.delta")).toBe("public reasoning continuation");
			expect(joined("assistant.delta")).toBe("public answer");
			expect(events.some(event => event.kind.startsWith("tool."))).toBe(false);
		});
	});

	it("test_received_fatal_error_closes_without_more_output", async () => {
		installScript([toolResponse()]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			await daemon.until("tool_call");
			const before = daemon.outputs.length;
			daemon.send("error", { code: "internal", message: "Daemon stopped", fatal: true });
			await expect(daemon.receive(5000)).rejects.toThrow("Sidecar closed");
			expect(daemon.outputs).toHaveLength(before);
		});
	});

	it("test_model_cannot_forward_private_memory_operation", async () => {
		installScript([toolResponse("__memory_consolidate"), textResponse()]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec("__memory_consolidate")], agent: { tool_retries: 0 } });
			daemon.user({ text: "delete everything" });
			const result = await daemon.until("message_result");
			expect(result.outcome).toBe("failed");
			await daemon.until("settled");
			expect(of(daemon, "tool_call")).toHaveLength(0);
		});
	});

	for (const front of [null, "provider:fast"]) {
		it(`test_consolidation_model_uses_front_then_reasoning[${front ?? "None"}]`, async () => {
			// Catalog ids stand in for the retired test ids; the route, not the model, is under test.
			const reasoning = "anthropic:claude-sonnet-5-5";
			const frontId = front === null ? null : "openai:gpt-6-luna";
			const routes: { model: string; fallback: string[] }[] = [];
			const extracted: string[] = [];
			const silent = createMockModel({
				handler: { content: [{ type: "toolCall", name: "stay_silent", arguments: {} }] },
			});
			const options: SessionOptions = {
				// Python patched providers.build_model; this records each route and streams the mock.
				makeStreamFn: (_port, ctx) => {
					routes.push({ model: ctx.model, fallback: [...(ctx.fallback ?? [])] });
					return (_model, context, streamOptions) => streamSimple(silent, context, streamOptions);
				},
				complete: async request => {
					extracted.push(`${request.model.provider}:${request.model.id}`);
					return { text: '{"edits":[],"additions":[]}', model: request.model.id, input: 0, output: 0 };
				},
			};
			await withFakeDaemon(session(options), async daemon => {
				await daemon.open({
					model: reasoning,
					agent: {
						orchestration: "front",
						front_tools: [],
						...(frontId !== null ? { front_model: frontId } : {}),
						title_needed: false,
						memory_context: { scope: "opaque" },
					},
				});
				daemon.user();
				const recall = await daemon.until("tool_call");
				expect(recall.name).toBe("__memory_context");
				daemon.result(recall, { status: "done", for_model: { scope: "opaque" } });
				await daemon.until("settled");
				const consolidate = await daemon.until("tool_call");
				expect(consolidate.name).toBe("__memory_consolidate");
				expect(consolidate.args.scope).toBe("opaque");
				daemon.result(consolidate, { status: "done", for_model: { applied: true } });
			});
			expect(extracted).toEqual([frontId ?? reasoning]);
			expect(routes.at(-1)).toEqual({ model: frontId ?? reasoning, fallback: [reasoning] });
		});
	}

	it("test_takeover_continuation_captures_and_checks_before_reply", async () => {
		installScript([
			{ parts: [{ type: "tool_call", name: "request_human_step", args: { tab_id: "tab-1" }, id: "takeover" }] },
			{
				parts: [
					{ type: "tool_call", name: "browser_snapshot", args: { tab_id: "{{result:takeover.tab_id}}" }, id: "fresh" },
				],
			},
			textResponse("Verified the signed-in account on the fresh page."),
		]);
		const tools = ["request_human_step", "browser_snapshot"].map(name => ({
			name,
			description: name,
			parameters: {
				type: "object",
				properties: { tab_id: { type: "string" } },
				required: ["tab_id"],
				additionalProperties: false,
			},
			approval: "read",
			deferrable: true,
		}));
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools });
			daemon.user({ text: "Sign in and verify the account." });
			const takeover = await daemon.until("tool_call");
			daemon.result(takeover, { status: "deferred", kind: "external", ticket_id: "takeover-1" });
			await daemon.until("settled");
			daemon.send("ticket_resolved", {
				tool_call_id: "takeover",
				ticket_id: "takeover-1",
				resolution: {
					status: "result",
					for_model: {
						status: "done",
						tab_id: "tab-1",
						page_changed: true,
						note:
							"Start the continuation with a fresh browser_snapshot; inspect " +
							"the requested outcome before replying. Earlier refs are void.",
					},
				},
			});
			const fresh = await daemon.until("tool_call");
			expect(fresh.name).toBe("browser_snapshot");
			expect(fresh.args).toEqual({ tab_id: "tab-1" });
			expect(eventKinds(daemon)).not.toContain("assistant.final");
			daemon.result(fresh, { status: "done", for_model: "[snapshot_id fresh-1]\nAccount settings\nSigned in" });
			const final = await daemon.until("event", { kind: "assistant.final" });
			expect(final.event.text).toBe("Verified the signed-in account on the fresh page.");
			await daemon.until("settled");
			expect(of(daemon, "tool_call").map(item => item.name)).toEqual(["request_human_step", "browser_snapshot"]);
			expect(of(daemon, "message_accepted")).toHaveLength(1);
		});
	});

	for (const [effort, speed] of [
		["high", "fast"],
		["off", "default"],
	] as const) {
		it(`session_effort_and_speed_reach_every_child_agent[${effort}-${speed}]`, async () => {
			const model = createMockModel({ handler: { content: ["unused"] } });
			await withFakeDaemon(session({ model }), async (daemon, live) => {
				await daemon.open({ agent: { effort, speed, title_needed: false } });
				const child = new Child({
					id: "child-effort",
					parent_tool_call_id: "delegate-effort",
					task: "inherit the session settings",
					model: "test:child",
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
				live.subs.childAgent(child);
				for (const options of [live.agent?.options, child.agent?.options]) {
					expect(options?.reasoning as string | undefined).toBe(effort === "off" ? undefined : effort);
					expect(options?.disableReasoning).toBe(effort === "off" ? true : undefined);
					expect(options?.serviceTier).toBe(speed === "fast" ? "priority" : undefined);
				}
			});
		});
	}

	it("front_agent_sees_exactly_the_daemon_front_tools", async () => {
		const model = createMockModel({ handler: { content: ["unused"] } });
		await withFakeDaemon(session({ model }), async (daemon, live) => {
			await daemon.open({
				tools: [toolSpec("present"), toolSpec("send_message"), toolSpec()],
				agent: { orchestration: "front", front_tools: ["send_message"] },
			});
			const names = live.agent?.options.tools.map(tool => tool.name) ?? [];
			expect(names).toContain("send_message");
			expect(names).not.toContain("present");
			expect(names).not.toContain("probe");
		});
	});

	it("unexposed_tool_call_returns_an_error_result_to_the_model", async () => {
		const model = createMockModel({
			responses: [
				{ content: [{ type: "toolCall", name: "present", arguments: { card: {} } }] },
				{ content: ["recovered"] },
			],
		});
		await withFakeDaemon(session({ model }), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			const final = await daemon.until("event", { kind: "assistant.final" });
			expect(final.event.text).toBe("recovered");
			await daemon.until("settled");
			expect(of(daemon, "tool_call")).toHaveLength(0);
			const retry = model.calls[1]?.context.messages.find(message => message.role === "toolResult");
			expect(retry).toMatchObject({ toolName: "present", isError: true });
			expect(JSON.stringify(retry?.content)).toContain("Unknown tool name: 'present'. Available tools: 'probe'");
		});
	});

	it("repeated_unknown_tool_calls_exhaust_the_retry_budget_and_fail_the_run", async () => {
		const unknown = { content: [{ type: "toolCall" as const, name: "not_a_tool", arguments: {} }] };
		const model = createMockModel({ responses: [unknown, unknown, unknown, unknown, { content: ["unreached"] }] });
		await withFakeDaemon(session({ model }), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			await daemon.until("settled");
			expect(model.calls).toHaveLength(4);
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["failed"]);
		});
	});

	it("alternating_unknown_tools_are_counted_per_name_across_steps", async () => {
		// pydantic-ai 2.55 ToolManager: counts are per tool name and survive steps that do not
		// call the tool, so a,b,a,b,a,b each reach 3 and the fourth `a` (7th request) fails.
		const call = (name: string) => ({ content: [{ type: "toolCall" as const, name, arguments: {} }] });
		const responses = ["a", "b", "a", "b", "a", "b", "a", "b"].map(call);
		const model = createMockModel({ responses: [...responses, { content: ["unreached"] }] });
		await withFakeDaemon(session({ model }), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			await daemon.until("settled");
			expect(model.calls).toHaveLength(7);
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["failed"]);
		});
	});

	it("a_tool_success_clears_only_its_own_retry_count", async () => {
		// probe fails three times, then succeeds; the fresh budget lets three more failures pass.
		installScript(
			[1, 2, 3, 4, 5, 6, 7].map(step => toolResponse("probe", `call-${step}`)).concat([textResponse("recovered")]),
		);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			for (let step = 1; step <= 7; step++) {
				const outcome: Wire =
					step === 4 ? { status: "done", for_model: "ok" } : { status: "error", message: "Tool unavailable" };
				daemon.result(await daemon.until("tool_call"), outcome);
			}
			await daemon.until("settled");
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["completed"]);
		});
	});

	it("a_tool_that_succeeds_and_fails_in_one_step_still_counts_the_failure", async () => {
		// pydantic-ai 2.55 for_run_step drops succeeded tools first, then sets every failed tool
		// to its previous count + 1, so a mixed step does not clear the count: 1, 2, 3, then fail.
		const both = { parts: [toolResponse("probe", "call-3a").parts[0], toolResponse("probe", "call-3b").parts[0]] };
		installScript([
			toolResponse("probe", "call-1"),
			toolResponse("probe", "call-2"),
			both,
			toolResponse("probe", "call-4"),
			textResponse(),
		]);
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			const error: Wire = { status: "error", message: "Tool unavailable" };
			for (let step = 0; step < 2; step++) daemon.result(await daemon.until("tool_call"), error);
			for (let call = 0; call < 2; call++) {
				const next = await daemon.until("tool_call");
				daemon.result(next, next.tool_call_id === "call-3a" ? { status: "done", for_model: "ok" } : error);
			}
			daemon.result(await daemon.until("tool_call"), error);
			await daemon.until("settled");
			expect(of(daemon, "message_result").map(item => item.outcome)).toEqual(["failed"]);
		});
	});

	it("scripted_child_with_bare_script_model_runs", async () => {
		installScript(
			[
				{ parts: [{ type: "tool_call", name: "delegate", args: { task: "inspect" }, id: "del-1" }] },
				textResponse("parent done"),
			],
			{ subagents: { "del-1": [textResponse("child done")] } },
		);
		const delegate = {
			name: "delegate",
			description: "Delegate a task to a subagent",
			parameters: {
				type: "object",
				properties: { task: { type: "string" }, model: { type: "string" } },
				required: ["task"],
				additionalProperties: false,
			},
			approval: "read",
			deferrable: true,
		};
		await withFakeDaemon(session(), async daemon => {
			await daemon.open({ tools: [delegate] });
			daemon.user();
			const call = await daemon.until("tool_call");
			daemon.result(call, {
				status: "done",
				for_model: { subagent_id: "sub-1", model: "script", tools: [], context: "fresh" },
			});
			let lifecycle = await daemon.until("event", { kind: "subagent.lifecycle" });
			while (lifecycle.event.status === "running")
				lifecycle = await daemon.until("event", { kind: "subagent.lifecycle" });
			expect(lifecycle.event).toMatchObject({ status: "completed", output: "child done" });
		});
	});
});
