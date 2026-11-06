/** Visual role routing: requests that carry a fresh image run on the configured Visual model. */
import type { StreamFn } from "@oh-my-pi/pi-agent-core";
import type { Context, Model } from "@oh-my-pi/pi-ai";

/**
 * True when the input since the last assistant turn (the person's message, or tool results such
 * as screenshots) contains an image. Older images were already answered by the requesting model.
 */
export function hasFreshImage(context: Context): boolean {
	for (let index = context.messages.length - 1; index >= 0; index--) {
		const message = context.messages[index];
		if (message === undefined || message.role === "assistant") return false;
		const content = message.content;
		if (Array.isArray(content) && content.some(part => part.type === "image")) return true;
	}
	return false;
}

/** Sends image-bearing requests to `visual` with `visualModel`; everything else to `primary`. */
export function routeVisual(primary: StreamFn, visual: StreamFn, visualModel: Model): StreamFn {
	return (model, context, options) =>
		hasFreshImage(context) ? visual(visualModel, context, options) : primary(model, context, options);
}
