import { beforeEach, describe, expect, it } from "bun:test";
import { type Complete, type CompleteRequest, completeText, Session } from "../src/runtime/session";
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

const isTitle = (request: CompleteRequest) => request.instructions.includes("conversation title");

function start(complete?: Complete) {
	return (conn: JsonlConnection, rpc: RpcRouter) => {
		const session = new Session(conn, rpc, complete === undefined ? {} : { complete });
		return { session, done: session.serve() };
	};
}

function titles(daemon: FakeDaemon): string[] {
	return daemon.outputs
		.filter(item => item.type === "event" && item.event.kind === "session.title_proposed")
		.map(item => item.event.title);
}

function lastState(daemon: FakeDaemon): Wire {
	return JSON.parse(daemon.snapshots().at(-1)?.data);
}

beforeEach(() => hermeticEnv());

describe("titles", () => {
	it("test_title_uses_a_separate_scripted_model_and_is_not_requested_after_resume", async () => {
		installScript([textResponse("First reply"), textResponse("Second reply")], { title: "Compare Acme plans" });
		let snapshot: Wire | undefined;
		await withFakeDaemon(start(), async (daemon, session) => {
			await daemon.open();
			daemon.user({ text: "https://shop.example/plans?private=token" });
			await daemon.until("settled");
			await session.titleTask?.promise;
			if (titles(daemon).length === 0) await daemon.until("event", { kind: "session.title_proposed" });
			expect(titles(daemon)).toEqual(["Compare Acme plans"]);
			snapshot = daemon.snapshots().at(-1);
			const saved = lastState(daemon);
			expect(saved.title_attempted).toBe(true);
			expect(saved.script_cursor).toBe(1);
		});
		await withFakeDaemon(start(), async daemon => {
			await daemon.open({ resume_state: snapshot });
			daemon.user({ message_id: "second", text: "Keep my name" });
			await daemon.until("settled");
			expect(titles(daemon)).toEqual([]);
			const finals = daemon.outputs
				.filter(item => item.type === "event" && item.event.kind === "assistant.final")
				.map(item => item.event.text);
			expect(finals).toEqual(["Second reply"]);
		});
	});

	it("test_naming_failure_does_not_fail_a_completed_reply", async () => {
		installScript([textResponse("Done")]);
		const complete: Complete = async request => {
			if (isTitle(request)) throw new Error("private provider diagnostic");
			return completeText(request);
		};
		await withFakeDaemon(start(complete), async (daemon, session) => {
			await daemon.open();
			daemon.user();
			await daemon.until("settled");
			await session.titleTask?.promise;
			const results = daemon.outputs.filter(item => item.type === "message_result");
			expect(results[0]?.outcome).toBe("completed");
			expect(daemon.outputs.some(item => item.type === "error")).toBe(false);
			expect(JSON.stringify(daemon.outputs)).not.toContain("private provider diagnostic");
			expect(lastState(daemon).title_attempted).toBe(true);
		});
	});

	it("test_slow_naming_does_not_delay_settle_and_is_cancelled_on_shutdown", async () => {
		installScript([textResponse("Done")]);
		const entered = Promise.withResolvers<void>();
		let cancelled = false;
		const complete: Complete = async request => {
			if (!isTitle(request)) return completeText(request);
			entered.resolve();
			const aborted = Promise.withResolvers<never>();
			request.signal?.addEventListener("abort", () => {
				cancelled = true;
				aborted.reject(request.signal?.reason);
			});
			return aborted.promise;
		};
		await withFakeDaemon(start(complete), async (daemon, session) => {
			await daemon.open();
			daemon.user();
			await entered.promise;
			await daemon.until("settled", { timeoutMs: 1000 });
			expect(session.titleTask).toBeDefined();
			expect(cancelled).toBe(false);
		});
		expect(cancelled).toBe(true);
	});

	it("test_named_conversations_do_not_spend_a_naming_request", async () => {
		installScript([textResponse("Done")]);
		await withFakeDaemon(start(), async (daemon, session) => {
			await daemon.open({ agent: { title_needed: false } });
			daemon.user();
			await daemon.until("settled");
			expect(session.titleTask).toBeUndefined();
			expect(titles(daemon)).toEqual([]);
			expect(lastState(daemon).title_attempted).toBe(true);
		});
	});

	it("test_naming_keeps_the_goal_from_a_request_with_media", async () => {
		installScript([textResponse("I checked the chart")]);
		const seen: string[] = [];
		const complete: Complete = async request => {
			if (isTitle(request)) seen.push(request.payload);
			return completeText(request);
		};
		await withFakeDaemon(start(complete), async (daemon, session) => {
			await daemon.open();
			daemon.send("user_message", {
				message_id: "with-chart",
				text: "Compare the plans in this chart",
				mode: "now",
				parts: [
					{
						type: "image",
						mime: "image/png",
						name: "chart.png",
						data_base64:
							"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
					},
				],
			});
			await daemon.until("settled");
			await session.titleTask?.promise;
			expect(seen.join("\n")).toContain("Compare the plans in this chart");
		});
	});
});
