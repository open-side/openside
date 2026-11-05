// Re-anchors long runs on the request being served. Restating the conversation's first
// message would point a long-lived session at an older, finished task.
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";

export const REANCHOR_EVERY = 5;
export const REANCHOR_MAX_CHARS = 500;

/** Reminder for the `request`-th model request (1-based) of one run, if it is due. */
export function currentRequestAnchor(request: number, messages: readonly AgentMessage[]): string | undefined {
	if (request === 0 || request % REANCHOR_EVERY !== 0) return undefined;
	for (let index = messages.length - 1; index >= 0; index--) {
		const message = messages[index] as AgentMessage;
		if (message.role !== "user" || message.synthetic === true || typeof message.content !== "string") continue;
		const text = [...message.content].slice(0, REANCHOR_MAX_CHARS).join("");
		return (
			"<system-reminder>\nThe request you are working on: " +
			`"${text}". Check that your next step advances it; ` +
			"if it is done, answer.\n</system-reminder>"
		);
	}
	return undefined;
}
