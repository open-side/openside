import { describe, expect, it } from "bun:test";
import type { CredentialReply } from "../../src/protocol/types";
import { validateInbound, validateOutbound } from "../../src/protocol/validate";
import { DaemonCredentialRpc } from "../../src/providers/credential-rpc";
import { AuthUnavailable } from "../../src/providers/errors";

const CANARY = "synthetic-private-wire-secret-canary";

function response(fields: Record<string, unknown> = {}): Record<string, unknown> {
	return {
		v: 1,
		type: "credential_response",
		id: "rpc1",
		request_id: "model1",
		lease_id: "lease1",
		generation: 1,
		account: { source: "vault", account_id: "opaque1" },
		auth: { kind: "api_key", value: CANARY },
		...fields,
	};
}

function replying(reply: Record<string, unknown>): DaemonCredentialRpc {
	return new DaemonCredentialRpc({
		credentialRequest: async () => reply as unknown as CredentialReply,
		credentialReport: async () => undefined,
	});
}

const printed = (value: unknown): string => `${String(value)} ${JSON.stringify(value)} ${Bun.inspect(value)}`;

describe("credential bounds", () => {
	for (const attempt of [-1, 64, true]) {
		it(`test_request_attempt_is_zero_based_and_bounded[${attempt}]`, async () => {
			const checked = validateOutbound({
				v: 1,
				type: "credential_request",
				id: "rpc1",
				request_id: "model1",
				provider: "anthropic",
				model: "anthropic:test",
				attempt,
			});
			expect(checked.ok).toBe(false);
			const rpc = replying(response());
			await expect(
				rpc.acquire(
					"anthropic",
					"anthropic:test",
					"model1",
					attempt as number,
					undefined,
					new AbortController().signal,
				),
			).rejects.toBeInstanceOf(AuthUnavailable);
		});
	}

	const auths: [string, Record<string, unknown>][] = [
		[
			"refresh_token",
			{
				kind: "api_key",
				value: CANARY,
				refresh_token: "synthetic-refresh-canary",
			},
		],
		["credential_home", { kind: "api_key", value: CANARY, credential_home: "/fake/private-home" }],
		["oversized", { kind: "api_key", value: "x".repeat(16385) }],
	];
	for (const [name, auth] of auths) {
		it(`test_invalid_private_response_is_rejected_without_echo[${name}]`, async () => {
			const checked = validateInbound(response({ auth }));
			expect(checked.ok).toBe(false);
			const reason = checked.ok ? "" : checked.reason;
			expect(reason).not.toContain(CANARY);
			expect(reason).not.toContain("synthetic-refresh-canary");
			if (name === "oversized") {
				const error = await replying(response({ auth }))
					.acquire("openai", "openai:gpt-5", "model1", 0, undefined, new AbortController().signal)
					.then(
						() => undefined,
						(failure: unknown) => failure,
					);
				expect(error).toBeInstanceOf(AuthUnavailable);
				expect(printed(error)).not.toContain("xxxxxxxx");
			}
		});
	}

	it("test_private_snapshot_repr_is_redacted", async () => {
		const parsed = validateInbound(response());
		expect(parsed.ok).toBe(true);
		const lease = await replying(response()).acquire(
			"openai",
			"openai:gpt-5",
			"model1",
			0,
			undefined,
			new AbortController().signal,
		);
		expect(printed(lease) + printed(lease.credential) + printed(lease.credential.secret)).not.toContain(CANARY);
		expect(lease.credential.secret.reveal()).toBe(CANARY);
		lease.credential.clear();
	});
});
