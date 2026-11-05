import { describe, expect, it } from "bun:test";
import { Agent, type AgentTool } from "@oh-my-pi/pi-agent-core";
import type { AssistantMessage, Context, ImageContent, Message, TextContent } from "@oh-my-pi/pi-ai";
import type { MockModel } from "@oh-my-pi/pi-ai/providers/mock";
import {
	lastUserText,
	mediaDigest,
	parseScript,
	resolveRefs,
	type ScriptCursor,
	SteerSignal,
	scriptedModel,
	scriptedTitle,
} from "../src/scripted";
import { textResponse, toolResponse } from "./helpers/fake-daemon";

async function reply(model: MockModel, messages: Message[] = []): Promise<AssistantMessage> {
	const context: Context = { messages };
	return model.stream(model, context).result();
}

function text(message: AssistantMessage): string {
	return message.content.flatMap(block => (block.type === "text" ? [block.text] : [])).join("");
}

const user = (content: string | (TextContent | ImageContent)[]): Message => ({ role: "user", content, timestamp: 0 });

describe("scripted model", () => {
	it("test_nonstream_cursor_and_exhaustion", async () => {
		const script = parseScript({ responses: [textResponse("first"), textResponse("second")] });
		const cursor: ScriptCursor = { index: 0 };
		expect(text(await reply(scriptedModel(script, cursor, "parent", new SteerSignal())))).toBe("first");
		expect(cursor.index).toBe(1);
		const restored: ScriptCursor = { index: cursor.index };
		const model = scriptedModel(script, restored, "parent", new SteerSignal());
		expect(text(await reply(model))).toBe("second");
		expect(text(await reply(model))).toBe("[script exhausted]");
		expect(restored.index).toBe(2);
	});

	it("test_wait_steer_gates_script_response", async () => {
		const script = parseScript({
			responses: [{ parts: [{ type: "wait_steer" }, { type: "text", text: "released" }] }],
			delay_ms: 10_000,
		});
		const steer = new SteerSignal();
		let done = false;
		const pending = reply(scriptedModel(script, { index: 0 }, "parent", steer)).then(message => {
			done = true;
			return message;
		});
		// A negative check: the reply must still be pending after the event loop ran a while.
		await Bun.sleep(50);
		expect(done).toBe(false);
		steer.set();
		expect(text(await pending)).toBe("released");
	});

	it("test_nonstream_tool_call_keeps_supplied_id", async () => {
		const script = parseScript({ responses: [toolResponse("probe", "stable-id")] });
		const message = await reply(scriptedModel(script, { index: 0 }, "parent", new SteerSignal()));
		const [call, ...rest] = message.content;
		expect(call?.type).toBe("toolCall");
		expect(call?.type === "toolCall" ? call.id : undefined).toBe("stable-id");
		expect(rest.some(block => block.type === "text")).toBe(false);
		expect(message.stopReason).toBe("toolUse");
	});

	it("test_responses_report_word_count_usage", async () => {
		const script = parseScript({
			responses: [textResponse("two words"), { parts: [{ type: "tool_call", name: "probe", args: { value: "x" } }] }],
		});
		const model = scriptedModel(script, { index: 0 }, "parent", new SteerSignal());
		const answer = await reply(model, [user("three word prompt")]);
		expect(answer.usage).toMatchObject({ input: 53, output: 2 });
		const call = await reply(model, [user("three word prompt"), answer]);
		// One token per call plus its JSON argument words: `{"value":"x"}` -> `{`, value, x, `}`.
		expect(call.usage).toMatchObject({ input: 55, output: 1 + 4 });
	});

	it("test_subagent_script_keyed_by_delegate_call_with_own_cursor", async () => {
		const script = parseScript({
			responses: [textResponse("parent")],
			subagents: { "del-1": [textResponse("child one"), textResponse("child two")] },
		});
		const parent: ScriptCursor = { index: 0 };
		scriptedModel(script, parent, "parent", new SteerSignal());
		const child: ScriptCursor = { index: 1 };
		expect(text(await reply(scriptedModel(script, child, { subagent: "del-1" }, new SteerSignal())))).toBe("child two");
		expect(child.index).toBe(2);
		expect(parent.index).toBe(0);
		const unknown = scriptedModel(script, { index: 0 }, { subagent: "del-2" }, new SteerSignal());
		expect(text(await reply(unknown))).toBe("[script exhausted]");
	});

	it("test_wait_steer_timeout_overrides_script_delay", async () => {
		const script = parseScript({
			responses: [
				{
					parts: [
						{ type: "wait_steer", timeout_ms: 10 },
						{ type: "text", text: "timed out" },
					],
				},
			],
			delay_ms: 60_000,
		});
		const started = Date.now();
		expect(text(await reply(scriptedModel(script, { index: 0 }, "parent", new SteerSignal())))).toBe("timed out");
		expect(Date.now() - started).toBeLessThan(5_000);
	});

	it("test_result_refs_follow_nested_fields_and_list_indexes", async () => {
		const script = parseScript({
			responses: [
				toolResponse("tabs", "list"),
				{
					parts: [{ type: "tool_call", name: "read_tab", args: { tab_id: "{{result:list.0.tab_id}}" }, id: "read" }],
				},
				textResponse("done"),
			],
		});
		const seen: unknown[] = [];
		const tool = (name: string, details: unknown): AgentTool => ({
			name,
			label: name,
			description: name,
			parameters: { type: "object", properties: {}, additionalProperties: true } as never,
			lenientArgValidation: true,
			execute: async (_id, params) => {
				if (name === "read_tab") seen.push((params as { tab_id: unknown }).tab_id);
				return { content: [{ type: "text", text: JSON.stringify(details) }], details };
			},
		});
		const model = scriptedModel(script, { index: 0 }, "parent", new SteerSignal());
		const agent = new Agent({
			initialState: {
				systemPrompt: [],
				model,
				tools: [tool("tabs", [{ tab_id: "assigned-tab" }]), tool("read_tab", "read")],
			},
		});
		await agent.prompt("read it");
		const last = agent.state.messages.at(-1) as AssistantMessage;
		expect(text(last)).toBe("done");
		expect(seen).toEqual(["assigned-tab"]);
	});

	it("test_result_refs_resolve_nested_attachment_arguments", () => {
		const messages: Message[] = [
			{
				role: "toolResult",
				toolCallId: "status",
				toolName: "get_tasks",
				content: [],
				details: { tasks: [{ artifacts: ["documents/tasks/task/report.md"] }] },
				isError: false,
				timestamp: 0,
			},
		];
		expect(resolveRefs({ attachments: [{ artifact: "{{result:status.tasks.0.artifacts.0}}" }] }, messages)).toEqual({
			attachments: [{ artifact: "documents/tasks/task/report.md" }],
		});
	});

	it("echo replies with the latest user text and skips a provenance block", async () => {
		const script = parseScript({ responses: [{ parts: [{ type: "echo" }] }, { parts: [{ type: "echo" }] }] });
		const model = scriptedModel(script, { index: 0 }, "parent", new SteerSignal());
		expect(text(await reply(model, [user("first"), user("steered")]))).toBe("steered");
		const provenance = JSON.stringify({ provenance: { source: "channel", authority: "data_only" } });
		expect(
			text(
				await reply(model, [
					user([
						{ type: "text", text: provenance },
						{ type: "text", text: "from channel" },
					]),
				]),
			),
		).toBe("from channel");
		const tail: Message = { role: "user", content: "memory tail", synthetic: true, timestamp: 0 };
		expect(lastUserText([user("prompt"), tail])).toBe("prompt");
	});

	it("media_digest hashes user and tool images in order", () => {
		const png = Buffer.from("png-bytes").toString("base64");
		const shot = Buffer.from("screenshot").toString("base64");
		const hash = (data: string) => new Bun.CryptoHasher("sha256").update(Buffer.from(data, "base64")).digest("hex");
		expect(mediaDigest([])).toBe("media none");
		const messages: Message[] = [
			user([
				{ type: "text", text: "look" },
				{ type: "image", data: png, mimeType: "image/png" },
			]),
			{
				role: "toolResult",
				toolCallId: "c",
				toolName: "shot",
				content: [{ type: "image", data: shot, mimeType: "image/jpeg" }],
				isError: false,
				timestamp: 0,
			},
		];
		expect(mediaDigest(messages)).toBe(`media image/png:${hash(png)} image/jpeg:${hash(shot)}`);
	});

	it("rejects unknown part fields and types and defaults the title", () => {
		expect(() => parseScript({ responses: [{ parts: [{ type: "text", extra: 1 }] }] })).toThrow("Invalid agent script");
		expect(() => parseScript({ responses: [{ parts: [{ type: "dance" }] }] })).toThrow("Invalid agent script");
		expect(() => parseScript({ responses: [], delay_ms: -1 })).toThrow("Invalid agent script");
		expect(scriptedTitle(parseScript({ responses: [] }))).toBe("Discuss the request");
		expect(scriptedTitle(parseScript({ responses: [], title: "Plan trip" }))).toBe("Plan trip");
	});
});
