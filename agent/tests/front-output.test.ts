import { beforeEach, describe, expect, it } from "bun:test";
import type { Message } from "@oh-my-pi/pi-ai";
import type { UserMessage } from "../src/protocol/types";
import { syntheticUser, userPrompt } from "../src/runtime/content";
import { Session } from "../src/runtime/session";
import { resolveRefs } from "../src/scripted";
import type { JsonlConnection } from "../src/transport/jsonl";
import type { RpcRouter } from "../src/transport/rpc";
import {
	type FakeDaemon,
	hermeticEnv,
	installScript,
	textResponse,
	type Wire,
	withFakeDaemon,
} from "./helpers/fake-daemon";

function start(conn: JsonlConnection, rpc: RpcRouter) {
	const session = new Session(conn, rpc);
	return { session, done: session.serve() };
}

const SEND = {
	name: "send_message",
	description: "Send one useful message to the person.",
	parameters: {
		type: "object",
		properties: {
			text: { type: "string" },
			attachments: { type: "array", items: { type: "object" } },
		},
		required: ["text"],
		additionalProperties: false,
	},
	approval: "read",
	deferrable: false,
};
const FRONT = { orchestration: "front", front_tools: ["send_message"], title_needed: false };

function output(name: string, args: Wire, callId = "finish"): Wire {
	return { parts: [{ type: "tool_call", name, args, id: callId }] };
}

function bubbles(daemon: FakeDaemon): Wire[] {
	return daemon.outputs.filter(
		item => item.type === "event" && ["assistant.final", "assistant.delta", "thinking.delta"].includes(item.event.kind),
	);
}

function outcomes(daemon: FakeDaemon): string[] {
	return daemon.outputs.filter(item => item.type === "message_result").map(item => item.outcome);
}

beforeEach(() => hermeticEnv());

describe("front output", () => {
	it("test_dynamic_front_output_uses_report_not_ephemeral_context", () => {
		const report = {
			type: "user_message",
			message_id: "report",
			text: "[task report] durable result",
			mode: "queue",
			author_kind: "internal",
			parts: [],
		} as unknown as UserMessage;
		const messages = [userPrompt(report), syntheticUser("memory context")] as Message[];
		expect(resolveRefs({ text: "{{user_text}}" }, messages)).toEqual({ text: "[task report] durable result" });
	});

	it("test_send_message_is_terminal_and_only_daemon_publishes", async () => {
		const args = {
			text: "Here is the result.",
			attachments: [{ artifact: "doc/report.md", display: "panel" }],
		};
		installScript([output("send_message", args)]);
		await withFakeDaemon(start, async (daemon, session) => {
			await daemon.open({ tools: [SEND], agent: FRONT });
			daemon.user();
			const call = await daemon.until("tool_call");
			expect(call.name).toBe("send_message");
			expect(call.tool_call_id).toBe("finish");
			expect(call.args).toEqual(args);
			daemon.result(call, { status: "done", for_model: { status: "sent" } });
			await daemon.until("settled");
			expect(session.cursor.index).toBe(1);
			expect(bubbles(daemon)).toEqual([]);
			expect(outcomes(daemon)).toEqual(["completed"]);
		});
	});

	it("test_silent_report_ends_without_empty_text_retry_or_bubble", async () => {
		installScript([output("stay_silent", {})]);
		await withFakeDaemon(start, async (daemon, session) => {
			await daemon.open({ tools: [SEND], agent: FRONT });
			daemon.send("user_message", {
				message_id: "report",
				text: "No useful update",
				mode: "queue",
				author_kind: "internal",
			});
			await daemon.until("settled");
			expect(session.cursor.index).toBe(1);
			expect(bubbles(daemon)).toEqual([]);
			expect(daemon.outputs.some(item => item.type === "tool_call")).toBe(false);
			expect(outcomes(daemon)).toEqual(["completed"]);
		});
	});

	for (const failure of [
		{ status: "denied", reason: "attachment[0]:not_found", hint: "Create the document first" },
		{ status: "error", message: "Message must not be empty" },
	]) {
		it(`test_daemon_rejection_retries_output_without_claiming_delivery[${failure.status}]`, async () => {
			installScript([output("send_message", { text: "bad" }, "bad"), output("send_message", { text: "good" }, "good")]);
			await withFakeDaemon(start, async (daemon, session) => {
				await daemon.open({ tools: [SEND], agent: FRONT });
				daemon.user();
				const first = await daemon.until("tool_call");
				daemon.result(first, failure);
				const second = await daemon.until("tool_call");
				expect(second.tool_call_id).toBe("good");
				daemon.result(second, { status: "done", for_model: { status: "sent" } });
				await daemon.until("settled");
				expect(session.cursor.index).toBe(2);
				expect(bubbles(daemon)).toEqual([]);
			});
		});
	}

	it("test_plain_text_is_not_a_front_output", async () => {
		installScript([textResponse("Do not publish this"), output("stay_silent", {})]);
		await withFakeDaemon(start, async (daemon, session) => {
			await daemon.open({ tools: [SEND], agent: FRONT });
			daemon.user();
			await daemon.until("settled");
			expect(session.cursor.index).toBe(2);
			expect(bubbles(daemon)).toEqual([]);
		});
	});

	it("test_multiple_terminal_messages_publish_at_most_one", async () => {
		const first = output("send_message", { text: "first" }, "first");
		first.parts.push(...output("send_message", { text: "second" }, "second").parts);
		installScript([first]);
		await withFakeDaemon(start, async daemon => {
			await daemon.open({ tools: [SEND], agent: FRONT });
			daemon.user();
			const call = await daemon.until("tool_call");
			daemon.result(call, { status: "done", for_model: { status: "sent" } });
			await daemon.until("settled");
			expect(daemon.outputs.filter(item => item.type === "tool_call")).toHaveLength(1);
			expect(bubbles(daemon)).toEqual([]);
		});
	});

	it("test_report_echo_output_survives_model_normalization_and_context_tail", async () => {
		installScript([output("send_message", { text: "{{user_text}}" })]);
		await withFakeDaemon(start, async (daemon, session) => {
			await daemon.open({ tools: [SEND], agent: FRONT });
			session.promptSections.profile = "changed profile";
			daemon.send("user_message", {
				message_id: "report",
				text: "[task report] durable result",
				mode: "queue",
				author_kind: "internal",
			});
			const call = await daemon.until("tool_call");
			expect(call.args).toEqual({ text: "[task report] durable result" });
			daemon.result(call, { status: "done", for_model: { status: "sent" } });
			await daemon.until("settled");
		});
	});
});
