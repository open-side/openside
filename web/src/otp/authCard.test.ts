import { describe, expect, it } from "vitest";
import { authFoot } from "./authCard";
import { STEPS } from "../onboarding/OnboardingPage";

describe("credential trust copy", () => {
  it("names the handler without promising script isolation", () => {
    expect(authFoot().textContent).toBe("Handled byopenside");
    expect(authFoot().textContent).not.toContain("Protected");
  });

  it("distinguishes tool results from page-script access", () => {
    const copy = STEPS.find((step) => step.id === "vault")!.lead;
    expect(copy).toContain("tool results");
    expect(copy).toContain("scripts the agent runs on a page can read its filled fields");
    expect(copy).not.toContain("never sees");
  });
});
