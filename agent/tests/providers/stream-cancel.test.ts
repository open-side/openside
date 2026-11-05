import { afterAll, describe, expect, it } from "bun:test";
import type { Api, AssistantMessageEvent, Model } from "@oh-my-pi/pi-ai";
import { registerCustomApi, unregisterCustomApis } from "@oh-my-pi/pi-ai";
import { AssistantMessageEventStream } from "@oh-my-pi/pi-ai/utils/event-stream";
import { resolveModel } from "../../src/providers/models";
import { makeStreamFn } from "../../src/providers/stream";
import { FakeCredentialDaemon, userContext } from "./fake-credential-daemon";

const SOURCE = "openside-tests/stream-cancel";

/** A provider stream whose iterator rejects once the request is aborted, after `visible` events. */
class RejectingStream extends AssistantMessageEventStream {
	readonly #events: AssistantMessageEvent[];
	readonly #signal: AbortSignal | undefined;

	constructor(events: AssistantMessageEvent[], signal: AbortSignal | undefined) {
		super();
		this.#events = events;
		this.#signal = signal;
	}

	override async *[Symbol.asyncIterator](): AsyncGenerator<AssistantMessageEvent> {
		for (const event of this.#events) yield event;
		const { promise, reject } = Promise.withResolvers<never>();
		const signal = this.#signal;
		if (signal?.aborted) reject(new Error("aborted by caller"));
		signal?.addEventListener("abort", () => reject(new Error("aborted by caller")), { once: true });
		await promise;
	}
}

function register(api: string, visible: boolean, entered: () => void): void {
	registerCustomApi(
		api,
		(model, _context, options) => {
			const partial = {
				role: "assistant" as const,
				content: [{ type: "text" as const, text: "par" }],
				api: model.api,
				provider: model.provider,
				model: model.id,
				usage: {
					input: 0,
					output: 0,
					cacheRead: 0,
					cacheWrite: 0,
					totalTokens: 0,
					cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
				},
				stopReason: "stop" as const,
				timestamp: 1,
			};
			const events: AssistantMessageEvent[] = [{ type: "start", partial }];
			if (visible) {
				events.push({ type: "text_start", contentIndex: 0, partial });
				events.push({ type: "text_delta", contentIndex: 0, delta: "par", partial });
			}
			entered();
			return new RejectingStream(events, options?.signal);
		},
		SOURCE,
	);
}

afterAll(() => unregisterCustomApis(SOURCE));

describe("stream cancellation", () => {
	for (const visible of [false, true]) {
		it(`test_iterator_rejection_on_abort_reports_cancelled_without_waiting[${visible ? "visible" : "pre_visible"}]`, async () => {
			const api = `openside-test-rejecting-${visible ? "visible" : "hidden"}`;
			const { promise: entered, resolve } = Promise.withResolvers<void>();
			register(api, visible, resolve);
			const daemon = new FakeCredentialDaemon();
			const model: Model<Api> = { ...resolveModel("openai-chat:gpt-5"), api: api as Api };
			const stream = makeStreamFn(daemon, { provider: "openai-chat", model: "openai-chat:gpt-5", networkAllow: [] });
			const controller = new AbortController();
			const pending = Promise.resolve(stream(model, userContext("first"), { signal: controller.signal }));
			await entered;
			if (visible) {
				// The visible stream is returned before the abort, so the pump owns the rejection.
				const outer = await pending;
				controller.abort();
				const result = await outer.result();
				expect(result.stopReason).toBe("aborted");
			} else {
				controller.abort();
				const result = await (await pending).result();
				expect(result.stopReason).toBe("aborted");
			}
			expect(daemon.reports).toHaveLength(1);
			expect(daemon.reports[0]?.outcome.outcome).toBe("cancelled");
			expect(daemon.reports[0]?.await).toBe(false);
			expect(daemon.reports[0]?.streamStarted).toBe(visible);
			expect(daemon.requests).toHaveLength(1);
		});
	}
});
