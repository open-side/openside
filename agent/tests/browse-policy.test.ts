// Worker escalation policy and the browser profiles' language/continuation rules.
// The takeover-continuation run (deferred human step, fresh snapshot before replying)
// is a session behaviour and is covered by the runtime tests.

import { describe, expect, it } from "bun:test";
import { SUBAGENT_INSTRUCTIONS } from "../src/features/browse-policy";

const SNAPSHOTS = `${import.meta.dir}/snapshots`;
const PROFILES = `${import.meta.dir}/../../docs/specs/profiles`;

describe("browse policy", () => {
	it("test_task_language_continuation_and_bulk_scope_policy_snapshot", async () => {
		expect(SUBAGENT_INSTRUCTIONS).toBe(await Bun.file(`${SNAPSHOTS}/task-instructions.txt`).text());
	});

	it("test_task_counts_scopes_and_candidate_facts_are_explicit", () => {
		const policy = SUBAGENT_INSTRUCTIONS.split(/\s+/).filter(Boolean).join(" ");
		expect(policy).toContain("restate the exact selected scope to the person");
		expect(policy).toContain("name every included and excluded folder, category, or target");
		expect(policy).toContain('A generic "go ahead" does not choose among unresolved alternatives');
		expect(policy).toContain("Compute counts from observed rows or explicit tool totals, never estimate");
		expect(policy).toContain("sum the group sizes and name which groups contribute to each total");
		expect(policy).toContain("status candidate is only a discovered sign-in");
		expect(policy).toContain("authentication is not verified");
		expect(policy).toContain("permission for this agent is a separate grant");
		expect(policy).toContain("Candidate status alone is not a failed connection");
		expect(policy).toContain("does not justify asking the person to reconnect");
	});

	it.each(["browser-agent", "ultrabrowse"])(
		"test_profile_snapshots_keep_language_and_human_step_continuation %p",
		async profile => {
			const open = (await Bun.file(`${PROFILES}/${profile}.open.json`).json()) as { system_prompt_parts: string[] };
			const prompt = open.system_prompt_parts.join("\n");
			expect(prompt).toContain("person's latest explicit language preference");
			expect(prompt).toContain("page/tool language never overrides it");
			expect(prompt).toContain("Tool-adjacent narration is internal, not a reply");
			expect(prompt).toContain("start the continuation with a fresh browser_snapshot");
			expect(prompt).toContain("before replying");
			expect(prompt).toContain("Never reuse earlier snapshot ids or refs");
		},
	);
});
