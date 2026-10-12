import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AccountsResponse } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json } from "../test/fakeDaemon";
import { AccountsPage } from "./AccountsPage";

const blockedUntil = Date.now() + 3_600_000;

const RESPONSE: AccountsResponse = {
  sources: [
    {
      name: "broker",
      kind: "omp-broker",
      available: true,
      reason: "",
      accounts: [
        {
          id: "broker:1",
          label: "w***@example.com",
          provider: "anthropic",
          models: ["provider-a/model-large"],
          usage_windows: [
            { name: "5h", used_percent: 82.4, reset_at_ms: Date.now() + 600_000 },
            { name: "weekly", used_percent: null, reset_at_ms: null },
          ],
          blocked_until_ms: blockedUntil,
        },
      ],
    },
    { name: "env", kind: "env", available: false, reason: "No provider key in the environment.", accounts: [] },
  ],
  cached_for_ms: 5000,
  truncated: false,
};

describe("AccountsPage", () => {
  it("shows accounts, models, usage windows and rate-limit blocks per source", async () => {
    const fake = fakeClient(() => json(200, RESPONSE));
    render(
      <ClientContext.Provider value={fake.client}>
        <AccountsPage />
      </ClientContext.Provider>,
    );
    const broker = await screen.findByRole("region", { name: "Source broker" });
    expect(within(broker).getByText("w***@example.com")).toBeInTheDocument();
    expect(within(broker).getByText(/provider-a\/model-large/)).toBeInTheDocument();
    expect(within(broker).getByRole("meter", { name: "5h usage" })).toHaveAttribute("aria-valuenow", "82.4");
    expect(within(broker).getByText(/82% used · resets/)).toBeInTheDocument();
    expect(within(broker).getByText("usage unknown")).toBeInTheDocument();
    expect(within(broker).getByText(/Rate limited until/)).toBeInTheDocument();

    const env = screen.getByRole("region", { name: "Source env" });
    expect(within(env).getByText("Unavailable")).toBeInTheDocument();
    expect(within(env).getByText("No provider key in the environment.")).toBeInTheDocument();
    expect(fake.calls.map((c) => c.path)).toEqual(["/v1/accounts"]);
  });
});
