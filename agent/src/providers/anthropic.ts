/**
 * Claude Code subscription shaping (D5). pi-ai already applies the full Claude
 * Code fingerprint when a request is in OAuth mode — the
 * `claude-code-20250219,oauth-2025-04-20` betas, `user-agent: claude-cli/<ver>
 * (external, cli)`, `x-app: cli` and the identity as the first system block —
 * so `stream.ts` only selects OAuth mode from the daemon's auth kind
 * (`claude_code`) and never from a token prefix. Ordinary API keys are never
 * shaped. This module owns the version override; pi-ai reads it from
 * `PI_AI_CLAUDE_CODE_VERSION` on every request.
 */
import type { JsonObject } from "../protocol/types";

export { claudeCodeSystemInstruction as CLAUDE_CODE_IDENTITY } from "@oh-my-pi/pi-ai/providers/claude-code-fingerprint";

const SEMVER = /^\d+\.\d+\.\d+$/;

/**
 * The configured Claude Code version: `OPENSIDE_CLAUDE_CODE_VERSION`, then
 * `credentials.anthropic.claude_code_version`. Undefined (pi-ai's pinned
 * default, which may self-upgrade on a `claude_code_version_too_old`
 * rejection) when unset or not a plain semver.
 */
export function claudeCodeVersion(credentials: JsonObject): string | undefined {
	const section = credentials.anthropic;
	const configured =
		process.env.OPENSIDE_CLAUDE_CODE_VERSION ||
		(typeof section === "object" && section !== null && !Array.isArray(section)
			? section.claude_code_version
			: undefined);
	return typeof configured === "string" && SEMVER.test(configured) ? configured : undefined;
}

/** Pins the wire version for this process; one sidecar process serves one session. */
export function applyClaudeCodeVersion(version: string | undefined): void {
	if (version !== undefined && SEMVER.test(version)) process.env.PI_AI_CLAUDE_CODE_VERSION = version;
}
