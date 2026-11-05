import agentPackage from "../../package.json" with { type: "json" };
import type { Versions } from "./types";

// Dependencies are pinned exactly (no ranges) and installed with a frozen bun.lock, so the
// manifest pin is the installed version.
export function sidecarVersions(): Versions {
	return {
		runtime: `bun ${Bun.version}`,
		pi_agent_core: agentPackage.dependencies["@oh-my-pi/pi-agent-core"],
		pi_ai: agentPackage.dependencies["@oh-my-pi/pi-ai"],
		openside_agent: agentPackage.version,
	};
}
