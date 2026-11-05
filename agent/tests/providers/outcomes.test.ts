import { describe, expect, it } from "bun:test";
import { classifyResponse, type OutcomeKind, retryAfterSeconds } from "../../src/providers/outcomes";

describe("outcomes", () => {
	const cases: [number, unknown, OutcomeKind][] = [
		[200, undefined, "success"],
		[401, { error: "synthetic-secret-canary" }, "auth"],
		[403, { error: { type: "authentication_error" } }, "auth"],
		[403, { error: { type: "permission_error" } }, "policy"],
		[429, { error: { code: "insufficient_quota" } }, "quota"],
		[429, { error: "synthetic-secret-canary" }, "rate_limit"],
		[429, { error: { code: "rpm_limit_exceeded" } }, "transient"],
		[500, undefined, "transient"],
		[400, { error: { message: "rate_limit synthetic-secret-canary" } }, "transient"],
	];
	for (const [status, body, expected] of cases) {
		it(`test_response_classification_is_structured_and_secret_free[${status}-${expected}]`, () => {
			const outcome = classifyResponse(status, "31", body);
			expect(outcome.kind).toBe(expected);
			expect(outcome.retryAfter).toBe(["rate_limit", "quota", "policy"].includes(expected) ? 31 : undefined);
			expect(`${String(outcome)} ${JSON.stringify(outcome)} ${Bun.inspect(outcome)}`).not.toContain(
				"synthetic-secret-canary",
			);
		});
	}

	const retryCases: [string | undefined, number | undefined][] = [
		[undefined, undefined],
		["bad", undefined],
		["nan", undefined],
		["inf", undefined],
		["-1", 0],
		["0.5", 0.5],
		["99999999999", 86400],
	];
	for (const [value, expected] of retryCases) {
		it(`test_retry_after_is_bounded_and_finite[${value}]`, () => {
			expect(retryAfterSeconds(value)).toBe(expected);
		});
	}

	it("test_retry_after_accepts_http_date", () => {
		const value = new Date(Date.now() + 60_000).toUTCString();
		const seconds = retryAfterSeconds(value) ?? -1;
		expect(seconds).toBeGreaterThanOrEqual(58);
		expect(seconds).toBeLessThanOrEqual(60);
	});

	it("test_model_scoped_policy_and_unknown_reset_remain_distinct", () => {
		const outcome = classifyResponse(403, undefined, {
			error: { scope: "model", type: "permission_error" },
		});
		expect(outcome.kind).toBe("policy");
		expect(outcome.scope).toBe("model");
		expect(outcome.retryAfterMs).toBeUndefined();
		expect(classifyResponse(429).retryAfterMs).toBeUndefined();
	});
});
