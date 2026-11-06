// The daemon's context meter tells the chat's own agent apart from subagents and title
// naming by whether `context_window` is present; 0 means the model's window is unknown.
import { describe, expect, it } from "bun:test";
import { usageEvent } from "../src/runtime/events";

const usage = { input: 10, output: 2, cacheRead: 3, cacheWrite: 1 } as Parameters<typeof usageEvent>[1];

describe("usage context window contract", () => {
	it("sends the window for the own agent, 0 when it is unknown", () => {
		expect(usageEvent("m", usage, undefined, 200_000).context_window).toBe(200_000);
		expect(usageEvent("m", usage, undefined, 0).context_window).toBe(0);
		expect(usageEvent("m", usage, undefined, Number.NaN).context_window).toBe(0);
	});

	it("omits it when no window is passed (subagents, title naming)", () => {
		const event = usageEvent("m", usage);
		expect("context_window" in event).toBe(false);
		expect(event.input_tokens).toBe(14);
	});
});
