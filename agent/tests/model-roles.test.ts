// Model roles in the sidecar: Visual routing of image turns and `generate_image` continuation.
import { beforeEach, describe, expect, it } from "bun:test";
import { deflateSync } from "node:zlib";
import { streamSimple, type Usage } from "@oh-my-pi/pi-ai";
import type { ImageGenerationResult } from "@oh-my-pi/pi-ai/images";
import { createMockModel } from "@oh-my-pi/pi-ai/providers/mock";
import type { GenerateImage } from "../src/providers/images";
import { Session, type SessionOptions } from "../src/runtime/session";
import type { JsonlConnection } from "../src/transport/jsonl";
import type { RpcRouter } from "../src/transport/rpc";
import {
	type FakeDaemon,
	hermeticEnv,
	installScript,
	textResponse,
	type Wire,
	withFakeDaemon,
} from "./helpers/fake-daemon";

const PRIMARY = "anthropic:claude-haiku-5-5";
const VISUAL = "google:gemini-3.8-flash";
const IMAGE_MODEL = "google:gemini-3.1-flash-image";

function session(options: SessionOptions = {}) {
	return (conn: JsonlConnection, rpc: RpcRouter) => {
		const started = new Session(conn, rpc, options);
		return { session: started, done: started.serve() };
	};
}

function crc32Chunk(kind: string, data: Buffer): Buffer {
	const body = Buffer.concat([Buffer.from(kind, "latin1"), data]);
	const length = Buffer.alloc(4);
	length.writeUInt32BE(data.length);
	const crc = Buffer.alloc(4);
	crc.writeUInt32BE(Bun.hash.crc32(body));
	return Buffer.concat([length, body, crc]);
}

/** A valid 1x1 PNG, so attachment handling treats it as a real image. */
function png(seed: number): Buffer {
	const header = Buffer.alloc(13);
	header.writeUInt32BE(1, 0);
	header.writeUInt32BE(1, 4);
	header.set([8, 2, 0, 0, 0], 8);
	const pixel = deflateSync(Buffer.from([0, seed % 256, 0, 0]));
	return Buffer.concat([
		Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
		crc32Chunk("IHDR", header),
		crc32Chunk("IDAT", pixel),
		crc32Chunk("IEND", Buffer.alloc(0)),
	]);
}

interface Routing {
	builds: { model: string; fallback: string[] }[];
	served: string[];
	options: SessionOptions;
}

/** Records each stream function the session builds and which one served each request. */
function routing(): Routing {
	const answer = createMockModel({ handler: { content: ["ok"] } });
	const builds: Routing["builds"] = [];
	const served: string[] = [];
	return {
		builds,
		served,
		options: {
			makeStreamFn: (_port, ctx) => {
				builds.push({ model: ctx.model, fallback: [...(ctx.fallback ?? [])] });
				return (_model, context, streamOptions) => {
					served.push(ctx.model);
					return streamSimple(answer, context, streamOptions);
				};
			},
		},
	};
}

async function imageTurnThenText(daemon: FakeDaemon, agent: Wire): Promise<void> {
	await daemon.open({ model: PRIMARY, agent: { title_needed: false, ...agent } });
	daemon.send("user_message", {
		message_id: "message-1",
		text: "what is in this picture?",
		mode: "now",
		parts: [
			{
				type: "image",
				mime: "image/png",
				data_base64: png(7).toString("base64"),
				artifact_ref: "attachments/message-1/1-a.png",
				name: "a.png",
			},
		],
	});
	await daemon.until("settled");
	daemon.user({ message_id: "message-2", text: "thanks, and in one word?" });
	await daemon.until("settled");
}

function generateImageSpec(): Wire {
	return {
		name: "generate_image",
		description: "Generate images",
		parameters: {
			type: "object",
			properties: { prompt: { type: "string" }, aspect_ratio: { type: "string" }, count: { type: "integer" } },
			required: ["prompt"],
			additionalProperties: false,
		},
		approval: "read",
		deferrable: true,
	};
}

const ZERO_USAGE: Usage = {
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

interface ImageExchange {
	credentialRequests: Wire[];
	credentialReports: Wire[];
	stores: Wire[];
	generateCallId: string | undefined;
}

/**
 * Plays the daemon until the run settles: admits `generate_image`, leases one credential per
 * request, acknowledges reports and stores each image as `images/x-<index+1>.png`.
 */
async function playImageDaemon(daemon: FakeDaemon): Promise<ImageExchange> {
	const exchange: ImageExchange = {
		credentialRequests: [],
		credentialReports: [],
		stores: [],
		generateCallId: undefined,
	};
	for (;;) {
		const message = await daemon.receive();
		expect(message.type !== "error" || !message.fatal, JSON.stringify(message)).toBe(true);
		if (message.type === "settled") return exchange;
		if (message.type === "tool_call" && message.name === "generate_image") {
			exchange.generateCallId = message.tool_call_id;
			daemon.result(message, {
				status: "done",
				for_model: { status: "admitted", model: IMAGE_MODEL, count: 2, aspect_ratio: "1:1" },
			});
		} else if (message.type === "tool_call" && message.name === "__image_store") {
			exchange.stores.push(message);
			const index = message.args.index;
			daemon.result(message, {
				status: "done",
				for_model: typeof index === "number" ? { path: `images/x-${index + 1}.png` } : { ended: true },
			});
		} else if (message.type === "credential_request") {
			exchange.credentialRequests.push(message);
			daemon.send("credential_response", {
				id: message.id,
				request_id: message.request_id,
				lease_id: `lease-${exchange.credentialRequests.length}`,
				generation: 1,
				account: { source: "vault", account_id: "fake-account-1" },
				auth: { kind: "api_key", value: "image-lease-secret" },
			});
		} else if (message.type === "credential_report") {
			exchange.credentialReports.push(message);
			daemon.send("credential_reported", {
				id: message.id,
				request_id: message.request_id,
				lease_id: message.lease_id,
				generation: 2,
				action: "complete",
			});
		}
	}
}

function generateImageResult(daemon: FakeDaemon, toolCallId: string): Wire {
	const state = JSON.parse((daemon.snapshots().at(-1) as Wire).data);
	const result = (state.messages as Wire[]).find(
		message => message.role === "toolResult" && message.toolCallId === toolCallId,
	);
	expect(result).toBeDefined();
	return result as Wire;
}

beforeEach(() => hermeticEnv());

describe("model roles", () => {
	it("test_visual_model_serves_a_turn_with_an_image_attachment", async () => {
		const route = routing();
		await withFakeDaemon(session(route.options), async daemon => {
			await imageTurnThenText(daemon, { model_roles: { visual: VISUAL } });
		});
		expect(route.builds.map(build => build.model)).toEqual([PRIMARY, VISUAL]);
		const visual = route.builds.find(build => build.model === VISUAL);
		// The requesting model stays behind the Visual model when it cannot serve.
		expect(visual?.fallback[0]).toBe(PRIMARY);
		expect(route.served).toEqual([VISUAL, PRIMARY]);
	});

	it("test_visual_routing_is_off_without_a_visual_role", async () => {
		const route = routing();
		await withFakeDaemon(session(route.options), async daemon => {
			await imageTurnThenText(daemon, {});
		});
		expect(route.builds.map(build => build.model)).toEqual([PRIMARY]);
		expect(route.served).toEqual([PRIMARY, PRIMARY]);
	});

	it("test_generate_image_stores_each_image_and_reports_paths", async () => {
		installScript([
			{ parts: [{ type: "tool_call", name: "generate_image", args: { prompt: "a red cube" }, id: "image-call" }] },
			textResponse("Here are your images."),
		]);
		const images = [png(1).toString("base64"), png(2).toString("base64")];
		const calls: { model: string; request: Wire }[] = [];
		const generateImage: GenerateImage = async (model, request) => {
			calls.push({ model: `${model.provider}:${model.id}`, request: request as Wire });
			const result: ImageGenerationResult = {
				images: images.map(data => ({ data, mimeType: "image/png" })),
				usage: ZERO_USAGE,
			};
			return result;
		};
		await withFakeDaemon(session({ generateImage }), async daemon => {
			await daemon.open({ tools: [generateImageSpec()], agent: { title_needed: false } });
			daemon.user({ text: "draw a red cube" });
			const exchange = await playImageDaemon(daemon);
			expect(exchange.generateCallId).toBe("image-call");
			expect(exchange.credentialRequests.map(request => [request.provider, request.model])).toEqual([
				["google", IMAGE_MODEL],
			]);
			expect(calls).toHaveLength(1);
			expect(calls[0]?.model).toBe(IMAGE_MODEL);
			expect(calls[0]?.request).toMatchObject({ prompt: "a red cube", count: 2, aspectRatio: "1:1" });
			expect(exchange.stores.map(store => store.args)).toEqual([
				{ tool_call_id: "image-call", index: 0, data_base64: images[0], last: false },
				{ tool_call_id: "image-call", index: 1, data_base64: images[1], last: true },
			]);
			expect(exchange.credentialReports.map(report => report.outcome)).toEqual(["success"]);
			const result = generateImageResult(daemon, "image-call");
			expect(result.isError).toBe(false);
			expect(result.details).toEqual({
				status: "generated",
				model: IMAGE_MODEL,
				images: ["images/x-1.png", "images/x-2.png"],
			});
			// What the model reads is the tool result's text content, not `details`.
			const text = JSON.stringify(result.content);
			expect(text).toContain("generated");
			expect(text).toContain("images/x-1.png");
			expect(text).toContain("images/x-2.png");
			expect(JSON.stringify(daemon.outputs)).not.toContain("image-lease-secret");
		});
	});

	it("test_generate_image_failure_ends_the_grant", async () => {
		installScript([
			{ parts: [{ type: "tool_call", name: "generate_image", args: { prompt: "a red cube" }, id: "image-call" }] },
			textResponse("The image could not be made."),
		]);
		const generateImage: GenerateImage = async () => {
			throw new Error("provider exploded");
		};
		await withFakeDaemon(session({ generateImage }), async daemon => {
			await daemon.open({ tools: [generateImageSpec()], agent: { title_needed: false } });
			daemon.user({ text: "draw a red cube" });
			const exchange = await playImageDaemon(daemon);
			expect(exchange.stores.map(store => store.args)).toEqual([{ tool_call_id: "image-call", last: true }]);
			const result = generateImageResult(daemon, "image-call");
			expect(result.details).toMatchObject({ status: "failed", model: IMAGE_MODEL });
			expect(JSON.stringify(result.content)).toContain("failed");
			expect(JSON.stringify(daemon.outputs)).not.toContain("provider exploded");
		});
	});
});
