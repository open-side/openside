// Versioned JSONL wire contract; semantics in docs/specs/sidecar-protocol.md, shape in
// agent/src/protocol/sidecar-wire.schema.json (the hand-maintained canonical schema).
// Names mirror the former Python wire models so specs and tests stay greppable.

export const MAX_LINE_BYTES = 4 * 1024 * 1024;
export const MAX_USER_TEXT_BYTES = MAX_LINE_BYTES / 4;
export const STATE_FORMAT = "openside-agent-state/2";
// Snapshots written by the retired Python sidecar; the runtime starts fresh on these.
export const LEGACY_STATE_FORMAT = "openside-agent-state/1";

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: JsonValue };

interface Envelope {
	v: 1;
}

export type ToolApproval = "read" | "write" | "exec" | "sensitive";

export interface ToolSpec {
	name: string;
	description: string;
	parameters: JsonObject;
	approval: ToolApproval;
	deferrable: boolean;
}

export interface ModelSpec {
	id: string;
	fallback: string[];
}

export interface OpenConfig {
	credentials: JsonObject;
	agent: JsonObject;
}

export interface AgentState {
	format: typeof STATE_FORMAT | typeof LEGACY_STATE_FORMAT;
	data: string;
}

export interface TeamContext {
	role: "chief" | "teammate";
	agent_id: string;
	/** max 1500 chars */
	persona: string;
	/** max 2000 chars */
	roster: string;
}

export interface Open extends Envelope {
	type: "open";
	session: string;
	model: ModelSpec;
	system_prompt_parts: string[];
	tools: ToolSpec[];
	resume_state?: AgentState;
	config: OpenConfig;
	team?: TeamContext;
}

/**
 * Attachment or binary tool-result content (PP-14). `data_base64` carries the bytes
 * inline; `artifact_ref` names the daemon-held copy. A part with only `artifact_ref`
 * was not inlined (too large or unsupported).
 */
export interface ContentPart {
	type: "text" | "image" | "audio" | "file";
	mime: string;
	data_base64?: string;
	artifact_ref?: string;
	name?: string;
}

export type AuthorKind = "person" | "agent_client" | "channel" | "internal";
export type MessageMode = "queue" | "steer" | "now";

export interface UserMessage extends Envelope {
	type: "user_message";
	message_id: string;
	text: string;
	mode: MessageMode;
	/** Absent on the wire means "person"; the validator fills the default. */
	author_kind: AuthorKind;
	/** Absent on the wire means []; the validator fills the default. */
	parts: ContentPart[];
}

export interface Abort extends Envelope {
	type: "abort";
}

export interface Shutdown extends Envelope {
	type: "shutdown";
}

export interface SubagentCancel extends Envelope {
	type: "subagent_cancel";
	subagent_id: string;
}

export interface SubagentSteer extends Envelope {
	type: "subagent_steer";
	subagent_id: string;
	text: string;
	control_id: string;
}

export interface DiscardDeferred extends Envelope {
	type: "discard_deferred";
}

export interface Done {
	status: "done";
	for_model: JsonValue;
	/** Absent on the wire means []; the validator fills the default. */
	parts: ContentPart[];
}

export type DeferredKind = "approval" | "external";

export interface DeferredOutcome {
	status: "deferred";
	ticket_id: string;
	kind: DeferredKind;
}

export interface Denied {
	status: "denied";
	reason: string;
	hint?: string;
}

export interface ToolError {
	status: "error";
	message: string;
}

export type ToolOutcome = Done | DeferredOutcome | Denied | ToolError;

export interface ToolResult extends Envelope {
	type: "tool_result";
	rpc_id: string;
	outcome: ToolOutcome;
}

export type CredentialErrorCode =
	| "auth_unavailable"
	| "account_blocked"
	| "account_not_in_pool"
	| "provider_unavailable"
	| "invalid_credential_request";

export type AccountSource =
	| "omp-broker"
	| "vault"
	| "env"
	| "command"
	| "claude-code"
	| "codex-cli"
	| "builtin"
	| "plugin";

export interface ModelAccountRef {
	source: AccountSource;
	/** 1..128 chars */
	account_id: string;
}

export type CredentialKind = "api_key" | "claude_code" | "codex_cli" | "bearer";

export interface CredentialAuth {
	kind: CredentialKind;
	/** Secret, 1..16384 chars. Never log or echo. */
	value: string;
	upstream_account_id?: string;
	expires_at_ms?: number;
	/** Private endpoint replacing the provider default for this lease (a plugin relay); max 512 chars. */
	base_url?: string | null;
	/** Private non-credential `x-` headers for that endpoint; max 8 entries. */
	headers?: Record<string, string> | null;
	/** The relay's refusal mapping, applied before generic status classification. */
	error_rules?: CredentialErrorRules | null;
	/** Private request context (a Cloud Code project); `project_id` 1..256 chars of `[A-Za-z0-9._:-]`. */
	request_metadata?: CredentialRequestMetadata | null;
}

export type CredentialRuleOutcome = "unsupported_client" | "sign_in" | "capacity" | "rate_limit";

export interface CredentialErrorRule {
	status: number;
	hint?: string | null;
	body_contains?: string | null;
	outcome: CredentialRuleOutcome;
	message?: string | null;
}

export interface CredentialErrorRules {
	hint_header?: string | null;
	/** At most 16; the first match wins. */
	rules: CredentialErrorRule[];
}

export interface CredentialRequestMetadata {
	project_id?: string;
}

export interface CredentialRequest extends Envelope {
	type: "credential_request";
	id: string;
	request_id: string;
	provider: string;
	model: string;
	/** 0..63 */
	attempt: number;
	subagent?: string;
}

export interface CredentialResponse extends Envelope {
	type: "credential_response";
	id: string;
	request_id: string;
	lease_id: string;
	generation: number;
	account: ModelAccountRef;
	auth: CredentialAuth;
}

export interface CredentialError extends Envelope {
	type: "credential_error";
	id: string;
	request_id: string;
	code: CredentialErrorCode;
	/** max 512 chars; daemon-authored, not shown to the model */
	reason: string;
	retry_at_ms?: number;
	generation: number;
}

export interface CredentialUsage {
	input_tokens: number;
	output_tokens: number;
	cache_read_tokens: number;
	cache_write_tokens: number;
}

export interface CredentialUsageWindow {
	name: string;
	used_percent?: number;
	reset_at_ms?: number;
}

export type CredentialOutcome = "success" | "auth" | "rate_limit" | "quota" | "policy" | "transient" | "cancelled";

export interface CredentialReport extends Envelope {
	type: "credential_report";
	id: string;
	request_id: string;
	lease_id: string;
	outcome: CredentialOutcome;
	stream_started: boolean;
	/** 0..86_400_000 */
	retry_after_ms?: number;
	scope: "account" | "model";
	usage?: CredentialUsage;
	/** max 64 entries; always present on the wire */
	usage_windows: CredentialUsageWindow[];
}

export type CredentialAction = "complete" | "refresh" | "rotate" | "stop";

export interface CredentialReported extends Envelope {
	type: "credential_reported";
	id: string;
	request_id: string;
	lease_id: string;
	generation: number;
	action: CredentialAction;
	retry_at_ms?: number;
	reason?: string;
}

export interface Approved {
	status: "approved";
}

export interface Rejected {
	status: "denied";
	reason: string;
}

export interface ExternalResult {
	status: "result";
	for_model: JsonValue;
}

export type Resolution = Approved | Rejected | ExternalResult;

export interface TicketResolved extends Envelope {
	type: "ticket_resolved";
	ticket_id: string;
	tool_call_id: string;
	resolution: Resolution;
}

export interface Versions {
	runtime: string;
	pi_agent_core: string;
	pi_ai: string;
	openside_agent: string;
}

export interface Hello extends Envelope {
	type: "hello";
	protocol: 1;
	nonce: string;
	pid: number;
	versions: Versions;
}

export interface DeferredCall {
	tool_call_id: string;
	ticket_id: string;
	kind: DeferredKind;
}

export interface Ready extends Envelope {
	type: "ready";
	tools: string[];
	pending_deferred: DeferredCall[];
	startup_ms: number;
}

export interface TurnStarted {
	kind: "turn.started";
}

export interface TurnEnded {
	kind: "turn.ended";
}

export interface AssistantDelta {
	kind: "assistant.delta";
	text: string;
}

export interface AssistantFinal {
	kind: "assistant.final";
	text: string;
}

export interface SessionTitleProposed {
	kind: "session.title_proposed";
	title: string;
}

export interface ThinkingDelta {
	kind: "thinking.delta";
	text: string;
}

export interface SubagentLifecycle {
	kind: "subagent.lifecycle";
	subagent_id: string;
	status: string;
	// Failure diagnostics are redacted by the daemon before recording.
	output?: string;
	known_subagents?: string[];
	error_code?: string;
	diagnostic?: string;
}

export interface SubagentProgress {
	kind: "subagent.progress";
	subagent_id: string;
	progress: JsonValue;
}

export interface Usage {
	kind: "usage";
	model: string;
	input_tokens: number;
	output_tokens: number;
}

export type EventBody =
	| TurnStarted
	| TurnEnded
	| AssistantDelta
	| AssistantFinal
	| SessionTitleProposed
	| ThinkingDelta
	| SubagentLifecycle
	| SubagentProgress
	| Usage;

export interface Event extends Envelope {
	type: "event";
	event: EventBody;
}

export interface MessageAccepted extends Envelope {
	type: "message_accepted";
	message_id: string;
}

export type MessageOutcome = "completed" | "aborted" | "failed" | "deferred";

export interface MessageResult extends Envelope {
	type: "message_result";
	message_id: string;
	outcome: MessageOutcome;
	reason?: string;
}

export interface ToolCall extends Envelope {
	type: "tool_call";
	rpc_id: string;
	tool_call_id: string;
	name: string;
	args: JsonObject;
	subagent?: string;
	approval_ticket?: string;
}

export interface Deferred extends Envelope {
	type: "deferred";
	calls: DeferredCall[];
}

export interface StateSnapshot extends Envelope {
	type: "state_snapshot";
	state: AgentState;
}

export interface Settled extends Envelope {
	type: "settled";
}

/** Earlier messages left one agent's model context; absent subagent = the parent. */
export interface ContextCompacted extends Envelope {
	type: "context_compacted";
	subagent?: string;
}

export type ErrorCode =
	| "protocol"
	| "bad_request"
	| "stale_ticket"
	| "model_error"
	| "auth_unavailable"
	| "tool_gate"
	| "internal";

export interface Error extends Envelope {
	type: "error";
	code: ErrorCode;
	message: string;
	fatal: boolean;
	message_id?: string;
}

export interface SubagentSteerResult extends Envelope {
	type: "subagent_steer_result";
	control_id: string;
	delivered: boolean;
}

export interface Bye extends Envelope {
	type: "bye";
}

export type Inbound =
	| Open
	| UserMessage
	| Abort
	| ToolResult
	| CredentialResponse
	| CredentialError
	| CredentialReported
	| TicketResolved
	| Shutdown
	| Error
	| SubagentCancel
	| SubagentSteer
	| DiscardDeferred;

export type Outbound =
	| Hello
	| Ready
	| Event
	| MessageAccepted
	| MessageResult
	| ToolCall
	| CredentialRequest
	| CredentialReport
	| Deferred
	| StateSnapshot
	| Settled
	| ContextCompacted
	| Error
	| SubagentSteerResult
	| Bye;

export type OutboundType = Outbound["type"];

/** Outbound message as built by callers; `send` adds the `v: 1` envelope. */
export type OutboundBody = Outbound extends infer M ? (M extends Outbound ? Omit<M, "v"> : never) : never;

export type CredentialReply = CredentialResponse | CredentialError | CredentialReported;
/** The classified result of one model request, as reported for a lease. */
export interface OutcomeWire {
	outcome: CredentialOutcome;
	scope: "account" | "model";
	retry_after_ms?: number;
	usage?: CredentialUsage;
	usage_windows?: CredentialUsageWindow[];
}
