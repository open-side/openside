// S-9 adoptions for every profile: input secret redaction, request re-anchoring, and
// runtime hooks that never add a model tool.
import { beforeEach, describe, expect, it } from "bun:test";
import type { Context, Message } from "@oh-my-pi/pi-ai";
import { createMockModel, type MockModel } from "@oh-my-pi/pi-ai/providers/mock";
import { REANCHOR_EVERY } from "../src/capabilities/reminders";
import { Session, type SessionOptions } from "../src/runtime/session";
import type { JsonlConnection } from "../src/transport/jsonl";
import type { RpcRouter } from "../src/transport/rpc";
import { hermeticEnv, toolSpec, withFakeDaemon } from "./helpers/fake-daemon";

// Shaped like an AWS access key id; not a real credential.
const FAKE_KEY = `AKIA${"Q".repeat(16)}`;
const TASK = `Upload the report with key ${FAKE_KEY} and summarize it.`;
// The runtime-side counterparts of the retired adopted capabilities: secret redaction,
// prompt-injection defense, system reminders, cache-bust warning, reporting compaction.
const ADOPTED = ["redact", "injection", "reminders", "cache_busts", "compaction"];

function session(options: SessionOptions = {}) {
	return (conn: JsonlConnection, rpc: RpcRouter) => {
		const started = new Session(conn, rpc, options);
		return { session: started, done: started.serve() };
	};
}

function prompts(messages: readonly Message[]): string[] {
	return messages.flatMap(message => {
		if (message.role !== "user") return [];
		if (typeof message.content === "string") return [message.content];
		return [message.content.flatMap(block => (block.type === "text" ? [block.text] : [])).join("")];
	});
}

/** Calls the probe tool `calls` times, then answers; the mock records every request. */
function looping(calls: number): MockModel {
	let requests = 0;
	return createMockModel({
		handler: () => {
			requests++;
			if (requests <= calls)
				return { content: [{ type: "toolCall", id: `c${requests}`, name: "probe", arguments: {} }] };
			return { content: ["done"] };
		},
	});
}

async function run(calls: number): Promise<Context[]> {
	const model = looping(calls);
	await withFakeDaemon(session({ model }), async daemon => {
		await daemon.open({ tools: [toolSpec()], agent: { title_needed: false } });
		daemon.user({ text: TASK });
		for (let index = 0; index < calls; index++) {
			const call = await daemon.until("tool_call");
			daemon.result(call, { status: "done", for_model: "ok" });
		}
		await daemon.until("event", { kind: "assistant.final" });
		await daemon.until("settled");
	});
	return model.calls.map(call => call.context);
}

beforeEach(() => hermeticEnv());

describe("capabilities", () => {
	it("test_user_prompt_secrets_never_reach_the_model", async () => {
		const requests = await run(0);
		const sent = prompts((requests[0] as Context).messages).join("\n");
		expect(sent).not.toContain(FAKE_KEY);
		expect(sent).toContain("Upload the report with key");
		expect(sent).toContain("redacted");
	});

	it("test_long_runs_are_reanchored_on_the_current_request", async () => {
		const requests = await run(REANCHOR_EVERY + 1);
		const reminded = requests.flatMap((context, index) =>
			prompts(context.messages).some(text => text.includes("<system-reminder>")) ? [index + 1] : [],
		);
		expect(reminded).toEqual([REANCHOR_EVERY]);
		const reminder = prompts((requests[REANCHOR_EVERY - 1] as Context).messages).find(text =>
			text.includes("<system-reminder>"),
		) as string;
		expect(reminder).toContain("Upload the report");
		expect(reminder).not.toContain(FAKE_KEY);
	});

	for (const [index, name] of ADOPTED.entries()) {
		// Capabilities are request hooks here, not toolsets: a run with no daemon tools
		// must expose no tool to the model while every hook is active.
		it(`test_adopted_capability_adds_zero_tools[${index}]`, async () => {
			const model = createMockModel({ handler: { content: [`answered with ${name} active`] } });
			await withFakeDaemon(session({ model }), async daemon => {
				const ready = await daemon.open({ agent: { title_needed: false } });
				expect(ready.tools).toEqual([]);
				daemon.user({ text: "probe" });
				const final = await daemon.until("event", { kind: "assistant.final" });
				expect(final.event.text).toBeTruthy();
				await daemon.until("settled");
			});
			expect(model.calls.length).toBeGreaterThan(0);
			for (const call of model.calls) expect(call.context.tools ?? []).toEqual([]);
		});
	}

	it("test_gate_uses_exposed_toolsets", async () => {
		const model = createMockModel({ handler: { content: ["unused"] } });
		await withFakeDaemon(session({ model }), async daemon => {
			const ready = await daemon.open({ tools: [toolSpec("one"), toolSpec("two")] });
			expect(ready.tools).toEqual(["one", "two"]);
			daemon.send("open", { session: "ignored" });
			expect((await daemon.until("error")).code).toBe("protocol");
		});
	});

	it("test_gate_rejects_added_capability_tool", async () => {
		const model = createMockModel({ handler: { content: ["unused"] } });
		await withFakeDaemon(session({ model }), async (daemon, live) => {
			await daemon.open();
			// A hook that smuggles a tool into the request must be caught by the gate.
			const agent = live.agent as NonNullable<typeof live.agent>;
			agent.options.tools.push({
				name: "extra",
				label: "extra",
				description: "extra",
				parameters: { type: "object", properties: {} } as never,
				execute: async () => ({ content: [{ type: "text", text: "extra" }], details: null }),
			});
			daemon.user();
			let error = await daemon.receive();
			while (error.type !== "error") error = await daemon.receive();
			expect(error.code).toBe("tool_gate");
			expect(error.fatal).toBe(true);
			expect(model.calls).toHaveLength(0);
		});
	});
});
