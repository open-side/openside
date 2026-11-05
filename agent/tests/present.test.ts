// The catalog `present` tool: card drafts, daemon rejections and secure outcomes.
import { beforeEach, expect, it, spyOn } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import { validateJsonSchemaValue } from "@oh-my-pi/pi-ai/utils/schema/json-schema-validator";
import { SUBAGENT_INSTRUCTIONS } from "../src/features/browse-policy.ts";
import { type JsonObject, STATE_FORMAT } from "../src/protocol/types.ts";
import { PRESENT_INSTRUCTIONS } from "../src/runtime/hooks.ts";
import { type ModelRequestInfo, Session } from "../src/runtime/session.ts";
import type { SubagentState } from "../src/runtime/state.ts";
import { CHIEF_INSTRUCTIONS, FRONT_INSTRUCTIONS, TEAMMATE_INSTRUCTIONS } from "../src/runtime/team.ts";
import {
	type FakeDaemon,
	hermeticEnv,
	installScript,
	type Wire as Message,
	textResponse,
	toolSpec,
	withFakeDaemon,
} from "./helpers/fake-daemon.ts";

const SNAPSHOTS = join(import.meta.dir, "../../docs/specs/profiles");

function catalogTool(name: string, profile = "browser-agent"): Message {
	// These snapshots are generated from the daemon's actual sidecar open payload.
	const payload = JSON.parse(readFileSync(join(SNAPSHOTS, `${profile}.open.json`), "utf8"));
	return (payload.tools as Message[]).find(tool => tool.name === name) as Message;
}

function draft(): Message {
	return {
		card: {
			id: "",
			kind: "table",
			site: { origin: null, name: "Comparison", favicon_ref: null },
			title: "Options",
			status: "done",
			blocks: [
				{
					type: "table",
					id: "options",
					columns: [{ id: "name", label: "Name" }],
					rows: [[{ type: "text", text: "Option A" }]],
					layout: "grid",
				},
			],
			actions: [],
			interaction: "none",
			collapsed_summary: "One option",
		},
		images: [],
	};
}

function call(name: string, id: string, args: JsonObject = {}) {
	return { type: "tool_call", name, args, id };
}

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

type SnapshotData = { messages: AgentMessage[]; subagents: SubagentState[] };

function snapshotData(daemon: FakeDaemon): SnapshotData {
	return JSON.parse(daemon.snapshots().at(-1)!.data);
}

/** Python ToolReturnPart ↔ a successful toolResult; RetryPromptPart ↔ an error toolResult. */
function toolResults(messages: readonly AgentMessage[], isError: boolean): Message[] {
	return (messages as unknown as Message[]).filter(
		message => message.role === "toolResult" && message.isError === isError,
	);
}

function resultText(message: Message): string {
	return (message.content as Message[]).map(block => block.text ?? "").join("");
}

function instructions(info: ModelRequestInfo): string {
	return info.systemPrompt.join("\n\n");
}

function recordingSession(requests: ModelRequestInfo[]) {
	return (...args: ConstructorParameters<typeof Session>) => {
		const session = new Session(args[0], args[1], { onModelRequest: info => requests.push(structuredClone(info)) });
		return { session, done: session.serve() };
	};
}

beforeEach(() => {
	hermeticEnv();
});

it("test_present_examples_follow_the_daemon_catalog_schema", () => {
	const spec = catalogTool("present");
	expect(spec).toEqual(catalogTool("present", "ultrabrowse"));
	expect(spec.approval).toBe("read");
	expect(spec.deferrable).toBe(false);
	const tail = PRESENT_INSTRUCTIONS.split("present(").slice(1).join("present(");
	// JSON.parse has no raw_decode: take the balanced object that starts the call.
	let depth = 0;
	let end = 0;
	let quoted = false;
	for (let index = 0; index < tail.length; index++) {
		const char = tail[index];
		if (quoted) {
			if (char === "\\") index++;
			else if (char === '"') quoted = false;
		} else if (char === '"') quoted = true;
		else if (char === "{") depth++;
		else if (char === "}" && --depth === 0) {
			end = index + 1;
			break;
		}
	}
	const example = JSON.parse(tail.slice(0, end));
	const schema = spec.parameters;
	expect(validateJsonSchemaValue(schema, example, schema).success).toBe(true);
	expect(validateJsonSchemaValue(schema, draft(), schema).success).toBe(true);
});

for (const role of ["single", "front", "chief", "teammate"] as const) {
	it(`test_catalog_present_returns_only_card_id[${role}]`, async () => {
		const front = role === "front" || role === "chief";
		const reply = "Here is the option I found.";
		installScript([
			{ parts: [call("present", "present-1", draft())] },
			{ parts: [call("probe", "check-id", { value: "{{result:present-1.card_id}}" })] },
			front ? { parts: [call("send_message", "present-reply", { text: reply })] } : textResponse(reply),
		]);
		const team =
			role === "chief" || role === "teammate"
				? { role, agent_id: role, persona: "Careful helper", roster: "Team" }
				: null;
		const spec = catalogTool("present");
		const tools: Message[] = [spec, toolSpec()];
		if (front) {
			const send = toolSpec("send_message");
			send.parameters = {
				type: "object",
				properties: { text: { type: "string" } },
				required: ["text"],
				additionalProperties: false,
			};
			tools.push(send);
		}
		const requests: ModelRequestInfo[] = [];
		const result = { card_id: "present:present-1" };
		await withFakeDaemon(recordingSession(requests), async (daemon, session) => {
			await daemon.open({
				tools,
				agent: {
					orchestration: role === "single" ? "single" : "front",
					front_tools: front ? ["present", "probe", "send_message"] : ["present", "probe"],
					title_needed: false,
				},
				team,
			});
			daemon.user();
			const presented = await daemon.until("tool_call");
			expect(presented.v).toBe(1);
			expect(presented.name).toBe("present");
			expect(presented.args).toEqual(draft());
			daemon.result(presented, { status: "done", for_model: result });
			const checked = await daemon.until("tool_call");
			expect(checked.args).toEqual({ value: result.card_id });
			daemon.result(checked, { status: "done", for_model: "ok" });
			if (front) {
				const final = await daemon.until("tool_call");
				expect(final.name).toBe("send_message");
				expect(final.args).toEqual({ text: reply });
				daemon.result(final, { status: "done", for_model: { sent: true } });
			} else {
				const final = await daemon.until("event", { kind: "assistant.final" });
				expect(final.event.text).toBe(reply);
			}
			await daemon.until("settled");
			if (front) {
				expect(
					(daemon.outputs as Message[]).some(
						item => item.type === "event" && ["assistant.final", "assistant.delta"].includes(item.event.kind),
					),
				).toBe(false);
			}
			const returns = toolResults(snapshotData(daemon).messages, false);
			expect(returns.filter(part => part.toolName === "present").map(part => part.details)).toEqual([result]);
			expect(session.front).toBe(front);
			// Model requests carry tool names only; the schema check reads the agent's tool specs.
			const present = session.agent!.options.tools.find(tool => tool.name === "present");
			expect(present?.parameters as unknown).toEqual(spec.parameters);
		});
		expect(requests.length).toBeGreaterThan(0);
		for (const info of requests) {
			const text = instructions(info);
			expect(text).toContain(PRESENT_INSTRUCTIONS);
			expect(info.tools).toContain("present");
			expect(
				toolResults(info.messages, false)
					.filter(part => part.toolName === "present")
					.every(part => JSON.stringify(part.details) === JSON.stringify(result)),
			).toBe(true);
			if (team) {
				expect(text).toContain(role === "chief" ? CHIEF_INSTRUCTIONS : TEAMMATE_INSTRUCTIONS);
				expect(text).toContain(team.persona);
				expect(JSON.stringify(info.messages)).toContain(team.roster);
				expect(text.includes(FRONT_INSTRUCTIONS)).toBe(role === "chief");
			} else if (role === "front") {
				expect(text).toContain(FRONT_INSTRUCTIONS);
			}
		}
	});
}

it("test_present_instructions_require_catalog_exposure", async () => {
	installScript([textResponse()]);
	const requests: ModelRequestInfo[] = [];
	await withFakeDaemon(recordingSession(requests), async daemon => {
		await daemon.open({ agent: { title_needed: false } });
		daemon.user();
		await daemon.until("settled");
	});
	expect(instructions(requests[0]!)).not.toContain(PRESENT_INSTRUCTIONS);
});

const REJECTIONS: [string, Message][] = [
	["malformed", { status: "error", message: "invalid_card: $: invalid shape" }],
	[
		"privileged",
		{ status: "denied", reason: "card_action_forbidden", hint: "Use the daemon's secure page tools instead" },
	],
];

for (const [mutation, outcome] of REJECTIONS) {
	it(`test_present_draft_rejection_reaches_model_retry[${mutation}]`, async () => {
		const invalid = draft();
		if (mutation === "malformed") {
			invalid.card.blocks[0].rows = [[]];
		} else {
			invalid.card.actions = [{ id: "reject", label: "Reject", style: "secondary", target: { type: "ticket_reject" } }];
		}
		installScript([
			{ parts: [call("present", "bad-draft", invalid)] },
			{ parts: [call("present", "corrected-draft", draft())] },
			textResponse("Here is the corrected comparison."),
		]);
		const requests: ModelRequestInfo[] = [];
		await withFakeDaemon(recordingSession(requests), async daemon => {
			await daemon.open({ tools: [catalogTool("present")] });
			daemon.user();
			const rejected = await daemon.until("tool_call");
			expect(rejected.args).toEqual(invalid);
			daemon.result(rejected, outcome);
			const retried = await daemon.until("tool_call");
			expect(retried.tool_call_id).toBe("corrected-draft");
			expect(retried.args).toEqual(draft());
			const result = { card_id: "present:corrected-draft" };
			daemon.result(retried, { status: "done", for_model: result });
			await daemon.until("settled");
			const messages = snapshotData(daemon).messages;
			const retries = toolResults(messages, true);
			expect(retries).toHaveLength(1);
			expect(retries[0]!.toolCallId).toBe("bad-draft");
			expect(resultText(retries[0]!)).toContain(outcome.reason ?? outcome.message);
			if (outcome.hint !== undefined) expect(resultText(retries[0]!)).toContain(outcome.hint);
			expect(toolResults(messages, false).map(part => part.details)).toEqual([result]);
		});
		expect(toolResults(requests[1]!.messages, true).length).toBeGreaterThan(0);
	});
}

const SECURE: [string, JsonObject, JsonObject][] = [
	["request_login", { tab_id: "tab" }, { status: "submitted", next: "unknown" }],
	["request_otp", { tab_id: "tab" }, { status: "filled" }],
	["request_form", { tab_id: "tab", purpose: "payment" }, { status: "filled", field_count: 4, next: "review_page" }],
];

for (const [name, args, result] of SECURE) {
	it(`test_secure_outcomes_keep_private_values_out_of_model_and_snapshots[${name}]`, async () => {
		const privateValue = "private-page-answer-canary-938247";
		process.env.OPENSIDE_BROKER_TOKEN = privateValue;
		installScript([{ parts: [call(name, "secure-step", args)] }, textResponse("The secure step is complete.")]);
		const requests: ModelRequestInfo[] = [];
		const written: string[] = [];
		const capture = (chunk: unknown) => {
			written.push(typeof chunk === "string" ? chunk : new TextDecoder().decode(chunk as Uint8Array));
			return true;
		};
		const stdout = spyOn(process.stdout, "write").mockImplementation(capture);
		const stderr = spyOn(process.stderr, "write").mockImplementation(capture);
		try {
			await withFakeDaemon(recordingSession(requests), async daemon => {
				await daemon.open({ tools: [catalogTool(name), catalogTool("present")] });
				daemon.user();
				const secure = await daemon.until("tool_call");
				daemon.result(secure, { status: "deferred", ticket_id: "secure", kind: "external" });
				await daemon.until("settled");
				daemon.send("ticket_resolved", {
					ticket_id: "secure",
					tool_call_id: "secure-step",
					resolution: { status: "result", for_model: result },
				});
				await daemon.until("settled");
				const returns = toolResults(snapshotData(daemon).messages, false);
				expect(returns.map(part => part.details)).toEqual([result]);
				expect(JSON.stringify(daemon.outputs)).not.toContain(privateValue);
				expect((daemon.outputs as Message[]).every(item => item.v === 1)).toBe(true);
				expect(daemon.snapshots().every(item => item.format === STATE_FORMAT)).toBe(true);
			});
		} finally {
			stdout.mockRestore();
			stderr.mockRestore();
		}
		expect(JSON.stringify(requests)).not.toContain(privateValue);
		expect(written.join("")).not.toContain(privateValue);
	});
}

it("test_delegated_present_retains_subagent_instructions", async () => {
	installScript(
		[
			{ parts: [call("delegate", "delegate-present", { task: "Compare options" })] },
			textResponse("The comparison is ready."),
		],
		{
			subagents: {
				"delegate-present": [
					{ parts: [call("present", "child-present", draft())] },
					textResponse("Comparison presented."),
				],
			},
		},
	);
	const requests: ModelRequestInfo[] = [];
	await withFakeDaemon(recordingSession(requests), async daemon => {
		await daemon.open({ tools: [delegateSpec(), catalogTool("present")] });
		daemon.user();
		const delegated = await daemon.until("tool_call");
		daemon.result(delegated, {
			status: "done",
			for_model: { subagent_id: "sub-present", model: "test:child-model", tools: ["present"], context: "fresh" },
		});
		const presented = await daemon.until("tool_call");
		expect(presented.name).toBe("present");
		expect(presented.subagent).toBe("sub-present");
		const result = { card_id: "present:child-present" };
		daemon.result(presented, { status: "done", for_model: result });
		await daemon.until("settled");
		const child = snapshotData(daemon).subagents[0]!;
		expect(child.status).toBe("completed");
		expect(toolResults(child.messages, false).map(part => part.details)).toEqual([result]);
	});
	const childRequests = requests.filter(info => info.tools.length === 1 && info.tools[0] === "present");
	expect(childRequests.length).toBeGreaterThan(0);
	for (const info of childRequests) {
		expect(instructions(info)).toContain(SUBAGENT_INSTRUCTIONS);
		expect(instructions(info)).toContain(PRESENT_INSTRUCTIONS);
	}
});
