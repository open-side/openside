import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "../api/types";
import { AgentsContext } from "../hooks/useAgents";
import { ClientContext } from "../context";
import { keyInfo, modelRoutes, modelState } from "../models/testDaemon";
import { fakeClient, json } from "../test/fakeDaemon";
import { ModelButton } from "./ModelSheet";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>;
}

const chief = { id: "chief", name: "openside", model: null } as unknown as AgentSummary;

function setup(agent: AgentSummary = chief, update: () => Response = () => json(200, { ...agent })) {
  const routes = modelRoutes(
    modelState({ keys: [keyInfo("openai", "k1", "Main", "1111")], pools: { openai: [{ source: "vault", account_id: "k1" }] }, defaultModel: "openai:gpt-6.1-sol" }),
  );
  const fake = fakeClient((call) => (call.method === "PATCH" && call.path === `/v1/agents/${agent.id}` ? update() : (routes(call) ?? json(404, { code: "not_found", message: "nope" }))));
  const refresh = vi.fn();
  render(
    <ClientContext.Provider value={fake.client}>
      <AgentsContext.Provider value={{ agents: [agent], error: null, offline: false, refresh }}>
        <MemoryRouter>
          <Where />
          <Routes>
            <Route path="/" element={<ModelButton agent={agent} />} />
            <Route path="/models" element={<p>models screen</p>} />
          </Routes>
        </MemoryRouter>
      </AgentsContext.Provider>
    </ClientContext.Provider>,
  );
  return { ...fake, refresh };
}

describe("ModelButton", () => {
  it("names the default model and saves a pick from the catalog for this person", async () => {
    const user = userEvent.setup();
    const { calls, refresh } = setup();
    await user.click(screen.getByRole("button", { name: "Model: Default model" }));
    const dialog = await screen.findByRole("dialog", { name: "Model" });
    expect(within(dialog).getByRole("radio", { name: /^Default/ })).toHaveAttribute("aria-checked", "true");

    const openai = await within(dialog).findByRole("group", { name: "OpenAI" });
    const luna = within(openai).getByRole("radio", { name: /GPT-6 Luna/ });
    expect(luna).toHaveTextContent("Fast");
    const anthropic = within(dialog).getByRole("group", { name: "Anthropic" });
    expect(within(anthropic).getByRole("radio", { name: /Claude Sonnet 5.5/ })).toBeDisabled();

    await user.click(luna);
    expect(calls.find((call) => call.method === "PATCH")?.body).toEqual({ model: "openai:gpt-6-luna" });
    expect(refresh).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog", { name: "Model" })).toBeNull();
  });

  it("shows the catalog label of a chosen model and returns to the default", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ ...chief, model: "openai:gpt-6.1-sol" });
    await user.click(await screen.findByRole("button", { name: "Model: GPT-6.1 Sol" }));
    const dialog = await screen.findByRole("dialog", { name: "Model" });
    await user.click(within(dialog).getByRole("radio", { name: /^Default/ }));
    expect(calls.find((call) => call.method === "PATCH")?.body).toEqual({ model: null });
  });

  it("keeps the sheet open with an error when saving fails", async () => {
    const user = userEvent.setup();
    setup(chief, () => json(500, { code: "internal", message: "boom" }));
    await user.click(screen.getByRole("button", { name: "Model: Default model" }));
    const dialog = await screen.findByRole("dialog", { name: "Model" });
    await user.click(within(await within(dialog).findByRole("group", { name: "OpenAI" })).getByRole("radio", { name: /GPT-6 Luna/ }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Could not change openside's model.");
  });

  it("links to Models & accounts", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: "Model: Default model" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Model" })).getByRole("link", { name: "Manage models & accounts" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/models");
  });

  it("is a plain label for a token without admin", () => {
    localStorage.setItem("openside.scopes", JSON.stringify(["read", "steer"]));
    setup();
    expect(screen.getByLabelText("Model: Default model").tagName).toBe("SPAN");
    expect(screen.queryByRole("button", { name: /^Model:/ })).toBeNull();
  });
});
