// Parent-context copy for forked subagents (acceptance A-5 ⑤, PP-24 ③).
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";

function callIds(message: AgentMessage): string[] {
	if (message.role !== "assistant") return [];
	return message.content.flatMap(block => (block.type === "toolCall" ? [block.id] : []));
}

/** Tool call answered by a message, if any. */
export function answerId(message: AgentMessage): string | undefined {
	return message.role === "toolResult" ? message.toolCallId : undefined;
}

/**
 * Copy history up to the first tool-call batch lacking a result for every call.
 *
 * The delegating call itself is always such a batch: its result is the child's
 * output, so a child must never inherit it. Results without a kept call are
 * dropped as well, so a provider never sees an orphaned tool result.
 */
export function forkHistory(messages: readonly AgentMessage[]): AgentMessage[] {
	let cut = messages.length;
	// Truncating can strand an earlier call whose only result came after the cut,
	// so shrink until every kept call is answered inside the kept prefix.
	while (true) {
		const answered = new Set<string>();
		let incomplete: number | undefined;
		for (let index = cut - 1; index >= 0; index--) {
			const message = messages[index] as AgentMessage;
			const answer = answerId(message);
			if (answer !== undefined) answered.add(answer);
			else if (callIds(message).some(id => !answered.has(id))) incomplete = index;
		}
		if (incomplete === undefined) break;
		cut = incomplete;
	}
	const issued = new Set<string>();
	const kept: AgentMessage[] = [];
	for (const message of messages.slice(0, cut)) {
		for (const id of callIds(message)) issued.add(id);
		const answer = answerId(message);
		if (answer !== undefined && !issued.has(answer)) continue;
		kept.push(structuredClone(message));
	}
	return kept;
}
