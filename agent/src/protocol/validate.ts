import { validateJsonSchemaValue } from "@oh-my-pi/pi-ai/utils/schema/json-schema-validator";
// The canonical wire schema is hand-maintained inside this package, so an installed sidecar
// carries it without a sibling docs/ tree; the Rust crate validates against the same file, so
// both peers fail on a one-sided shape change.
import schema from "./sidecar-wire.schema.json" with { type: "json" };
import type { Inbound, Outbound, TeamContext } from "./types";

export const WIRE_SCHEMA = schema;

export const TEAM_PERSONA_MAX_CHARS = schema.$defs.TeamContext.properties.persona.maxLength;
export const TEAM_ROSTER_MAX_CHARS = schema.$defs.TeamContext.properties.roster.maxLength;

// The schema caps team text in Unicode characters (code points, as the daemon counts),
// while the validator measures UTF-16 units; those two caps are checked here instead.
const validationSchema: typeof schema = structuredClone(schema);
const teamProperties = validationSchema.$defs.TeamContext.properties;
Reflect.deleteProperty(teamProperties.persona, "maxLength");
Reflect.deleteProperty(teamProperties.roster, "maxLength");

const DAEMON_TO_SIDECAR = { $ref: "#/$defs/DaemonToSidecar" };
const SIDECAR_TO_DAEMON = { $ref: "#/$defs/SidecarToDaemon" };
const TEAM_CONTEXT = { $ref: "#/$defs/TeamContext" };

export type Validated<T> = { ok: true; value: T } | { ok: false; reason: string };

function teamWithinCaps(team: TeamContext): boolean {
	return [...team.persona].length <= TEAM_PERSONA_MAX_CHARS && [...team.roster].length <= TEAM_ROSTER_MAX_CHARS;
}

// Absent optional collections and the author default are part of the wire contract
// (the schema lists them as non-required with defaults); fill them so consumers never
// branch on absence.
function fillDefaults(message: Inbound): void {
	if (message.type === "user_message") {
		message.author_kind ??= "person";
		message.parts ??= [];
	} else if (message.type === "tool_result" && message.outcome.status === "done") {
		message.outcome.parts ??= [];
	}
}

/** Validate one parsed daemon → sidecar message; the reason never echoes peer values. */
export function validateInbound(value: unknown): Validated<Inbound> {
	if (typeof value !== "object" || value === null || Array.isArray(value)) {
		return { ok: false, reason: "Protocol envelope must be an object" };
	}
	if (!("v" in value) || value.v !== 1) return { ok: false, reason: "Protocol version 1 is required" };
	if (!validateJsonSchemaValue(DAEMON_TO_SIDECAR, value, validationSchema).success) {
		return { ok: false, reason: "Message does not match the wire schema" };
	}
	const message = value as Inbound;
	if (message.type === "open" && message.team != null && !teamWithinCaps(message.team)) {
		return { ok: false, reason: "Team context exceeds its character caps" };
	}
	fillDefaults(message);
	return { ok: true, value: message };
}

/** Validate a TeamContext alone (role enum, required fields, code-point caps). */
export function validateTeamContext(value: unknown): Validated<TeamContext> {
	if (!validateJsonSchemaValue(TEAM_CONTEXT, value, validationSchema).success) {
		return { ok: false, reason: "Team context does not match the wire schema" };
	}
	const team = value as TeamContext;
	if (!teamWithinCaps(team)) return { ok: false, reason: "Team context exceeds its character caps" };
	return { ok: true, value: team };
}

/** Validate a sidecar → daemon message (tests and the fake daemon); issues name schema paths. */
export function validateOutbound(value: unknown): Validated<Outbound> {
	const result = validateJsonSchemaValue(SIDECAR_TO_DAEMON, value, validationSchema);
	if (!result.success) {
		const issues = result.issues.map(issue => `${issue.path.join(".")}: ${issue.message}`).join("; ");
		return { ok: false, reason: issues };
	}
	return { ok: true, value: value as Outbound };
}
