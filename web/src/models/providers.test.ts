import { describe, expect, it } from "vitest";
import { providerLabel } from "./providers";

describe("providerLabel", () => {
  it("names provider namespaces the way a person says them", () => {
    expect(providerLabel("google-gla")).toBe("Google");
    expect(providerLabel("google-vertex")).toBe("Google");
    expect(providerLabel("openai-codex")).toBe("OpenAI (ChatGPT plan)");
    expect(providerLabel("mistral")).toBe("Mistral");
  });
});
