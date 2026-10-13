import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { PolicySettings, PolicyValidation } from "../api/types";
import { ClientContext } from "../context";
import { chooseOption } from "../test/select";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { PolicyPage } from "./PolicyPage";

const POLICY: PolicySettings = {
  default_mode: "guard",
  allow_origins: ["https://*.example.com"],
  deny_origins: [],
  rules: [{ effect: "deny", capability: "fs_write", pattern: "/etc/**" }],
};

function setup(options: { validate?: (call: Call) => PolicyValidation; put?: (call: Call) => Response; network?: { allow: string[]; restart_required: boolean } } = {}) {
  let policy = POLICY;
  let network = options.network ?? { allow: ["huggingface.co"], restart_required: false };
  const fake = fakeClient((call) => {
    if (call.path === "/v1/policy" && call.method === "GET") return json(200, policy);
    if (call.path === "/v1/policy" && call.method === "PUT") {
      if (options.put) return options.put(call);
      policy = call.body as PolicySettings;
      return json(200, policy);
    }
    if (call.path === "/v1/policy/validate") return json(200, options.validate?.(call) ?? { valid: true, problems: [] });
    if (call.path === "/v1/policy/network" && call.method === "GET") return json(200, network);
    if (call.path === "/v1/policy/network" && call.method === "PUT") {
      network = { allow: (call.body as { allow: string[] }).allow, restart_required: true };
      return json(200, network);
    }
    if (call.path === "/v1/policy/test") return json(200, { effect: "ask", reason: "sensitive_floor" });
    return json(404, { code: "not_found", message: "nope" });
  });
  render(
    <ClientContext.Provider value={fake.client}>
      <PolicyPage />
    </ClientContext.Provider>,
  );
  return fake;
}

describe("PolicyPage", () => {
  it("shows the live policy and saves an edit after the daemon checked it", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const allow = await screen.findByLabelText("Only these sites");
    expect(allow).toHaveValue("https://*.example.com");
    expect(screen.getByLabelText("Rule 1 pattern")).toHaveValue("/etc/**");
    expect(screen.queryByRole("button", { name: "Save policy" })).toBeNull();

    await user.type(screen.getByLabelText("Never these sites"), "https://*.bank.example");
    await chooseOption(user, screen.getByLabelText("Default mode"), /Read only/);
    await user.click(screen.getByRole("button", { name: "Save policy" }));

    await screen.findByDisplayValue("https://*.bank.example");
    const validate = calls.find((call) => call.path === "/v1/policy/validate")!.body;
    const put = calls.find((call) => call.method === "PUT" && call.path === "/v1/policy")!.body;
    expect(put).toEqual({ default_mode: "read-only", allow_origins: ["https://*.example.com"], deny_origins: ["https://*.bank.example"], rules: POLICY.rules });
    expect(validate).toEqual({ policy: put });
  });

  it("marks the exact entry the daemon rejects and saves nothing", async () => {
    const user = userEvent.setup();
    const { calls } = setup({
      validate: () => ({ valid: false, problems: [{ list: "rules", index: 1, message: "pattern must be absolute" }, { list: "allow_origins", index: 0, message: "not an origin" }] }),
    });
    await screen.findByLabelText("Only these sites");
    await user.click(screen.getByRole("button", { name: "Add rule" }));
    await user.type(screen.getByLabelText("Rule 2 pattern"), "relative/path");
    await user.click(screen.getByRole("button", { name: "Save policy" }));

    expect(await screen.findByText("pattern must be absolute")).toBeInTheDocument();
    expect(screen.getByText(/not an origin/)).toBeInTheDocument();
    expect(within(screen.getByText(/not an origin/)).getByText("https://*.example.com")).toBeInTheDocument();
    expect(calls.some((call) => call.method === "PUT")).toBe(false);
  });

  it("explains where to edit when this browser is not on the computer", async () => {
    const user = userEvent.setup();
    setup({ put: () => json(403, { code: "local_access_required", message: "local" }) });
    await user.type(await screen.findByLabelText("Never these sites"), "https://x.example");
    await user.click(screen.getByRole("button", { name: "Save policy" }));
    expect(await screen.findByText(/To change the policy, use openside on the computer that runs it/)).toBeInTheDocument();
  });

  it("saves the network allow-list and offers the restart that applies it", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const list = await screen.findByLabelText("Allowed addresses");
    expect(list).toHaveValue("huggingface.co");
    await user.type(list, "{Enter}*.example.org");
    await user.click(screen.getByRole("button", { name: "Save list" }));
    expect(await screen.findByRole("button", { name: "Restart daemon" })).toBeInTheDocument();
    expect(calls.find((call) => call.method === "PUT" && call.path === "/v1/policy/network")?.body).toEqual({ allow: ["huggingface.co", "*.example.org"] });
    expect(calls.find((call) => call.path === "/v1/policy/validate" && JSON.stringify(call.body).includes("network_allow"))).toBeDefined();
  });

  it("tries an action against the live policy and says why", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.type(await screen.findByLabelText("File, folder or address"), "https://shop.example/checkout");
    await chooseOption(user, screen.getByLabelText("Action"), "Payments and messages");
    await user.click(screen.getByRole("button", { name: "Check" }));
    const decision = await screen.findByText("Payments and messages always ask first.");
    expect(decision.closest("p")).toHaveTextContent("Ask me");
    expect(calls.find((call) => call.path === "/v1/policy/test")?.body).toEqual({ target: "https://shop.example/checkout", capability: "sensitive" });
  });
});
