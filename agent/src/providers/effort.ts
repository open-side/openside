/**
 * Effort resolution shared by the request path (`stream.ts`) and the model catalog, so the
 * level the composer shows for an unpicked effort is the level requests are sent with.
 */
import type { Api, Model } from "@oh-my-pi/pi-ai";
import { clampThinkingLevelForModel, type Effort, getSupportedEfforts } from "@oh-my-pi/pi-catalog";
import { EffortUnsupported } from "./errors";

/** What the front agent asks for when the chat has no effort of its own. */
export const FRONT_EFFORT = "minimal" as Effort;

/**
 * The effort a candidate is asked for. Runtime defaults are provider-neutral (the front
 * asks for `minimal`) and pi-ai throws before any request when the candidate's ladder
 * lacks the tier (Claude has no `minimal`), so they move to the nearest served tier.
 */
export function servedEffort(
	candidate: { id: string; model: Model<Api> },
	requested: Effort | undefined,
	fixed: boolean,
): Effort | undefined {
	if (requested === undefined) return undefined;
	const supported = getSupportedEfforts(candidate.model);
	if (supported.includes(requested)) return requested;
	if (fixed) throw new EffortUnsupported(requested, candidate.id, supported);
	return clampThinkingLevelForModel(candidate.model, requested);
}
