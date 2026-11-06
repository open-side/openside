import { describe, expect, it } from "bun:test";
import type { StreamFn } from "@oh-my-pi/pi-agent-core";
import type { Context, Model } from "@oh-my-pi/pi-ai";
import { hasFreshImage, routeVisual } from "../../src/providers/visual";

const image = { type: "image" as const, data: "AAAA", mimeType: "image/png" };
const user = (content: Context["messages"][number]["content"]) =>
	({ role: "user", content, timestamp: 0 }) as Context["messages"][number];
const assistant = { role: "assistant", content: [], timestamp: 0 } as unknown as Context["messages"][number];

describe("visual role routing", () => {
	it("test_only_input_since_the_last_assistant_turn_counts", () => {
		expect(hasFreshImage({ messages: [user([image])] })).toBe(true);
		expect(hasFreshImage({ messages: [user([image]), assistant, user("thanks")] })).toBe(false);
		expect(hasFreshImage({ messages: [user("hi")] })).toBe(false);
		const screenshot = {
			role: "toolResult",
			toolCallId: "t",
			toolName: "browser_screenshot",
			content: [image],
			isError: false,
			timestamp: 0,
		};
		expect(hasFreshImage({ messages: [user("look"), assistant, screenshot as Context["messages"][number]] })).toBe(
			true,
		);
	});

	it("test_image_requests_use_the_visual_model_and_others_the_primary", () => {
		const calls: string[] = [];
		const fake = (name: string): StreamFn =>
			((model: Model) => {
				calls.push(`${name}:${model.id}`);
				return undefined as never;
			}) as unknown as StreamFn;
		const visualModel = { id: "visual-model" } as Model;
		const primaryModel = { id: "chat-model" } as Model;
		const routed = routeVisual(fake("primary"), fake("visual"), visualModel);
		routed(primaryModel, { messages: [user("hi")] });
		routed(primaryModel, { messages: [user([image])] });
		expect(calls).toEqual(["primary:chat-model", "visual:visual-model"]);
	});
});
