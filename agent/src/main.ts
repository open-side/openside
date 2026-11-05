// Sidecar socket entrypoint and credential-value-free diagnostics.
// Runtime modules are imported dynamically on purpose: diagnostics must not load the
// session runtime, transport or provider SDKs they do not print.
import { dlopen, ptr } from "bun:ffi";
import { openSync } from "node:fs";
import { join } from "node:path";
import agentPackage from "../package.json" with { type: "json" };
import type { JsonlConnection } from "./transport/jsonl";

type Report = Record<string, unknown>;

const USAGE = "usage: openside-agent [doctor|accounts|models|probe] [--json]";
const RLIMIT_CORE = 4;
const PR_SET_DUMPABLE = 4;

/**
 * `auth-login` / `auth-refresh`: the daemon's one-shot OAuth helpers (private
 * stdio IPC). They hold tokens, so they get the session's dump protection, and
 * pi-ai's file logger is switched off so nothing reaches disk. Imports are
 * dynamic so a session never loads helper code and a helper never loads the session.
 */
async function authHelper(command: "auth-login" | "auth-refresh"): Promise<number> {
	const { stdioHelperIo } = await import("./protocol/auth-helper");
	const io = stdioHelperIo();
	try {
		protectSidecarProcess();
		const { logger } = await import("@oh-my-pi/pi-utils");
		logger.setTransports({ file: false });
		if (command === "auth-login") {
			const { authLogin } = await import("./auth-login");
			return await authLogin(io);
		}
		const { authRefresh } = await import("./auth-refresh");
		return await authRefresh(io);
	} catch {
		// Exception text can carry provider responses; the daemon only needs the code.
		io.write({ type: "error", code: "failed" });
		return 1;
	}
}

function packageVersions(): Record<string, string> {
	return {
		"@oh-my-pi/pi-agent-core": agentPackage.dependencies["@oh-my-pi/pi-agent-core"],
		"@oh-my-pi/pi-ai": agentPackage.dependencies["@oh-my-pi/pi-ai"],
		"openside-agent": agentPackage.version,
	};
}

function doctorConfig(): Report {
	const value: unknown = JSON.parse(process.env.OPENSIDE_DOCTOR_CONFIG ?? "{}");
	return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Report) : {};
}

function objectField(config: Report, key: string): Report {
	const value = config[key];
	return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Report) : {};
}

async function doctor(): Promise<Report> {
	const base = { runtime: `bun ${Bun.version}`, packages: packageVersions() };
	try {
		const { doctorReport } = await import("./providers/doctor");
		const config = doctorConfig();
		return { ...base, ...doctorReport(objectField(config, "credentials"), objectField(config, "models")) };
	} catch {
		return { ...base, credentials: [], models: { default: null, fallback: [], providers_ok: false } };
	}
}

async function accounts(): Promise<Report> {
	const { accountsReport } = await import("./providers/accounts");
	const config = doctorConfig();
	return accountsReport(objectField(config, "credentials"), objectField(config, "models"));
}

async function modelsCatalog(): Promise<Report> {
	const { catalogReport } = await import("./providers/catalog");
	const config = doctorConfig();
	const names = config.provider_names;
	return catalogReport(
		objectField(config, "models"),
		Array.isArray(names) ? names.filter((name): name is string => typeof name === "string") : [],
	);
}

/**
 * Apply only in the session child, before accepting private credential messages: no core
 * dumps and no ptrace/proc access from same-uid processes (execve resets dumpability, so
 * the daemon's pre-exec guard is reapplied here).
 */
export function protectSidecarProcess(): void {
	if (process.platform === "win32") return;
	const linux = process.platform === "linux";
	const libc = dlopen(linux ? "libc.so.6" : "libc.dylib", {
		setrlimit: { args: ["i32", "ptr"], returns: "i32" },
		...(linux ? { prctl: { args: ["i32", "u64", "u64", "u64", "u64"], returns: "i32" } } : {}),
	} as const);
	try {
		const limit = new BigUint64Array(2);
		if (libc.symbols.setrlimit(RLIMIT_CORE, ptr(limit)) !== 0) {
			throw new Error("Sidecar crash protection unavailable");
		}
		const { prctl } = libc.symbols as { prctl?: (...args: (number | bigint)[]) => number };
		if (prctl !== undefined && prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) !== 0) {
			throw new Error("Sidecar crash protection unavailable");
		}
	} finally {
		libc.close();
	}
}

// Test harness hook: e2e runs keep sidecar stderr in a per-process file under the harness
// root instead of the daemon's bounded diagnostic buffer.
function redirectTestStderr(): void {
	const root = process.env.OPENSIDE_TEST_STDERR;
	if (!root || !process.env.OPENSIDE_SIDECAR_NONCE || process.platform === "win32") return;
	const fd = openSync(join(root, `sidecar-${process.pid}.stderr`), "a", 0o600);
	const libc = dlopen(process.platform === "linux" ? "libc.so.6" : "libc.dylib", {
		dup2: { args: ["i32", "i32"], returns: "i32" },
		close: { args: ["i32"], returns: "i32" },
	});
	try {
		libc.symbols.dup2(fd, 2);
		libc.symbols.close(fd);
	} finally {
		libc.close();
	}
}

export interface SidecarDeps {
	protect: () => void;
	connect: (socketPath: string) => Promise<JsonlConnection>;
}

export async function sidecar(deps?: Partial<SidecarDeps>): Promise<void> {
	(deps?.protect ?? protectSidecarProcess)();
	const socketPath = process.env.OPENSIDE_SIDECAR_SOCKET;
	const nonce = process.env.OPENSIDE_SIDECAR_NONCE;
	if (!socketPath || !nonce) throw new Error("Sidecar socket environment missing");
	// Every outbound transport is guarded before anything else can connect: only
	// loopback, unix sockets (the daemon) and admitted provider/allow-list origins.
	const { installEgress } = await import("./providers/egress");
	installEgress([]);
	// Test-only preload (e2e hooks), honoured only for daemon-spawned sessions.
	const preload = process.env.OPENSIDE_AGENT_PRELOAD;
	if (preload) await import(preload);
	const { JsonlConnection } = await import("./transport/jsonl");
	const { RpcRouter } = await import("./transport/rpc");
	const { sidecarVersions } = await import("./protocol/versions");
	const connection = await (deps?.connect ?? JsonlConnection.connect)(socketPath);
	await connection.send({ type: "hello", protocol: 1, nonce, pid: process.pid, versions: sidecarVersions() });
	const { serveSession } = await import("./runtime/session");
	await serveSession(connection, new RpcRouter(connection));
}

/**
 * `probe`: load the module graph `sidecar()` loads, without connecting, so doctor catches a
 * package that cannot start a session (2026-11-05: a release copy missing a file `validate.ts`
 * imported passed `doctor`, then every session exited before its hello). Imports stay dynamic
 * because a failed load is the verdict; egress is guarded first, as in a session.
 */
async function probeSessionModules(): Promise<void> {
	const { installEgress } = await import("./providers/egress");
	installEgress([]);
	await import("./transport/jsonl");
	await import("./transport/rpc");
	await import("./protocol/versions");
	await import("./runtime/session");
}

export async function main(argv: string[]): Promise<number> {
	const [command, ...rest] = argv;
	const json = rest.length === 0 || (rest.length === 1 && rest[0] === "--json");
	if (command === "doctor" && json) {
		console.log(JSON.stringify(await doctor()));
		return 0;
	}
	if ((command === "accounts" || command === "models") && json) {
		try {
			const report = await (command === "accounts" ? accounts() : modelsCatalog());
			console.log(JSON.stringify(report));
			return 0;
		} catch {
			console.error("Account diagnostics unavailable");
			return 1;
		}
	}
	if (command === "probe" && json) {
		try {
			await probeSessionModules();
		} catch {
			// Module errors name paths and can quote source; doctor only needs the verdict.
			return 1;
		}
		console.log(JSON.stringify({ ok: true }));
		return 0;
	}
	if ((command === "auth-login" || command === "auth-refresh") && rest.length === 0) {
		return authHelper(command);
	}
	if (command !== undefined) {
		console.error(USAGE);
		return 2;
	}
	redirectTestStderr();
	try {
		await sidecar();
	} catch {
		// Exception strings can contain model input, tool arguments or credentials.
		console.error("Sidecar connection failed; check runtime configuration");
		return 1;
	}
	return 0;
}

if (import.meta.main) {
	// Explicit exit: a finished session must not linger on stray timers or sockets.
	process.exit(await main(Bun.argv.slice(2)));
}
