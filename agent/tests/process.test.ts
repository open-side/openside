import { beforeEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FakeDaemon, hermeticEnv, installScript, textResponse } from "./helpers/fake-daemon";

const MAIN = join(import.meta.dir, "../src/main.ts");

beforeEach(hermeticEnv);

describe("sidecar process", () => {
	it("test_doctor_subprocess_one_json_without_canary", async () => {
		const canary = "broker-token-canary-process-852124";
		const child = Bun.spawn([process.execPath, "run", MAIN, "doctor", "--json"], {
			env: {
				...process.env,
				OPENSIDE_BROKER_TOKEN: canary,
				OPENSIDE_DOCTOR_CONFIG: JSON.stringify({ credentials: {}, models: {} }),
			},
			stdout: "pipe",
			stderr: "pipe",
		});
		const [stdout, stderr] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text()]);
		expect(await child.exited).toBe(0);
		expect(stdout.trimEnd().split("\n")).toHaveLength(1);
		const value = JSON.parse(stdout);
		expect(new Set(Object.keys(value))).toEqual(new Set(["runtime", "packages", "credentials", "models"]));
		expect(value.models.default).toBeNull();
		expect(stdout + stderr).not.toContain(canary);
	});

	it("test_diagnostics_do_not_initialize_the_session_runtime", () => {
		for (const command of ["accounts", "doctor"]) {
			const script = `
				import { main } from ${JSON.stringify(MAIN)};
				const code = await main([${JSON.stringify(command)}, "--json"]);
				const loaded = Object.keys(require.cache).filter(path => /agent\\/src\\/(runtime|protocol|transport)\\//.test(path));
				if (code !== 0 || loaded.length > 0) {
					console.error(JSON.stringify({ code, loaded }));
					process.exit(1);
				}
			`;
			const child = Bun.spawnSync([process.execPath, "-e", script], {
				env: { ...process.env, OPENSIDE_DOCTOR_CONFIG: "{}" },
				stderr: "pipe",
				timeout: 15_000,
			});
			expect(child.exitCode, child.stderr.toString()).toBe(0);
			const lines = child.stdout.toString().trimEnd().split("\n");
			expect(lines).toHaveLength(1);
			JSON.parse(lines[0] as string);
		}
	});

	it.if(process.platform !== "win32")(
		"test_real_sidecar_process_handshake_run_shutdown",
		async () => {
			installScript([textResponse("real process answer")]);
			const canary = "vault-canary-real-sidecar-581239";
			const directory = mkdtempSync(join(tmpdir(), "osrpc-"));
			const path = join(directory, "rpc.sock");
			const daemon = new FakeDaemon();
			const listener = Bun.listen({
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
			const child = Bun.spawn([process.execPath, "run", MAIN], {
				env: {
					...process.env,
					OPENSIDE_BROKER_TOKEN: canary,
					OPENSIDE_SIDECAR_SOCKET: path,
					OPENSIDE_SIDECAR_NONCE: "test-nonce",
				},
				stdin: "ignore",
				stdout: "pipe",
				stderr: "pipe",
			});
			try {
				await daemon.connected();
				const hello = await daemon.receive(15_000);
				expect(hello).toMatchObject({ type: "hello", nonce: "test-nonce", pid: child.pid, protocol: 1 });
				expect(hello.versions.runtime).toBe(`bun ${Bun.version}`);
				await daemon.open();
				daemon.user();
				const final = await daemon.until("event", { kind: "assistant.final" });
				expect(final.event.text).toBe("real process answer");
				await daemon.until("settled");
				daemon.send("shutdown");
				await daemon.until("bye");
				const [stdout, stderr] = await Promise.all([
					new Response(child.stdout).text(),
					new Response(child.stderr).text(),
				]);
				expect(await child.exited).toBe(0);
				expect(stdout).toBe("");
				expect(stdout + stderr).not.toContain(canary);
				expect(JSON.stringify(daemon.outputs)).not.toContain(canary);
			} finally {
				child.kill();
				daemon.close();
				listener.stop(true);
				rmSync(directory, { recursive: true, force: true });
			}
		},
		30_000,
	);
});
