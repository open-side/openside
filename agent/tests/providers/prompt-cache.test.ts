import { afterEach, describe, expect, it } from "bun:test";
import type { Context, Tool } from "@oh-my-pi/pi-ai";
import { resolveModel, setProviderBaseUrlForTests } from "../../src/providers/models";
import { makeStreamFn } from "../../src/providers/stream";
import { FakeCredentialDaemon, type ProviderServer, serveProvider } from "./fake-credential-daemon";

let server: ProviderServer | undefined;

afterEach(() => {
	server?.stop();
	server = undefined;
});

function sse(events: { event?: string; data: unknown }[]): Response {
	const body = events
		.map(({ event, data }) => `${event ? `event: ${event}\n` : ""}data: ${JSON.stringify(data)}\n\n`)
		.join("");
	return new Response(body, {
		headers: { "content-type": "text/event-stream" },
	});
}

function anthropicReply(): Response {
	return sse([
		{
			event: "message_start",
			data: {
				type: "message_start",
				message: {
					id: "msg_test",
					type: "message",
					role: "assistant",
					model: "claude-sonnet-5-5",
					content: [],
					stop_reason: null,
					stop_sequence: null,
					usage: {
						input_tokens: 100,
						output_tokens: 1,
						cache_read_input_tokens: 50,
						cache_creation_input_tokens: 0,
					},
				},
			},
		},
		{
			event: "content_block_start",
			data: {
				type: "content_block_start",
				index: 0,
				content_block: { type: "text", text: "" },
			},
		},
		{
			event: "content_block_delta",
			data: {
				type: "content_block_delta",
				index: 0,
				delta: { type: "text_delta", text: "ok" },
			},
		},
		{
			event: "content_block_stop",
			data: { type: "content_block_stop", index: 0 },
		},
		{
			event: "message_delta",
			data: {
				type: "message_delta",
				delta: { stop_reason: "end_turn", stop_sequence: null },
				usage: { output_tokens: 1 },
			},
		},
		{ event: "message_stop", data: { type: "message_stop" } },
	]);
}

function openaiReply(): Response {
	const message = {
		id: "msg_test",
		type: "message",
		status: "completed",
		role: "assistant",
		content: [{ type: "output_text", text: "ok", annotations: [] }],
	};
	const response = {
		id: "resp_test",
		object: "response",
		created_at: 1,
		status: "completed",
		model: "gpt-5.6",
		output: [message],
		usage: {
			input_tokens: 100,
			output_tokens: 1,
			total_tokens: 101,
			input_tokens_details: { cached_tokens: 50 },
		},
	};
	return sse([
		{
			data: {
				type: "response.created",
				response: { ...response, status: "in_progress", output: [] },
			},
		},
		{
			data: {
				type: "response.output_item.added",
				output_index: 0,
				item: { ...message, status: "in_progress", content: [] },
			},
		},
		{
			data: {
				type: "response.content_part.added",
				output_index: 0,
				item_id: "msg_test",
				content_index: 0,
				part: { type: "output_text", text: "", annotations: [] },
			},
		},
		{
			data: {
				type: "response.output_text.delta",
				output_index: 0,
				item_id: "msg_test",
				content_index: 0,
				delta: "ok",
			},
		},
		{
			data: {
				type: "response.output_item.done",
				output_index: 0,
				item: message,
			},
		},
		{ data: { type: "response.completed", response } },
	]);
}

const probe: Tool = {
	name: "probe",
	description: "Probe tool",
	parameters: {
		type: "object",
		properties: {},
	} as unknown as Tool["parameters"],
};

describe("prompt cache", () => {
	for (const provider of ["anthropic", "openai"] as const) {
		it(`test_provider_requests_enable_stable_prefix_cache[${provider}]`, async () => {
			server = serveProvider(() => (provider === "anthropic" ? anthropicReply() : openaiReply()));
			setProviderBaseUrlForTests(provider, provider === "anthropic" ? server.url : `${server.url}/v1`);
			const id = provider === "anthropic" ? "anthropic:claude-sonnet-5-5" : "openai:gpt-5.6";
			const stream = makeStreamFn(new FakeCredentialDaemon(1), {
				provider,
				model: id,
				networkAllow: [],
			});
			const context: Context = {
				systemPrompt: ["stable instructions"],
				messages: [{ role: "user", content: "same prompt", timestamp: 1 }],
				tools: [probe],
			};
			for (let turn = 0; turn < 2; turn++) {
				const result = await (
					await stream(resolveModel(id), context, {
						maxTokens: 16,
						sessionId: "stable-session",
					})
				).result();
				expect(result.stopReason).toBe("stop");
				expect(result.usage.cacheRead).toBe(50);
			}
			const [first, second] = server.seen.map(request => JSON.parse(request.body) as Record<string, unknown>);
			expect(first).toEqual(second as Record<string, unknown>);
			if (provider === "anthropic") {
				const system = first?.system as { cache_control?: { type: string } }[];
				const tools = first?.tools as { cache_control?: { type: string } }[];
				expect(system.at(-1)?.cache_control?.type).toBe("ephemeral");
				expect(tools.at(-1)?.cache_control?.type).toBe("ephemeral");
			} else {
				expect(first?.prompt_cache_key).toBeString();
			}
		});
	}
});
