import { beforeEach, expect, it } from "bun:test";
import { validateTeamContext } from "../src/protocol/validate.ts";
import { Session, type SessionOptions } from "../src/runtime/session.ts";
import { CHIEF_TOOLS, FRONT_INSTRUCTIONS, TEAMMATE_TOOLS } from "../src/runtime/team.ts";
import { hermeticEnv, installScript, textResponse, toolSpec, withFakeDaemon } from "./helpers/fake-daemon.ts";

beforeEach(() => {
	hermeticEnv();
});

for (const [role, roleTools] of [
	["chief", CHIEF_TOOLS],
	["teammate", TEAMMATE_TOOLS],
] as const) {
	it(`test_team_role_prompt_and_tool_visibility[${role}]`, async () => {
		const names = new Set(roleTools);
		if (role === "chief") names.add("send_message");
		const terminal =
			role === "chief"
				? { parts: [{ type: "tool_call", name: "stay_silent", args: {}, id: "silent" }] }
				: textResponse();
		installScript([terminal]);
		const persona = `Persona: ${"🦉".repeat(1500 - "Persona: ".length)}`;
		const roster = `Roster: ${"界".repeat(2000 - "Roster: ".length)}`;
		const observed: Parameters<NonNullable<SessionOptions["onModelRequest"]>>[0][] = [];
		const tools = [...names].sort().map(toolSpec);
		for (const spec of tools) {
			if (spec.name === "send_message") {
				spec.parameters = {
					type: "object",
					properties: { text: { type: "string" } },
					required: ["text"],
					additionalProperties: false,
				};
			}
		}
		const functionNames = new Set(names);
		if (role === "chief") functionNames.delete("send_message");
		await withFakeDaemon(
			(conn, rpc) => {
				const session = new Session(conn, rpc, { onModelRequest: request => observed.push(request) });
				return { session, done: session.serve() };
			},
			async (daemon, session) => {
				const ready = (await daemon.open({
					tools,
					team: { role, agent_id: role, persona, roster },
					agent: { orchestration: "front", front_tools: [...names].sort(), title_needed: false },
				})) as { tools: string[] };
				expect(new Set(ready.tools)).toEqual(names);
				expect(new Set(session.expectedTools)).toEqual(functionNames);
				await daemon.user();
				await daemon.until("settled");
			},
		);
		expect(observed).toHaveLength(1);
		const request = observed[0]!;
		expect(new Set(request.tools)).toEqual(functionNames);
		if (role === "chief") expect(new Set(request.outputTools)).toEqual(new Set(["send_message", "stay_silent"]));
		const instructions = request.systemPrompt.join("\n\n");
		expect(instructions).toContain(persona);
		expect(JSON.stringify(request.messages)).toContain(roster);
		expect([...persona]).toHaveLength(1500);
		expect([...roster]).toHaveLength(2000);
		expect(instructions).toContain(`[${role.toUpperCase()}]`);
		if (role === "chief") expect(instructions).toContain(FRONT_INSTRUCTIONS);
		else expect(instructions).not.toContain("You are the front agent");
	});
}

for (const [role, wrong] of [
	["chief", "team_report"],
	["teammate", "team_hire"],
	[null, "team_status"],
] as const) {
	it(`test_team_wrong_role_tools_fail_closed[${role}-${wrong}]`, async () => {
		await withFakeDaemon(
			(conn, rpc) => {
				const session = new Session(conn, rpc);
				return { session, done: session.serve() };
			},
			async daemon => {
				await daemon.send("open", {
					session: "test",
					model: { id: "test:model", fallback: [] },
					tools: [toolSpec(wrong)],
					system_prompt_parts: [],
					config: { credentials: {}, agent: {} },
					...(role === null ? {} : { team: { role, agent_id: role, persona: "", roster: "" } }),
				});
				const error = (await daemon.until("error")) as { code: string; fatal: boolean };
				expect(error.code).toBe("tool_gate");
				expect(error.fatal).toBe(true);
			},
		);
	});
}

for (const [field, limit] of [
	["persona", 1500],
	["roster", 2000],
] as const) {
	it(`test_team_wire_character_caps[${field}-${limit}]`, () => {
		const data = { role: "chief", agent_id: "chief", persona: "", roster: "" };
		data[field] = "🦉".repeat(limit);
		const accepted = validateTeamContext(data);
		expect(accepted.ok).toBe(true);
		if (accepted.ok) expect([...accepted.value[field]]).toHaveLength(limit);
		data[field] += "x";
		expect(validateTeamContext(data).ok).toBe(false);
	});
}

it("test_team_wire_rejects_unknown_role", () => {
	expect(validateTeamContext({ role: "admin", agent_id: "chief", persona: "", roster: "" }).ok).toBe(false);
});
