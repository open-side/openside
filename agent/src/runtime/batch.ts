// Queued daemon messages: a short window collects a burst into one model request.
import { MAX_USER_TEXT_BYTES, type UserMessage } from "../protocol/types";

export const BATCH_WINDOW_MS = 100;
export const BATCH_MAX_EXTRA = 31;

export function textBytes(message: UserMessage): number {
	return Buffer.byteLength(message.text, "utf8");
}

/** Agent clients, channels and queue-mode messages never preempt a running owner turn. */
export function isQueued(message: UserMessage): boolean {
	return message.author_kind !== "person" || message.mode === "queue";
}

/** Takes the first queued message plus as many followers as fit one request. */
export function takeBatch(queue: UserMessage[]): { first: UserMessage; rest: UserMessage[] } | undefined {
	const first = queue.shift();
	if (first === undefined) return undefined;
	let size = textBytes(first);
	const rest: UserMessage[] = [];
	while (queue.length > 0 && rest.length < BATCH_MAX_EXTRA) {
		const next = textBytes(queue[0] as UserMessage);
		if (size + next > MAX_USER_TEXT_BYTES) break;
		size += next;
		rest.push(queue.shift() as UserMessage);
	}
	return { first, rest };
}
