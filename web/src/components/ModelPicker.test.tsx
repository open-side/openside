import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Client } from "../api/client";
import { ClientContext } from "../context";
import { keyInfo, modelsDaemon } from "../models/testDaemon";
import { fakeClient, json } from "../test/fakeDaemon";
import { ModelPicker } from "./ModelPicker";

afterEach(cleanup);

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>;
}

function open(client: Client, value = "", onChange: (model: string) => void = () => undefined) {
  render(
    <ClientContext.Provider value={client}>
      <MemoryRouter>
        <Where />
        <Routes>
          <Route path="/" element={<ModelPicker value={value} onChange={onChange} />} />
          <Route path="/models" element={<p>models screen</p>} />
        </Routes>
      </MemoryRouter>
    </ClientContext.Provider>,
  );
}

describe("ModelPicker", () => {
  it("groups the catalog by provider and disables models that cannot run, saying why", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const daemon = modelsDaemon({ keys: [keyInfo("openai", "k1", "Main", "1111")], pools: { openai: [{ source: "vault", account_id: "k1" }] }, defaultModel: "openai:gpt-5" });
    open(daemon.client, "", onChange);
    expect(await screen.findByRole("button", { name: /Default \(openai:gpt-5\)/ })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Default/ }));
    const dialog = screen.getByRole("dialog", { name: "Models" });

    const openai = within(dialog).getByRole("group", { name: "OpenAI" });
    expect(within(openai).getByRole("radio", { name: /GPT-5 mini/ })).toBeEnabled();
    const anthropic = within(dialog).getByRole("group", { name: "Anthropic" });
    const sonnet = within(anthropic).getByRole("radio", { name: /Claude Sonnet 4.5/ });
    expect(sonnet).toBeDisabled();
    // Every Anthropic model shares one reason, so it is said once under the provider.
    expect(within(anthropic).getAllByText(/No credential yet/)).toHaveLength(1);
    await user.click(sonnet);
    expect(onChange).not.toHaveBeenCalled();

    expect(within(dialog).getByRole("radio", { name: /Default \(openai:gpt-5\)/ })).toBeEnabled();
    await user.click(within(openai).getByRole("radio", { name: /GPT-5 mini/ }));
    expect(onChange).toHaveBeenCalledWith("openai:gpt-5-mini");
  });

  it("keeps free-text custom models", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    open(modelsDaemon().client, "", onChange);
    await user.click(await screen.findByRole("button", { name: /Default/ }));
    await user.type(screen.getByLabelText("Custom model"), "x");
    expect(onChange).toHaveBeenLastCalledWith("x");
  });

  it("shows a typed custom model in the chip and in the field", async () => {
    const user = userEvent.setup();
    open(modelsDaemon().client, "acme:big");
    const chip = await screen.findByRole("button", { name: /acme:big/ });
    await user.click(chip);
    expect(screen.getByLabelText("Custom model")).toHaveValue("acme:big");
  });

  it("points a user with no usable model to Models & accounts", async () => {
    const user = userEvent.setup();
    open(modelsDaemon().client);
    await user.click(await screen.findByRole("button", { name: /Default/ }));
    expect(screen.getByText("No model is ready yet.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Models & accounts" }));
    expect(await screen.findByText("models screen")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent("/models");
  });

  it("falls back to the configured default and fallbacks when the catalog is unreadable", async () => {
    const user = userEvent.setup();
    const fake = fakeClient((call) => {
      if (call.path === "/v1/settings/models") return json(500, { code: "internal", message: "x" });
      if (call.path === "/v1/settings") return json(200, { default_model: "script", fallback_models: ["provider:big"], policy_mode: "guard" });
      return json(404, { code: "not_found", message: "nope" });
    });
    open(fake.client);
    await user.click(await screen.findByRole("button", { name: /Default \(script\)/ }));
    const dialog = screen.getByRole("dialog", { name: "Models" });
    expect(within(dialog).getByRole("radio", { name: /provider:big/ })).toBeEnabled();
  });
});
