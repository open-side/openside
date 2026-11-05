// Input guardrail: vendor keys and private keys in user prompts never reach a provider.
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";

const SECRETS: readonly [label: string, pattern: RegExp][] = [
	["private_key", /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g],
	["aws_access_key", /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/g],
	["anthropic_key", /\bsk-ant-[A-Za-z0-9_-]{20,}/g],
	["openai_key", /\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}/g],
	["github_token", /\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{60,})\b/g],
	["slack_token", /\bxox[abprs]-[A-Za-z0-9-]{10,}/g],
	["google_api_key", /\bAIza[0-9A-Za-z_-]{35}\b/g],
	["stripe_key", /\b(?:sk|rk)_live_[0-9A-Za-z]{20,}\b/g],
];

export function redactSecrets(text: string): string {
	let redacted = text;
	for (const [label, pattern] of SECRETS) redacted = redacted.replace(pattern, `[redacted ${label}]`);
	return redacted;
}

/** Request-only copy of `messages` with secrets removed from user prompts; history keeps the original. */
export function redactUserPrompts(messages: readonly AgentMessage[]): AgentMessage[] {
	return messages.map(message => {
		if (message.role !== "user") return message;
		if (typeof message.content === "string") {
			const content = redactSecrets(message.content);
			return content === message.content ? message : { ...message, content };
		}
		let changed = false;
		const content = message.content.map(block => {
			if (block.type !== "text") return block;
			const text = redactSecrets(block.text);
			if (text === block.text) return block;
			changed = true;
			return { ...block, text };
		});
		return changed ? { ...message, content } : message;
	});
}
