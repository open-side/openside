// Parent assistant stream -> wire events. Child agents emit no parent stream events.
import type { AssistantMessage, AssistantMessageEvent, Usage as LlmUsage } from "@oh-my-pi/pi-ai";
import type { EventBody, Usage } from "../protocol/types";

/** Daemon usage counts all prompt tokens, cached or not. */
export function usageEvent(model: string, usage: LlmUsage): Usage {
	return {
		kind: "usage",
		model,
		input_tokens: usage.input + usage.cacheRead + usage.cacheWrite,
		output_tokens: usage.output,
	};
}

export interface TurnEventsOptions {
	emit: (event: EventBody) => void;
	/** Front agents publish only through send_message; nothing streams. */
	front: boolean;
	/** Ultrabrowse: text waits until the response proves not to be the final answer. */
	holdFinalText: boolean;
	fallbackModel: string;
}

export class TurnEvents {
	turnOpen = false;
	#held: string[] = [];

	constructor(readonly options: TurnEventsOptions) {}

	started(): void {
		this.#held = [];
		this.options.emit({ kind: "turn.started" });
		this.turnOpen = true;
	}

	delta(event: AssistantMessageEvent): void {
		const { emit, front, holdFinalText } = this.options;
		if (event.type === "text_delta" && !front && event.delta) {
			if (holdFinalText) this.#held.push(event.delta);
			else emit({ kind: "assistant.delta", text: event.delta });
		} else if (event.type === "thinking_delta" && !front && event.delta) {
			emit({ kind: "thinking.delta", text: event.delta });
		}
	}

	/** A finished response; failed and cancelled ones only close their turn later. */
	response(message: AssistantMessage): void {
		if (message.stopReason === "error" || message.stopReason === "aborted") return;
		const { emit, front } = this.options;
		// Text beside tool calls is narration, never the final answer.
		if (!front && this.#held.length > 0 && message.content.some(block => block.type === "toolCall")) {
			emit({ kind: "assistant.delta", text: this.#held.join("") });
		}
		this.#held = [];
		emit(usageEvent(message.model || this.options.fallbackModel, message.usage));
		emit({ kind: "turn.ended" });
		this.turnOpen = false;
	}

	/** A failed or cancelled request still closes its turn. */
	closeTurn(): void {
		if (!this.turnOpen) return;
		this.options.emit({ kind: "turn.ended" });
		this.turnOpen = false;
	}
}
