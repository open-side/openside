import { describe, expect, it } from "bun:test";
import { join } from "node:path";

describe("egress guard", () => {
	it("test_bare_fetch_and_sockets_are_refused_outside_the_list", async () => {
		// The guard patches process globals permanently, so it runs in a child process.
		const child = Bun.spawn([process.execPath, join(import.meta.dir, "fixtures/egress-guard-child.ts")], {
			stdout: "pipe",
			stderr: "pipe",
			env: { ...process.env, NO_PROXY: "*" },
		});
		const [stdout, stderr, code] = await Promise.all([
			new Response(child.stdout).text(),
			new Response(child.stderr).text(),
			child.exited,
		]);
		expect(code).toBe(0);
		expect(JSON.parse(stdout.trim().split("\n").at(-1) ?? "{}")).toEqual({
			fetchRefused: "EgressRefused",
			fetchLoopback: "allowed",
			fetchAllowed: expect.not.stringMatching(/^(EgressRefused|allowed)$/),
			webSocket: "EgressRefused",
			bunConnect: "EgressRefused",
			netConnect: "EgressRefused",
			fetchRebind: "EgressRefused",
			bunConnectRebind: "EgressRefused",
			netConnectRebind: "EgressRefused",
			httpRebind: "EgressRefused",
			webSocketRebind: "EgressRefused",
			fetchPinned: expect.stringMatching(/^host=lan\.test:\d+$/),
			anthropicPinned: expect.not.stringMatching(/^(EgressRefused|allowed)$/),
			anthropicPinnedV6: expect.not.stringMatching(/^(EgressRefused|allowed)$/),
			anthropicMappedPrivate: "EgressRefused",
			httpsLiteral: "EgressRefused",
			httpsLiteralV6: "EgressRefused",
			httpsMapped: "EgressRefused",
			httpsMappedPrivate: "EgressRefused",
			fetchAdmitted: "allowed",
		});
		expect(stderr).toContain("outbound connection to https://collector.invalid:443 is not in network.allow");
		expect(stderr).toContain("outbound connection to rebind.example (10.0.0.7):443 is not in network.allow");
	});
});
