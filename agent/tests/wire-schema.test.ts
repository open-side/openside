// OP-2: what this sidecar sends validates against the hand-maintained wire schema, and the
// package version sits inside the range the daemon accepts.
import { describe, expect, it } from "bun:test";
import agentPackage from "../package.json" with { type: "json" };
import { type OutboundBody, type OutboundType, STATE_FORMAT } from "../src/protocol/types";
import { validateInbound, validateOutbound, WIRE_SCHEMA } from "../src/protocol/validate";
import { sidecarVersions } from "../src/protocol/versions";

const deferred = { tool_call_id: "call-1", ticket_id: "ticket-1", kind: "approval" } as const;

// One or more samples per outbound type; the `satisfies` keeps them in step with the TS types.
const OUTBOUND = [
	{ type: "hello", protocol: 1, nonce: "0123456789abcdef", pid: 42, versions: sidecarVersions() },
	{ type: "ready", tools: ["read"], pending_deferred: [{ ...deferred, kind: "external" }], startup_ms: 7 },
	{ type: "event", event: { kind: "turn.started" } },
	{ type: "event", event: { kind: "turn.ended" } },
	{ type: "event", event: { kind: "assistant.delta", text: "he" } },
	{ type: "event", event: { kind: "assistant.final", text: "hello" } },
	{ type: "event", event: { kind: "session.title_proposed", title: "Plan" } },
	{ type: "event", event: { kind: "thinking.delta", text: "hm" } },
	{
		type: "event",
		event: { kind: "subagent.lifecycle", subagent_id: "sub-1", status: "running", known_subagents: ["sub-1"] },
	},
	{
		type: "event",
		event: { kind: "subagent.lifecycle", subagent_id: "sub-1", status: "failed", error_code: "x", diagnostic: "d" },
	},
	{ type: "event", event: { kind: "subagent.progress", subagent_id: "sub-1", progress: { kind: "model_request" } } },
	{ type: "event", event: { kind: "usage", model: "script", input_tokens: 1, output_tokens: 2 } },
	{ type: "message_accepted", message_id: "message-1" },
	{ type: "message_result", message_id: "message-1", outcome: "failed", reason: "model_error" },
	{ type: "message_result", message_id: "message-1", outcome: "completed" },
	{
		type: "tool_call",
		rpc_id: "rpc-1",
		tool_call_id: "call-1",
		name: "read",
		args: { path: "a" },
		subagent: "sub-1",
		approval_ticket: "t",
	},
	{ type: "tool_call", rpc_id: "rpc-2", tool_call_id: "call-2", name: "read", args: {} },
	{
		type: "credential_request",
		id: "c1",
		request_id: "r1",
		provider: "anthropic",
		model: "claude",
		attempt: 63,
		subagent: "sub-1",
	},
	{
		type: "credential_report",
		id: "c2",
		request_id: "r1",
		lease_id: "l1",
		outcome: "rate_limit",
		stream_started: false,
		retry_after_ms: 1000,
		scope: "account",
		usage: { input_tokens: 1, output_tokens: 2, cache_read_tokens: 0, cache_write_tokens: 0 },
		usage_windows: [{ name: "5h", used_percent: 12.5, reset_at_ms: 1 }],
	},
	{ type: "deferred", calls: [deferred] },
	{ type: "state_snapshot", state: { format: STATE_FORMAT, data: "opaque" } },
	{ type: "settled" },
	{ type: "context_compacted" },
	{ type: "context_compacted", subagent: "sub-1" },
	{ type: "error", code: "tool_gate", message: "Exposed tools differ from daemon tools", fatal: true },
	{ type: "error", code: "bad_request", message: "Message text exceeds 1 MiB", fatal: false, message_id: "m" },
	{ type: "subagent_steer_result", control_id: "steer-1", delivered: true },
	{ type: "bye" },
] satisfies OutboundBody[];

const defs = WIRE_SCHEMA.$defs;

describe("wire schema", () => {
	it("every outbound sample validates against the committed schema", () => {
		for (const sample of OUTBOUND) {
			const result = validateOutbound({ v: 1, ...sample });
			expect(result.ok ? "ok" : `${sample.type}: ${result.reason}`).toBe("ok");
		}
	});

	it("test_schema_covers_every_message_type_in_both_directions", () => {
		const outbound = new Set(Object.keys(defs.SidecarToDaemon.discriminator.mapping));
		expect(new Set<OutboundType>(OUTBOUND.map(sample => sample.type))).toEqual(outbound as Set<OutboundType>);
		const inbound = Object.keys(defs.DaemonToSidecar.discriminator.mapping);
		for (const type of [
			"open",
			"user_message",
			"tool_result",
			"credential_response",
			"credential_error",
			"credential_reported",
			"ticket_resolved",
			"shutdown",
		]) {
			expect(inbound).toContain(type);
		}
		for (const name of ["Hello", "Open", "Event"] as const) {
			expect(defs[name].required).toEqual(expect.arrayContaining(["v", "type"]));
		}
	});

	it("rejects shapes outside the contract", () => {
		for (const bad of [
			{ v: 2, type: "settled" },
			{ type: "settled" },
			{
				v: 1,
				type: "hello",
				protocol: 1,
				nonce: "n",
				pid: 1,
				versions: { python: "", openside_agent: "" },
			},
			{ v: 1, type: "ready", tools: [], pending_deferred: [], startup_ms: 1, extra: 1 },
			{ v: 1, type: "unknown" },
		]) {
			expect(validateOutbound(bad).ok).toBe(false);
		}
	});

	it("accepts legacy and current snapshot formats on resume", () => {
		for (const format of ["openside-agent-state/1", STATE_FORMAT]) {
			const open = {
				v: 1,
				type: "open",
				session: "s",
				model: { id: "script", fallback: [] },
				system_prompt_parts: [],
				tools: [],
				resume_state: { format, data: "x" },
				config: { credentials: {}, agent: {} },
			};
			expect(validateInbound(open).ok).toBe(true);
		}
	});

	it("test_credential_request_metadata_is_bounded", () => {
		const response = (request_metadata: unknown) => ({
			v: 1,
			type: "credential_response",
			id: "credential-rpc",
			request_id: "model-request",
			lease_id: "lease-1",
			generation: 1,
			account: { source: "vault", account_id: "opaque-1" },
			auth: { kind: "bearer", value: "synthetic-wire-secret", request_metadata },
		});
		for (const good of [null, {}, { project_id: "proj.alpha:beta-1_2" }, { project_id: "a".repeat(256) }]) {
			expect(validateInbound(response(good)).ok).toBe(true);
		}
		for (const bad of [
			{ project_id: "" },
			{ project_id: "a".repeat(257) },
			{ project_id: "has space" },
			{ project_id: "slash/project" },
			{ project_id: 7 },
			{ project_id: "p", email: "person@example.test" },
			"proj",
		]) {
			expect(validateInbound(response(bad)).ok).toBe(false);
		}
	});

	it("test_this_package_is_inside_the_published_supported_range", () => {
		const parse = (version: string) => version.split(/[+-]/)[0]?.split(".").slice(0, 3).map(Number) ?? [];
		const before = (a: number[], b: number[]) => {
			for (let index = 0; index < 3; index++) {
				if ((a[index] ?? 0) !== (b[index] ?? 0)) return (a[index] ?? 0) < (b[index] ?? 0);
			}
			return false;
		};
		const [lower = "", upper = ""] = WIRE_SCHEMA["x-openside-agent-versions"].split(",");
		expect(lower.startsWith(">=") && upper.startsWith("<")).toBe(true);
		const version = parse(agentPackage.version);
		expect(before(version, parse(lower.slice(2)))).toBe(false);
		expect(before(version, parse(upper.slice(1)))).toBe(true);
		expect(WIRE_SCHEMA["x-protocol"]).toBe(1);
		expect(sidecarVersions().openside_agent).toBe(agentPackage.version);
	});
});
