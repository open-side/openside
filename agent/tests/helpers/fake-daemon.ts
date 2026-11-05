// In-process daemon peer for sidecar tests: a Unix socket server speaking the daemon side
// of protocol v1, with the session under test connected through the real transport.
import { expect } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Socket, UnixSocketListener } from "bun";
import { validateOutbound } from "../../src/protocol/validate";
import type { ScriptResponse } from "../../src/scripted";
import { JsonlConnection } from "../../src/transport/jsonl";
import { RpcRouter } from "../../src/transport/rpc";

// biome-ignore lint/suspicious/noExplicitAny: wire messages are inspected structurally in tests.
export type Wire = Record<string, any>;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export class FakeDaemon {
	readonly outputs: Wire[] = [];
	#socket: Socket<undefined> | undefined;
	#buffer = "";
	#lines: string[] = [];
	#waiter: (() => void) | undefined;
	#ended = false;
	#connected = Promise.withResolvers<void>();
	#pending: Uint8Array[] = [];

	/** @internal wiring from the listener */
	attach(socket: Socket<undefined>): void {
		this.#socket = socket;
		this.#connected.resolve();
	}

	/** @internal */
	onData(chunk: Uint8Array): void {
		this.#buffer += decoder.decode(chunk, { stream: true });
		let newline = this.#buffer.indexOf("\n");
		while (newline >= 0) {
			this.#lines.push(this.#buffer.slice(0, newline));
			this.#buffer = this.#buffer.slice(newline + 1);
			newline = this.#buffer.indexOf("\n");
		}
		this.#waiter?.();
	}

	/** @internal */
	onEnd(): void {
		this.#ended = true;
		this.#waiter?.();
	}

	connected(): Promise<void> {
		return this.#connected.promise;
	}

	get ended(): boolean {
		return this.#ended && this.#lines.length === 0;
	}

	/** Send raw bytes (no envelope added); for malformed-input tests. Writes are queued until drained. */
	sendRaw(line: string | Uint8Array): void {
		this.#pending.push(typeof line === "string" ? encoder.encode(line) : line);
		if (this.#pending.length === 1) this.flush();
	}

	/** @internal drain handler */
	flush(): void {
		while (this.#socket !== undefined && this.#pending.length > 0) {
			const head = this.#pending[0] as Uint8Array;
			const written = this.#socket.write(head);
			if (written < head.byteLength) {
				this.#pending[0] = head.subarray(Math.max(written, 0));
				return;
			}
			this.#pending.shift();
		}
	}

	send(type: string, fields: Wire = {}): void {
		this.sendRaw(`${JSON.stringify({ v: 1, type, ...fields })}\n`);
	}

	/** Next sidecar message or undefined after `timeoutMs`; every message must match the schema. */
	async tryReceive(timeoutMs: number): Promise<Wire | undefined> {
		const deadline = Date.now() + timeoutMs;
		while (this.#lines.length === 0) {
			if (this.#ended) throw new Error("Sidecar closed before expected output");
			const left = deadline - Date.now();
			if (left <= 0) return undefined;
			const { promise, resolve } = Promise.withResolvers<void>();
			this.#waiter = resolve;
			const timer = setTimeout(resolve, left);
			await promise;
			clearTimeout(timer);
			this.#waiter = undefined;
		}
		const line = this.#lines.shift() as string;
		const value: Wire = JSON.parse(line);
		const checked = validateOutbound(value);
		if (!checked.ok) throw new Error(`Outbound message violates the wire schema: ${checked.reason}: ${line}`);
		this.outputs.push(value);
		return value;
	}

	async receive(timeoutMs = 10_000): Promise<Wire> {
		const message = await this.tryReceive(timeoutMs);
		if (message === undefined) throw new Error(`No sidecar output within ${timeoutMs} ms`);
		return message;
	}

	/** Read until a message of `type` (and event `kind`); a fatal error on the way fails the test. */
	async until(type: string, opts: { kind?: string; timeoutMs?: number } = {}): Promise<Wire> {
		for (;;) {
			const message = await this.receive(opts.timeoutMs);
			if (message.type === type && (opts.kind === undefined || message.event?.kind === opts.kind)) return message;
			expect(message.type !== "error" || !message.fatal, JSON.stringify(message)).toBe(true);
		}
	}

	async open(
		opts: { tools?: Wire[]; resume_state?: Wire | null; agent?: Wire; team?: Wire | null; model?: string } = {},
	): Promise<Wire> {
		this.send("open", {
			session: "test-session",
			model: { id: opts.model ?? "script", fallback: [] },
			system_prompt_parts: ["Use daemon tools only."],
			tools: opts.tools ?? [],
			resume_state: opts.resume_state ?? null,
			config: { credentials: {}, agent: opts.agent ?? {} },
			team: opts.team ?? null,
		});
		return this.until("ready");
	}

	user(fields: { message_id?: string; text?: string; mode?: string } & Wire = {}): void {
		this.send("user_message", { message_id: "message-1", text: "start", mode: "now", ...fields });
	}

	result(call: Wire, outcome: Wire): void {
		this.send("tool_result", { rpc_id: call.rpc_id, outcome });
	}

	snapshots(): Wire[] {
		return this.outputs.filter(item => item.type === "state_snapshot").map(item => item.state);
	}

	sawTerminal(): boolean {
		return this.outputs.some(item => item.type === "bye" || (item.type === "error" && item.fatal));
	}

	close(): void {
		this.#socket?.end();
	}
}

/** A session under test: the object tests may inspect and the promise its serve loop returns. */
export interface Started<S> {
	session: S;
	done: Promise<void>;
}

/**
 * Run `fn` against a session started by `start` over a real Unix socket. On exit the
 * daemon asks for shutdown (unless the session already ended) and the serve loop must
 * finish within 10 s.
 */
export async function withFakeDaemon<S, T>(
	start: (connection: JsonlConnection, rpc: RpcRouter) => Started<S>,
	fn: (daemon: FakeDaemon, session: S) => Promise<T>,
): Promise<T> {
	const directory = mkdtempSync(join(tmpdir(), "osrpc-"));
	const path = join(directory, "rpc.sock");
	const daemon = new FakeDaemon();
	const listener: UnixSocketListener<undefined> = Bun.listen({
		unix: path,
		socket: {
			binaryType: "uint8array",
			open: socket => daemon.attach(socket),
			data: (_socket, chunk) => daemon.onData(chunk),
			drain: () => daemon.flush(),
			end: () => daemon.onEnd(),
			close: () => daemon.onEnd(),
		},
	});
	try {
		const connection = await JsonlConnection.connect(path);
		await daemon.connected();
		const rpc = new RpcRouter(connection);
		const started = start(connection, rpc);
		try {
			return await fn(daemon, started.session);
		} finally {
			if (!daemon.sawTerminal() && !daemon.ended) {
				daemon.send("shutdown");
				await daemon.until("bye").catch(() => undefined);
			}
			await Promise.race([
				started.done,
				Bun.sleep(10_000).then(() => {
					throw new Error("Session did not finish within 10 s");
				}),
			]);
			daemon.close();
		}
	} finally {
		listener.stop(true);
		rmSync(directory, { recursive: true, force: true });
	}
}

export function toolSpec(name = "probe"): Wire {
	return {
		name,
		description: "Daemon probe",
		parameters: { type: "object", properties: { value: { type: "string" } }, additionalProperties: false },
		approval: "read",
		deferrable: true,
	};
}

export function toolResponse(name = "probe", id = "call-1"): Wire {
	return { parts: [{ type: "tool_call", name, args: {}, id }] };
}

export function textResponse(text = "finished"): Wire {
	return { parts: [{ type: "text", text }] };
}

const scratch: string[] = [];

/** Write a §9 script to a temp file and point OPENSIDE_AGENT_SCRIPT at it. */
export function installScript(
	responses: (Wire | ScriptResponse)[],
	opts: { delayMs?: number; title?: string; subagents?: Record<string, Wire[]> } = {},
): string {
	const directory = mkdtempSync(join(tmpdir(), "openside-script-"));
	scratch.push(directory);
	const path = join(directory, "script.json");
	writeFileSync(
		path,
		JSON.stringify({
			responses,
			delay_ms: opts.delayMs ?? 0,
			...(opts.title === undefined ? {} : { title: opts.title }),
			...(opts.subagents === undefined ? {} : { subagents: opts.subagents }),
		}),
	);
	process.env.OPENSIDE_AGENT_SCRIPT = path;
	return path;
}

const PROVIDER_KEYS = [
	"ANTHROPIC_API_KEY",
	"OPENAI_API_KEY",
	"OPENROUTER_API_KEY",
	"GOOGLE_API_KEY",
	"GEMINI_API_KEY",
	"GITHUB_COPILOT_API_KEY",
	"GITHUB_COPILOT_API_TOKEN",
	"COPILOT_GITHUB_TOKEN",
];
const HOME_DIRS = [
	"HOME",
	"XDG_CONFIG_HOME",
	"XDG_DATA_HOME",
	"XDG_STATE_HOME",
	"XDG_RUNTIME_DIR",
	"CODEX_HOME",
	"CLAUDE_CONFIG_DIR",
];

/** Fresh home/XDG directories and no OPENSIDE_* or provider key variables; call in beforeEach. */
export function hermeticEnv(): void {
	for (const directory of scratch.splice(0)) rmSync(directory, { recursive: true, force: true });
	const root = mkdtempSync(join(tmpdir(), "openside-env-"));
	scratch.push(root);
	for (const name of HOME_DIRS) {
		const path = join(root, name.toLowerCase());
		mkdirSync(path);
		process.env[name] = path;
	}
	for (const name of Object.keys(process.env)) {
		if (name.startsWith("OPENSIDE_") || PROVIDER_KEYS.includes(name)) delete process.env[name];
	}
}
