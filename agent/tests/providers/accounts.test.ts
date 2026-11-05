import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { accountsReport } from "../../src/providers/accounts";
import { doctorReport } from "../../src/providers/doctor";
import { cliLookup } from "../../src/providers/subscriptions";

const saved = {
	broker: process.env.OPENSIDE_BROKER_TOKEN,
	openai: process.env.OPENAI_API_KEY,
};

afterEach(() => {
	for (const [name, value] of [
		["OPENSIDE_BROKER_TOKEN", saved.broker],
		["OPENAI_API_KEY", saved.openai],
	] as const) {
		if (value === undefined) delete process.env[name];
		else process.env[name] = value;
	}
});

describe("accounts", () => {
	it("test_account_diagnostics_pass_through_sanitized_daemon_metadata", async () => {
		const metadata = {
			sources: [
				{
					name: "vault",
					kind: "vault",
					available: true,
					reason: "Account available",
					accounts: [
						{
							id: "opaque-key-1",
							label: "Work",
							provider: "openai",
							models: ["openai:gpt-5"],
							available: true,
							enabled: true,
							ready: true,
							usage_windows: [{ name: "weekly", used_percent: null, reset_at_ms: null }],
							blocked_until_ms: null,
						},
					],
				},
			],
			cached_for_ms: 5000,
			truncated: false,
		};
		const config = {
			_diagnostics: { accounts: metadata },
			_vault_keys: { openai: "synthetic-ignored-secret-canary" },
		};
		const result = await accountsReport(config, { default: "openai:gpt-5" });
		expect(result).toEqual(metadata);
		expect(JSON.stringify(result)).not.toContain("synthetic-ignored-secret-canary");
		const report = doctorReport(config, { default: "openai:gpt-5" });
		expect(report.models).toMatchObject({ providers_ok: true });
		expect(JSON.stringify(report)).not.toContain("synthetic-ignored-secret-canary");
	});

	it("test_standalone_diagnostics_do_not_read_tokens_or_detect_login", async () => {
		const opened: unknown[] = [];
		const file = spyOn(Bun, "file").mockImplementation((...args: unknown[]) => {
			opened.push(args);
			throw new Error("Sidecar diagnostics must not inspect credential files");
		});
		const which = spyOn(cliLookup, "which").mockImplementation(() => "/fake/installed-cli");
		try {
			process.env.OPENSIDE_BROKER_TOKEN = "synthetic-broker-canary";
			process.env.OPENAI_API_KEY = "synthetic-key-canary";
			const config = {
				omp_broker: {
					url: "http://not-a-real-broker.invalid",
					token_file: "/fake/token",
				},
				command: { openai: ["not-a-real-command"] },
			};
			const result = await accountsReport(config, { default: "openai:gpt-5" });
			expect(opened).toEqual([]);
			const sources = result.sources as {
				available: boolean;
				accounts: unknown[];
				reason: string;
			}[];
			expect(sources.length).toBeGreaterThan(0);
			expect(sources.every(source => !source.available && source.accounts.length === 0)).toBe(true);
			expect(sources.every(source => source.reason.includes("unknown"))).toBe(true);
			const report = doctorReport(config, { default: "openai:gpt-5" });
			expect(report.models).toMatchObject({ providers_ok: false });
			const printed = JSON.stringify(result) + JSON.stringify(report);
			expect(printed).not.toContain("synthetic-broker-canary");
			expect(printed).not.toContain("synthetic-key-canary");
		} finally {
			file.mockRestore();
			which.mockRestore();
		}
	});
});
