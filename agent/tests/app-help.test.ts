import { describe, expect, it } from "bun:test";
import { join } from "node:path";
import { SUBAGENT_INSTRUCTIONS } from "../src/features/browse-policy";
import { FRONT_APP_HELP, FRONT_INSTRUCTIONS } from "../src/runtime/team";

const ROOT = join(import.meta.dir, "../..");

describe("front app help", () => {
	it("test_front_help_exactly_matches_checked_ui_feature_map", async () => {
		const help = await Bun.file(join(ROOT, "docs/specs/app-help.md")).text();
		expect(FRONT_APP_HELP).toBe(help);
		expect(Buffer.byteLength(help)).toBeLessThanOrEqual(1500);
		expect(FRONT_INSTRUCTIONS).toContain(FRONT_APP_HELP);
		expect(SUBAGENT_INSTRUCTIONS).not.toContain(FRONT_APP_HELP);
		expect(await Bun.file(join(ROOT, "crates/opensided/src/prompt.rs")).text()).not.toContain("app-help.md");
	});

	it("test_front_describes_broken_output_and_candidate_signins_without_guessing", () => {
		const policy = FRONT_INSTRUCTIONS.split(/\s+/).filter(Boolean).join(" ");
		for (const phrase of [
			"use the static openside app help",
			"Never invent a menu or control",
			"the chief's home and teammates' home conversations offer neither Delete conversation nor Clear",
			"Archive is reversible and is not deletion",
			"acknowledge that it is broken without speculating",
			"Markdown document attachment or a native result card",
			"only claim delivery after the tool succeeds",
			"discovered sign-in suggested by cookies or history, not verified authentication",
			"Active status and authorization for this agent are separate facts",
			"Candidate status alone does not mean failure",
			"not a reason to ask the person to reconnect",
		]) {
			expect(policy).toContain(phrase);
		}
	});
});
