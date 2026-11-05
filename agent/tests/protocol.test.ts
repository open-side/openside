import { beforeEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { MAX_LINE_BYTES, STATE_FORMAT } from "../src/protocol/types";
import { validateInbound, validateTeamContext } from "../src/protocol/validate";
import { ConnectionClosed, JsonlConnection, type UnixConnector } from "../src/transport/jsonl";
import { RpcClosed, type RpcRouter } from "../src/transport/rpc";
import { hermeticEnv, type Started, type Wire, withFakeDaemon } from "./helpers/fake-daemon";

interface Echo {
	connection: JsonlConnection;
	rpc: RpcRouter;
	received: Wire[];
}

// A minimal serve loop: replies are routed through the RPC router, everything else is
// recorded; `shutdown` answers `bye` like the session does.
function echo(connection: JsonlConnection, rpc: RpcRouter): Started<Echo> {
	const session: Echo = { connection, rpc, received: [] };
	const done = (async () => {
		for (;;) {
			const message = await connection.receive();
			if (message === null) break;
			if (rpc.dispatch(message)) continue;
			session.received.push(message);
			if (message.type === "error" && message.fatal) break;
			if (message.type === "shutdown") {
				await connection.send({ type: "bye" });
				break;
			}
			if (message.type === "abort") await connection.send({ type: "settled" });
		}
		rpc.close();
		await connection.close();
	})();
	return { session, done };
}

beforeEach(hermeticEnv);

describe("jsonl transport", () => {
	it("test_protocol_fuzz_survives_and_recovers", async () => {
		const bytes = new Uint8Array(100).map((_, index) => 128 + ((index * 37) % 128));
		const malformed: (string | Uint8Array)[] = [
			"{bad json}",
			"[]",
			'{"v":2,"type":"abort"}',
			'{"v":1,"type":"unknown"}',
			'{"v":1,"type":"user_message"}',
			'{"type":"abort"}',
			'{"v":true,"type":"abort"}',
			bytes,
			"x".repeat(MAX_LINE_BYTES + 100_000),
		];
		await withFakeDaemon(echo, async (daemon, session) => {
			for (const line of malformed) {
				daemon.sendRaw(line);
				daemon.sendRaw("\n");
			}
			daemon.send("abort");
			for (const _ of malformed) {
				const error = await daemon.receive();
				expect(error).toEqual({
					v: 1,
					type: "error",
					code: "protocol",
					message: "Invalid protocol message discarded",
					fatal: false,
				});
			}
			expect((await daemon.receive()).type).toBe("settled");
			expect(session.received.map(message => message.type)).toEqual(["abort"]);
		});
	});

	it("an unterminated oversized line is reported once and skipped", async () => {
		await withFakeDaemon(echo, async daemon => {
			daemon.sendRaw("y".repeat(MAX_LINE_BYTES + 10));
			daemon.sendRaw("y".repeat(1000));
			daemon.sendRaw('\n{"v":1,"type":"abort"}\n');
			expect((await daemon.receive()).code).toBe("protocol");
			expect((await daemon.receive()).type).toBe("settled");
		});
	});

	it("test_writer_serializes_concurrent_messages", async () => {
		await withFakeDaemon(echo, async (daemon, session) => {
			await Promise.all(
				Array.from({ length: 100 }, (_, index) =>
					session.connection.send({ type: "message_accepted", message_id: String(index) }),
				),
			);
			const values: Wire[] = [];
			for (let index = 0; index < 100; index++) values.push(await daemon.receive());
			expect(values.map(value => value.message_id)).toEqual(Array.from({ length: 100 }, (_, index) => String(index)));
		});
	});

	it("test_large_snapshot_survives_slow_pipe_reader", async () => {
		await withFakeDaemon(echo, async (daemon, session) => {
			const data = "synthetic-snapshot-canary-".repeat(120_000);
			await session.connection.send({ type: "state_snapshot", state: { format: STATE_FORMAT, data } });
			const snapshot = await daemon.receive();
			expect(snapshot.state.data).toBe(data);
		});
	});

	it("rejects outgoing lines above the limit", async () => {
		await withFakeDaemon(echo, async (_daemon, session) => {
			const data = "z".repeat(MAX_LINE_BYTES);
			expect(
				session.connection.send({ type: "state_snapshot", state: { format: STATE_FORMAT, data } }),
			).rejects.toThrow("Outgoing protocol line exceeds the size limit");
		});
	});

	it("a chunk with more frames than the read high-water mark loses none of them", async () => {
		const count = 300;
		const reader = (
			connection: JsonlConnection,
		): Started<{ start: () => void; lines: Wire[]; finished: Promise<void> }> => {
			const gate = Promise.withResolvers<void>();
			const lines: Wire[] = [];
			const done = (async () => {
				await gate.promise;
				for (;;) {
					const message = await connection.receive();
					if (message === null) break;
					lines.push(message);
				}
				await connection.close();
			})();
			return { session: { start: gate.resolve, lines, finished: done }, done };
		};
		await withFakeDaemon(reader, async (daemon, session) => {
			const frames = Array.from({ length: count }, (_, index) =>
				JSON.stringify({ v: 1, type: "user_message", message_id: `m${index}`, text: "t", mode: "now" }),
			);
			daemon.sendRaw(`${frames.join("\n")}\n`);
			daemon.close();
			// Real socket delivery: the whole flood must sit in the reader before anything is consumed.
			await Bun.sleep(100);
			session.start();
			await session.finished;
			expect(session.lines.map(line => line.message_id)).toEqual(frames.map((_, index) => `m${index}`));
		});
	});

	it("a write that throws rejects pending and later sends instead of hanging", async () => {
		const directory = mkdtempSync(join(tmpdir(), "osrpc-"));
		const path = join(directory, "rpc.sock");
		const listener = Bun.listen({ unix: path, socket: { data() {} } });
		try {
			const failing: UnixConnector = async options => {
				const socket = await Bun.connect(options);
				return new Proxy(socket, {
					get(target, key) {
						if (key === "write")
							return () => {
								throw new Error("EPIPE");
							};
						const value = Reflect.get(target, key);
						return typeof value === "function" ? value.bind(target) : value;
					},
				});
			};
			const connection = await JsonlConnection.connect(path, failing);
			expect(connection.send({ type: "settled" })).rejects.toBeInstanceOf(ConnectionClosed);
			expect(connection.send({ type: "bye" })).rejects.toBeInstanceOf(ConnectionClosed);
			await connection.close();
		} finally {
			listener.stop(true);
			rmSync(directory, { recursive: true, force: true });
		}
	});

	it("receive fills wire defaults", () => {
		const user = validateInbound({ v: 1, type: "user_message", message_id: "m", text: "hi", mode: "now" });
		expect(user.ok && user.value).toMatchObject({ author_kind: "person", parts: [] });
		const result = validateInbound({
			v: 1,
			type: "tool_result",
			rpc_id: "r",
			outcome: { status: "done", for_model: 1 },
		});
		expect(result.ok && result.value).toMatchObject({ outcome: { parts: [] } });
		expect(validateInbound({ v: 1, type: "abort", extra: 1 }).ok).toBe(false);
	});

	it("team caps count code points", () => {
		const team = (persona: string, roster = "") => ({ role: "chief", agent_id: "a", persona, roster });
		expect(validateTeamContext(team("😀".repeat(1500))).ok).toBe(true);
		expect(validateTeamContext(team("a".repeat(1501))).ok).toBe(false);
		expect(validateTeamContext(team("", "r".repeat(2001))).ok).toBe(false);
		expect(validateTeamContext({ ...team(""), role: "boss" }).ok).toBe(false);
	});
});

describe("reverse rpc", () => {
	it("routes tool results by rpc id and reports unknown ids", async () => {
		await withFakeDaemon(echo, async (daemon, session) => {
			const pending = session.rpc.toolCall(
				{ name: "probe", arguments: { value: "x" }, toolCallId: "call-1", subagent: "sub-1" },
				new AbortController().signal,
			);
			const call = await daemon.until("tool_call");
			expect(call).toMatchObject({ tool_call_id: "call-1", name: "probe", args: { value: "x" }, subagent: "sub-1" });
			expect(session.rpc.pendingCount).toBe(1);
			daemon.result(call, { status: "done", for_model: "ok" });
			expect(await pending).toEqual({ status: "done", for_model: "ok", parts: [] });
			expect(session.rpc.pendingCount).toBe(0);
			daemon.result(call, { status: "done", for_model: "again" });
			expect(await daemon.until("error")).toMatchObject({
				code: "bad_request",
				message: "Unknown reverse RPC response",
				fatal: false,
			});
		});
	});

	it("an aborted call is abandoned and its late reply dropped silently", async () => {
		await withFakeDaemon(echo, async (daemon, session) => {
			const controller = new AbortController();
			const pending = session.rpc.toolCall({ name: "probe", arguments: {}, toolCallId: "c" }, controller.signal);
			const call = await daemon.until("tool_call");
			controller.abort(new Error("stop"));
			expect(pending).rejects.toThrow("stop");
			daemon.result(call, { status: "error", message: "late" });
			daemon.send("abort");
			expect((await daemon.receive()).type).toBe("settled");
		});
	});

	it("credential exchanges correlate by id and fire-and-forget reports are not errors", async () => {
		await withFakeDaemon(echo, async (daemon, session) => {
			const lease = session.rpc.credentialRequest(
				{ provider: "anthropic", model: "m", requestId: "req-1", attempt: 0 },
				new AbortController().signal,
			);
			const request = await daemon.until("credential_request");
			expect(request).toMatchObject({ request_id: "req-1", provider: "anthropic", attempt: 0 });
			const response = {
				id: request.id,
				request_id: "req-1",
				lease_id: "lease-1",
				generation: 1,
				account: { source: "vault", account_id: "key1" },
				auth: { kind: "api_key", value: "secret-canary" },
			};
			daemon.send("credential_response", response);
			const reply = await lease;
			expect(reply.type).toBe("credential_response");
			if (reply.type !== "credential_response") throw new Error("unreachable");
			await session.rpc.credentialReport(
				{ lease: reply, outcome: { outcome: "cancelled", scope: "account" }, requestId: "req-1", streamStarted: false },
				{ await: false },
			);
			const report = await daemon.until("credential_report");
			expect(report).toMatchObject({
				lease_id: "lease-1",
				outcome: "cancelled",
				usage_windows: [],
				stream_started: false,
			});
			daemon.send("credential_reported", {
				id: report.id,
				request_id: "req-1",
				lease_id: "lease-1",
				generation: 1,
				action: "complete",
			});
			daemon.send("credential_response", { ...response, id: "unknown" });
			expect(await daemon.until("error")).toMatchObject({ message: "Unknown credential RPC response" });
			expect(JSON.stringify(daemon.outputs)).not.toContain("secret-canary");
		});
	});

	it("pending calls fail with RpcClosed when the connection ends", async () => {
		await withFakeDaemon(echo, async (daemon, session) => {
			const pending = session.rpc.toolCall(
				{ name: "probe", arguments: {}, toolCallId: "c" },
				new AbortController().signal,
			);
			await daemon.until("tool_call");
			daemon.send("error", { code: "internal", message: "Daemon stopped", fatal: true });
			expect(pending).rejects.toBeInstanceOf(RpcClosed);
			await pending.catch(() => undefined);
		});
	});
});
