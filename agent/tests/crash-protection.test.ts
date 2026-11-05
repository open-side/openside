import { dlopen, ptr } from "bun:ffi";
import { describe, expect, it } from "bun:test";
import { join } from "node:path";
import { sidecar } from "../src/main";

const RLIMIT_CORE = 4;
const PR_GET_DUMPABLE = 3;
const linux = process.platform === "linux";

function probe(): { limits: [string, string]; dumpable: number | null } {
	const libc = dlopen(linux ? "libc.so.6" : "libc.dylib", {
		getrlimit: { args: ["i32", "ptr"], returns: "i32" },
		...(linux ? { prctl: { args: ["i32", "u64", "u64", "u64", "u64"], returns: "i32" } } : {}),
	} as const);
	try {
		const limit = new BigUint64Array(2);
		libc.symbols.getrlimit(RLIMIT_CORE, ptr(limit));
		const { prctl } = libc.symbols as { prctl?: (...args: (number | bigint)[]) => number };
		return {
			limits: [String(limit[0]), String(limit[1])],
			dumpable: prctl === undefined ? null : prctl(PR_GET_DUMPABLE, 0, 0, 0, 0),
		};
	} finally {
		libc.close();
	}
}

describe.if(process.platform !== "win32")("crash protection", () => {
	it("test_session_child_disables_core_dumps_and_dumpability_without_touching_parent", () => {
		const parent = probe();
		const main = join(import.meta.dir, "../src/main.ts");
		const script = `
			import { dlopen, ptr } from "bun:ffi";
			import { protectSidecarProcess } from ${JSON.stringify(main)};
			protectSidecarProcess();
			const linux = process.platform === "linux";
			const libc = dlopen(linux ? "libc.so.6" : "libc.dylib", {
				getrlimit: { args: ["i32", "ptr"], returns: "i32" },
				...(linux ? { prctl: { args: ["i32", "u64", "u64", "u64", "u64"], returns: "i32" } } : {}),
			});
			const limit = new BigUint64Array(2);
			libc.symbols.getrlimit(${RLIMIT_CORE}, ptr(limit));
			console.log(JSON.stringify({
				limits: [String(limit[0]), String(limit[1])],
				dumpable: linux ? libc.symbols.prctl(${PR_GET_DUMPABLE}, 0, 0, 0, 0) : null,
			}));
		`;
		const child = Bun.spawnSync([process.execPath, "-e", script], { stderr: "pipe", timeout: 15_000 });
		expect(child.exitCode, child.stderr.toString()).toBe(0);
		const report = JSON.parse(child.stdout.toString());
		expect(report.limits).toEqual(["0", "0"]);
		if (linux) expect(report.dumpable).toBe(0);
		expect(probe()).toEqual(parent);
	});

	it("test_session_fails_closed_before_opening_the_credential_socket", async () => {
		let connected = false;
		process.env.OPENSIDE_SIDECAR_SOCKET = "/nonexistent/rpc.sock";
		process.env.OPENSIDE_SIDECAR_NONCE = "nonce";
		try {
			await expect(
				sidecar({
					protect: () => {
						throw new Error("Crash protection unavailable");
					},
					connect: async () => {
						connected = true;
						throw new Error("Must not connect without crash protection");
					},
				}),
			).rejects.toThrow("Crash protection unavailable");
			expect(connected).toBe(false);
		} finally {
			delete process.env.OPENSIDE_SIDECAR_SOCKET;
			delete process.env.OPENSIDE_SIDECAR_NONCE;
		}
	});
});
