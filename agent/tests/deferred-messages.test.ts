import { beforeEach, describe, expect, it } from "bun:test";
import { Session } from "../src/runtime/session";
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

function start(conn: JsonlConnection, rpc: RpcRouter) {
	const session = new Session(conn, rpc);
	return { session, done: session.serve() };
}

function results(daemon: FakeDaemon): [string, string][] {
	return daemon.outputs.filter(item => item.type === "message_result").map(item => [item.message_id, item.outcome]);
}

async function defer(daemon: FakeDaemon, ticket: string): Promise<Wire> {
	const call = await daemon.until("tool_call");
	daemon.result(call, { status: "deferred", kind: "external", ticket_id: ticket });
	await daemon.until("settled");
	return call;
}

function resolve(daemon: FakeDaemon, callId: string, ticket: string): void {
	daemon.send("ticket_resolved", {
		ticket_id: ticket,
		tool_call_id: callId,
		resolution: { status: "result", for_model: "filled" },
	});
}

function lastState(daemon: FakeDaemon): Wire {
	return JSON.parse(daemon.snapshots().at(-1)?.data);
}

beforeEach(() => hermeticEnv());

describe("deferred messages", () => {
	it("test_resume_completes_originating_message_across_redeferral_and_restart", async () => {
		installScript([toolResponse("probe", "call-1"), toolResponse("probe", "call-2"), textResponse()]);
		let snapshot: Wire | undefined;
		await withFakeDaemon(start, async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			const first = await defer(daemon, "ticket-1");
			expect(results(daemon)).toEqual([["message-1", "deferred"]]);
			resolve(daemon, first.tool_call_id, "ticket-1");
			await defer(daemon, "ticket-2");
			expect(results(daemon)).toEqual([["message-1", "deferred"]]);
			snapshot = daemon.snapshots().at(-1);
			expect(lastState(daemon).deferred_messages).toEqual(["message-1"]);
		});
		await withFakeDaemon(start, async daemon => {
			const ready = await daemon.open({ tools: [toolSpec()], resume_state: snapshot });
			expect(ready.pending_deferred[0].tool_call_id).toBe("call-2");
			resolve(daemon, "call-2", "ticket-2");
			await daemon.until("settled");
			expect(results(daemon)).toEqual([["message-1", "completed"]]);
			expect(lastState(daemon).deferred_messages).toEqual([]);
		});
	});

	it("test_discard_drops_restored_continuation_without_rerunning_it", async () => {
		installScript([toolResponse("probe", "call-1"), textResponse("resumed"), textResponse("next")]);
		let snapshot: Wire | undefined;
		await withFakeDaemon(start, async daemon => {
			await daemon.open({ tools: [toolSpec()] });
			daemon.user();
			await defer(daemon, "ticket-1");
			snapshot = daemon.snapshots().at(-1);
		});
		await withFakeDaemon(start, async daemon => {
			const ready = await daemon.open({ tools: [toolSpec()], resume_state: snapshot });
			expect(ready.pending_deferred[0].ticket_id).toBe("ticket-1");
			daemon.send("discard_deferred");
			const state = JSON.parse((await daemon.until("state_snapshot")).state.data);
			expect(state.pending_deferred).toEqual([]);
			expect(state.deferred_messages).toEqual([]);
			// The unanswered batch left the model's history, so the daemon must retire any
			// tool-result base (browser snapshot id) that lived only in it.
			const notices = daemon.outputs.filter(item => item.type === "context_compacted");
			expect(notices).toHaveLength(1);
			expect("subagent" in (notices[0] as Wire)).toBe(false);
			daemon.user({ message_id: "message-2", text: "next" });
			await daemon.until("settled");
			expect(results(daemon)).toEqual([["message-2", "completed"]]);
			expect(daemon.outputs.some(item => item.type === "error" || item.type === "tool_call")).toBe(false);
		});
	});
});
