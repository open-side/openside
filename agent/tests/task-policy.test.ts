import { beforeEach, describe, expect, it } from "bun:test";
import { createMockModel } from "@oh-my-pi/pi-ai/providers/mock";
import { SUBAGENT_INSTRUCTIONS } from "../src/features/browse-policy";
import { Session } from "../src/runtime/session";
import type { JsonlConnection } from "../src/transport/jsonl";
import type { RpcRouter } from "../src/transport/rpc";
import { hermeticEnv, toolSpec, withFakeDaemon } from "./helpers/fake-daemon";

function session(conn: JsonlConnection, rpc: RpcRouter) {
	const started = new Session(conn, rpc, { model: createMockModel({ handler: { content: ["ok"] } }) });
	return { session: started, done: started.serve() };
}

function instructions(live: Session): string {
	return (live.agent as NonNullable<Session["agent"]>).options.systemPrompt.join("\n\n");
}

beforeEach(() => hermeticEnv());

describe("task policy", () => {
	it("test_worker_policy_is_installed_without_inventing_tools", async () => {
		await withFakeDaemon(session, async (daemon, live) => {
			await daemon.open({ tools: [toolSpec()] });
			expect(instructions(live)).toContain(SUBAGENT_INSTRUCTIONS);
			expect([...live.expectedTools]).toEqual(["probe"]);
		});
	});

	it("test_front_does_not_receive_worker_escalation_policy", async () => {
		await withFakeDaemon(session, async (daemon, live) => {
			await daemon.open({
				tools: [toolSpec("send_message")],
				agent: { orchestration: "front", front_tools: ["send_message"] },
			});
			expect(instructions(live)).not.toContain(SUBAGENT_INSTRUCTIONS);
		});
	});

	it("test_escalation_ladder_distinguishes_absence_from_failure", () => {
		const policy = SUBAGENT_INSTRUCTIONS;
		expect(policy.indexOf("1. Use information")).toBeLessThan(policy.indexOf("2. Use an available"));
		expect(policy.indexOf("2. Use an available")).toBeLessThan(policy.indexOf("3. Use web search"));
		expect(policy.indexOf("3. Use web search")).toBeLessThan(policy.indexOf("4. Use the authorized"));
		expect(policy.indexOf("1. Use information")).toBeGreaterThanOrEqual(0);
		for (const phrase of [
			"only if a search capability is actually exposed",
			"absence permits the next available",
			"expired authentication, or access denial means stop",
			"Never bypass it with the browser",
			"structure before screenshots",
			"security challenges require the\nperson",
		]) {
			expect(policy).toContain(phrase);
		}
	});
});
