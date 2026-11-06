import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { Client } from "../../api/client";
import { ClientContext } from "../../context";
import { keyInfo, modelsDaemon, oauthAccount, subscriptionAccount, type ModelState } from "../../models/testDaemon";
import type { Call } from "../../test/fakeDaemon";
import { chooseOption } from "../../test/select";
import { ToastProvider } from "../../ui";
import { ModelsSettings, modelsRequest } from "./Models";

afterEach(cleanup);

const POLL = { timeout: 5000 };
const PASTED = "https://localhost:53692/callback?code=fixture-code-77&state=s1";
const ref = (source: string, id: string) => ({ source, account_id: id });

function open(client: Client) {
  return render(
    <ClientContext.Provider value={client}>
      <ToastProvider>
        <ModelsSettings />
      </ToastProvider>
    </ClientContext.Provider>,
  );
}

/** Anthropic and OpenAI keys in their pools: chat models from both, and one image model. */
function withKeys(patch: Partial<ModelState> = {}) {
  return modelsDaemon({
    keys: [keyInfo("anthropic", "k1", "Anthropic"), keyInfo("openai", "k2", "OpenAI")],
    pools: { anthropic: [ref("vault", "k1")], openai: [ref("vault", "k2")] },
    defaultModel: "anthropic:claude-sonnet-5-5",
    ...patch,
  });
}

const providerList = () => screen.findByRole("list", { name: "Providers" });
const rowOf = async (name: string) => (await screen.findByRole("group", { name })) as HTMLElement;
const modelSaves = (daemon: { calls: Call[] }) => daemon.calls.filter((call) => call.method === "PUT" && call.path === "/v1/settings/models");

async function rowMenu(user: UserEvent, name: string) {
  await user.click(await screen.findByRole("button", { name: `${name} options` }));
  return screen.findByRole("menu", { name: `${name} options` });
}

async function connect(user: UserEvent) {
  await user.click(await screen.findByRole("button", { name: "Connect" }));
  return screen.findByRole("dialog", { name: "Connect an AI account" });
}

describe("Settings › Models", () => {
  it("lists one row per provider, then the task model cards in Aside's order, without pool or rotation words", async () => {
    const daemon = withKeys({ oauthAccounts: [oauthAccount("anthropic", "oa-9", "Claude", { auth_invalid: true })] });
    open(daemon.client);
    const list = await providerList();
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(2));
    expect(within(list).getAllByRole("listitem").map((row) => row.dataset.provider)).toEqual(["anthropic", "openai"]);
    // The key and the sign-in share the Anthropic row; the subtitle names each and flags the broken one.
    expect(within(list).getAllByRole("group").map((row) => row.textContent)).toEqual(["AnthropicAPI key, Claude needs attention", "OpenAIAPI key"]);
    expect(within(await rowOf("Anthropic")).getByRole("img", { name: "Connected" })).toBeInTheDocument();
    const menu = await rowMenu(userEvent.setup(), "Anthropic");
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Replace API key", "Remove API key", "Sign in to Claude again", "Disconnect Claude"]);
    expect(screen.getByRole("button", { name: "Connect" })).toBeInTheDocument();

    await screen.findByRole("combobox", { name: "Default model" });
    const selects = ["Default model", "Fast", "Standard", "Deep", "Visual", "Image generation model"].map((name) => screen.getByRole("combobox", { name }));
    for (const [index, select] of selects.slice(1).entries()) expect(selects[index]!.compareDocumentPosition(select) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByRole("combobox", { name: "Default model" })).toHaveTextContent("Claude Sonnet 5.5");
    expect(screen.getByRole("combobox", { name: "Fast" })).toHaveTextContent("Session model");
    expect(document.body.textContent).not.toMatch(/pool|rotation|broker|reserve|credential/i);
  });

  it.each([
    ["Default model", "default_model", /GPT-6\.1 Sol/, "openai:gpt-6.1-sol"],
    ["Fast", "fast_model", /GPT-6 Luna/, "openai:gpt-6-luna"],
    ["Standard", "standard_model", /Claude Haiku 5\.5/, "anthropic:claude-haiku-5-5"],
    ["Deep", "deep_model", /GPT-6 Astra/, "openai:gpt-6-astra"],
    ["Visual", "visual_model", /GPT-6\.1 Sol/, "openai:gpt-6.1-sol"],
  ])("%s writes %s and keeps every other role", async (title, field, option, id) => {
    const user = userEvent.setup();
    const daemon = withKeys({ fastModel: "anthropic:claude-haiku-5-5", deepModel: "anthropic:claude-sonnet-5-5", fallbacks: ["openai:gpt-6.1-sol"] });
    open(daemon.client);
    await chooseOption(user, await screen.findByRole("combobox", { name: title }), option);
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    const previous = {
      default_model: "anthropic:claude-sonnet-5-5",
      fast_model: "anthropic:claude-haiku-5-5",
      standard_model: null,
      deep_model: "anthropic:claude-sonnet-5-5",
      visual_model: null,
    };
    // Image generation is never part of another role's save.
    const next = { ...previous, [field]: id };
    // A new default never stays a fallback too; the daemon refuses the pair.
    expect(modelSaves(daemon)[0]!.body).toEqual({ ...next, fallback_models: ["openai:gpt-6.1-sol"].filter((fallback) => fallback !== next.default_model) });
    expect(await screen.findByText("Saved.")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("combobox", { name: title })).toHaveTextContent(option));
  });

  it("returns a task to the session model from its … menu", async () => {
    const user = userEvent.setup();
    const daemon = withKeys({ deepModel: "openai:gpt-6-astra" });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "More for Deep" }));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Use the session model" }));
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    expect(modelSaves(daemon)[0]!.body).toMatchObject({ deep_model: null, default_model: "anthropic:claude-sonnet-5-5" });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Deep" })).toHaveTextContent("Session model"));
    expect(daemon.state.deepModel).toBeNull();
  });

  it("resets every task to the session model from the default model's … menu", async () => {
    const user = userEvent.setup();
    const daemon = withKeys({ fastModel: "openai:gpt-6-luna", standardModel: "openai:gpt-6-luna", visualModel: "openai:gpt-6.1-sol" });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "More for Default model" }));
    await user.click(await screen.findByRole("menuitem", { name: "Use the session model for every task" }));
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    expect(modelSaves(daemon)[0]!.body).toMatchObject({ fast_model: null, standard_model: null, deep_model: null, visual_model: null });
  });

  it("leaves out a stored role whose model is no longer usable, so the save still goes through", async () => {
    const user = userEvent.setup();
    const daemon = withKeys({ visualModel: "google-gla:gemini-2.5-pro" });
    open(daemon.client);
    await chooseOption(user, await screen.findByRole("combobox", { name: "Fast" }), /GPT-6 Luna/);
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    expect(modelSaves(daemon)[0]!.body).not.toHaveProperty("visual_model");
    expect(daemon.state.visualModel).toBe("google-gla:gemini-2.5-pro");
  });

  it("drops a stale fast model and fallback it must send, so picking another role still saves", async () => {
    const user = userEvent.setup();
    const daemon = withKeys({ fastModel: "google-gla:gemini-2.5-flash", fallbacks: ["google-gla:gemini-2.5-pro"] });
    open(daemon.client);
    await chooseOption(user, await screen.findByRole("combobox", { name: "Deep" }), /GPT-6 Astra/);
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    expect(modelSaves(daemon)[0]!.body).toMatchObject({ fast_model: null, fallback_models: [], deep_model: "openai:gpt-6-astra" });
    expect(daemon.state.deepModel).toBe("openai:gpt-6-astra");
  });

  it("offers only usable models, and no image model when none is connected", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ keys: [keyInfo("anthropic", "k1", "Anthropic")], pools: { anthropic: [ref("vault", "k1")] }, defaultModel: "anthropic:claude-sonnet-5-5" });
    open(daemon.client);
    const image = await screen.findByRole("combobox", { name: "Image generation model" });
    expect(image).toHaveTextContent("None available");
    expect(image).toHaveAttribute("data-disabled");
    await user.click(screen.getByRole("combobox", { name: "Fast" }));
    const options = (await screen.findAllByRole("option")).map((option) => option.textContent);
    expect(options).toEqual(["Claude Sonnet 5.5", "Claude Haiku 5.5"]);
  });

  it("asks for a default model first and keeps the task rows waiting until there is one", async () => {
    const daemon = withKeys({ defaultModel: null });
    open(daemon.client);
    expect(await screen.findByRole("combobox", { name: "Default model" })).toHaveTextContent("Choose a model");
    expect(screen.getByRole("combobox", { name: "Fast" })).toHaveAttribute("data-disabled");
  });
});

describe("Task model safety", () => {
  it("shows the model automatic image generation uses, with Pick automatically checked", async () => {
    const user = userEvent.setup();
    const daemon = withKeys();
    open(daemon.client);
    expect(await screen.findByRole("combobox", { name: "Image generation model" })).toHaveTextContent("GPT Image 2");
    await user.click(screen.getByRole("button", { name: "More for Image generation model" }));
    expect(await screen.findByRole("menuitemcheckbox", { name: "Pick automatically" })).toHaveAttribute("aria-checked", "true");
    expect(modelSaves(daemon)).toHaveLength(0);
  });

  it("saves None as off, keeps it after a reload, and a new default model leaves it alone", async () => {
    const user = userEvent.setup();
    const daemon = withKeys();
    open(daemon.client);
    await chooseOption(user, await screen.findByRole("combobox", { name: "Image generation model" }), "None");
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    expect(modelSaves(daemon)[0]!.body).toMatchObject({ image_model: "off" });
    expect(daemon.state.imageModel).toBe("off");
    cleanup();
    open(daemon.client);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Image generation model" })).toHaveTextContent("None"));
    await chooseOption(user, screen.getByRole("combobox", { name: "Default model" }), /GPT-6\.1 Sol/);
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(2));
    expect(modelSaves(daemon)[1]!.body).toMatchObject({ default_model: "openai:gpt-6.1-sol" });
    expect(modelSaves(daemon)[1]!.body).not.toHaveProperty("image_model");
    expect(daemon.state.imageModel).toBe("off");
  });

  it("never writes image generation when the default model or another role changes", async () => {
    const user = userEvent.setup();
    const daemon = withKeys();
    open(daemon.client);
    await chooseOption(user, await screen.findByRole("combobox", { name: "Default model" }), /GPT-6\.1 Sol/);
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    await chooseOption(user, screen.getByRole("combobox", { name: "Fast" }), /GPT-6 Luna/);
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(2));
    for (const save of modelSaves(daemon)) expect(save.body).not.toHaveProperty("image_model");
    expect(daemon.state.imageModel).toBeNull();
  });

  it("stores a picked image model, and Pick automatically goes back to automatic", async () => {
    const user = userEvent.setup();
    const daemon = withKeys({ imageModel: "off" });
    open(daemon.client);
    await chooseOption(user, await screen.findByRole("combobox", { name: "Image generation model" }), /GPT Image 2/);
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    expect(modelSaves(daemon)[0]!.body).toMatchObject({ image_model: "openai:gpt-image-2" });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Image generation model" })).toHaveTextContent("GPT Image 2"));
    await user.click(screen.getByRole("button", { name: "More for Image generation model" }));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Pick automatically" }));
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(2));
    expect(modelSaves(daemon)[1]!.body).toMatchObject({ image_model: null });
    expect(daemon.state.imageModel).toBeNull();
  });

  it("waits for a usable default when the stored one is gone, then sends only usable chat models", async () => {
    const user = userEvent.setup();
    // The stored default, fast model and fallbacks name a model with no account and an image model.
    const daemon = withKeys({ defaultModel: "google-gla:gemini-2.5-pro", fastModel: "openai:gpt-image-2", fallbacks: ["openai:gpt-image-2", "google-gla:gemini-2.5-pro", "anthropic:claude-haiku-5-5"] });
    open(daemon.client);
    expect(await screen.findByRole("combobox", { name: "Default model" })).toHaveTextContent("Choose a model");
    expect(screen.getByRole("combobox", { name: "Fast" })).toHaveAttribute("data-disabled");
    expect(screen.getByRole("combobox", { name: "Image generation model" })).toHaveAttribute("data-disabled");
    await chooseOption(user, screen.getByRole("combobox", { name: "Default model" }), /Claude Sonnet 5\.5/);
    await waitFor(() => expect(modelSaves(daemon)).toHaveLength(1));
    expect(modelSaves(daemon)[0]!.body).toMatchObject({ default_model: "anthropic:claude-sonnet-5-5", fast_model: null, fallback_models: ["anthropic:claude-haiku-5-5"] });
    expect(daemon.state.defaultModel).toBe("anthropic:claude-sonnet-5-5");
  });

  it("never sends an image model to a chat role", () => {
    const daemon = withKeys({ visualModel: "openai:gpt-image-2" });
    // modelsRequest reads the same catalog the page does; build it from the fake directly.
    return daemon.client.modelCatalog().then((catalog) => {
      expect(modelsRequest(catalog, { fast_model: "openai:gpt-6-luna" })).not.toHaveProperty("visual_model");
      expect(modelsRequest({ ...catalog, default_model: "openai:gpt-image-2" }, {})).toBeNull();
    });
  });
});

describe("Connect", () => {
  it("offers ChatGPT and Claude, and signs in to Claude through the browser with a pasted final address", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    const dialog = await connect(user);
    const chatgpt = await within(dialog).findByRole("radio", { name: /ChatGPT/ });
    expect(chatgpt).toBeChecked();
    expect(within(dialog).getAllByRole("radio")).toHaveLength(2);
    expect(within(dialog).queryByLabelText("Name")).toBeNull();
    await user.click(within(dialog).getByRole("radio", { name: /Claude/ }));
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/accounts/anthropic", body: { label: "Claude", join_pool: true } }));

    const link = await screen.findByRole("link", { name: "Open sign-in page" }, POLL);
    expect(link).toHaveAttribute("href", "https://auth.example.test/authorize?tx=tx-2");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(screen.getByText(/Never paste a password or an access token/)).toBeInTheDocument();
    expect(daemon.state.pools.anthropic).toBeUndefined();

    await user.type(screen.getByLabelText(/^Paste the code or the final page address/), PASTED);
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/transactions/tx-2/input", body: { code: PASTED } }));
    expect(await screen.findByText("Connected. Claude is ready to use.", undefined, POLL)).toBeInTheDocument();
    expect(daemon.state.pools.anthropic).toEqual([ref("pi-oauth", "oa-1")]);
    await user.click(screen.getByRole("button", { name: "Done" }));

    const anthropic = await rowOf("Anthropic");
    await waitFor(() => expect(anthropic).toHaveTextContent("Claude"));
    expect(within(anthropic).getByRole("img", { name: "Connected" })).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("fixture-code-77");
  }, 20000);

  it("shows the code for a device sign-in picked from more options", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    const dialog = await connect(user);
    await within(dialog).findByRole("radio", { name: /ChatGPT/ });
    await user.click(within(dialog).getByRole("button", { name: "Show more options" }));
    const more = await screen.findByRole("dialog", { name: "More options" });
    expect(within(within(more).getByRole("list", { name: "More options" })).getAllByRole("button").map((button) => button.textContent)).toEqual(["GitHub Copilot", "Gemini", "Antigravity", "Use your own API key"]);
    await user.click(within(more).getByRole("button", { name: "GitHub Copilot" }));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/accounts/github-copilot", body: { label: "GitHub Copilot", join_pool: true } }));
    expect(await screen.findByLabelText("Sign-in code", undefined, POLL)).toHaveTextContent("ABCD-1234");
    expect(screen.queryByLabelText(/^Paste the code or the final page address/)).toBeNull();

    daemon.state.oauthOutcome["tx-2"] = "ready";
    expect(await screen.findByText("Connected. GitHub Copilot is ready to use.", undefined, POLL)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(await rowOf("GitHub Copilot")).toBeInTheDocument();
  }, 20000);

  it("explains a failed sign-in by its reason and starts a new one on Try again", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    const dialog = await connect(user);
    await within(dialog).findByRole("radio", { name: /ChatGPT/ });
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));
    await screen.findByRole("link", { name: "Open sign-in page" }, POLL);
    daemon.state.oauthOutcome["tx-2"] = "oauth_callback_busy";
    expect(await screen.findByText(/Another sign-in is using the sign-in port/, undefined, POLL)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Try again" }));
    const creates = daemon.calls.filter((call) => call.method === "POST" && call.path === "/v1/settings/oauth/accounts/openai-codex");
    expect(creates.map((call) => call.body)).toEqual([
      { label: "ChatGPT", join_pool: true },
      { label: "ChatGPT", join_pool: true },
    ]);
    await screen.findByRole("link", { name: "Open sign-in page" }, POLL);
    daemon.state.oauthOutcome["tx-4"] = "ready";
    expect(await screen.findByText("Connected. ChatGPT is ready to use.", undefined, POLL)).toBeInTheDocument();
    expect(daemon.state.oauthAccounts.map((account) => account.account_id)).toEqual(["oa-3"]);
  }, 20000);

  it("cancels the running sign-in on the daemon", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    const dialog = await connect(user);
    await user.click(await within(dialog).findByRole("radio", { name: /Claude/ }));
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));
    await screen.findByRole("link", { name: "Open sign-in page" }, POLL);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/transactions/tx-2/cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(daemon.state.oauthAccounts).toEqual([]);
  }, 20000);

  it("saves your own API key and puts it to work at once", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    const dialog = await connect(user);
    await within(dialog).findByRole("radio", { name: /ChatGPT/ });
    await user.click(within(dialog).getByRole("button", { name: "Show more options" }));
    await user.click(within(await screen.findByRole("dialog", { name: "More options" })).getByRole("button", { name: "Use your own API key" }));
    const form = await screen.findByRole("form", { name: "Use your own API key" });
    await chooseOption(user, within(form).getByRole("combobox", { name: "Provider" }), "OpenAI");
    await user.type(within(form).getByLabelText("API key"), "sk-test-4242");
    await user.click(within(form).getByRole("button", { name: "Save key" }));
    await waitFor(() => expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/keys/openai", body: { label: "OpenAI", key: "sk-test-4242" } })));
    await waitFor(() => expect(daemon.state.pools.openai).toEqual([ref("vault", daemon.state.keys[0]!.key_id)]));
    expect(await rowOf("OpenAI")).toHaveTextContent("API key");
    expect(document.body.textContent).not.toContain("sk-test-4242");
  });
});

describe("Provider rows", () => {
  it("disconnects only after confirmation", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ oauthAccounts: [oauthAccount("anthropic", "oa-7", "Claude")], pools: { anthropic: [ref("pi-oauth", "oa-7")] } });
    open(daemon.client);
    let menu = await rowMenu(user, "Anthropic");
    await user.click(within(menu).getByRole("menuitem", { name: "Disconnect Claude" }));
    let confirm = await screen.findByRole("dialog", { name: "Disconnect Claude?" });
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(daemon.calls.some((call) => call.method === "DELETE")).toBe(false);

    menu = await rowMenu(user, "Anthropic");
    await user.click(within(menu).getByRole("menuitem", { name: "Disconnect Claude" }));
    confirm = await screen.findByRole("dialog", { name: "Disconnect Claude?" });
    await user.click(within(confirm).getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "DELETE", path: "/v1/settings/oauth/accounts/anthropic/oa-7" })));
    await waitFor(() => expect(screen.queryByRole("group", { name: "Anthropic" })).toBeNull());
    expect(await screen.findByText("Claude disconnected.")).toBeInTheDocument();
  });

  it("removes a terminal sign-in through its own source and keeps a key the computer supplies", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({
      subscriptions: [subscriptionAccount("claude-code", "cc-1", "Work")],
      extra: { anthropic: [{ source: "env", account_id: "ANTHROPIC_API_KEY" }] },
    });
    open(daemon.client);
    const menu = await rowMenu(user, "Anthropic");
    expect(within(menu).getByRole("menuitem", { name: "Remove API key" })).toHaveAttribute("aria-disabled", "true");
    await user.click(within(menu).getByRole("menuitem", { name: "Disconnect Claude" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Disconnect Claude?" })).getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "DELETE", path: "/v1/settings/subscriptions/claude-code/cc-1" })));
  });

  it("signs a subscription in again from Reconnect", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ oauthAccounts: [oauthAccount("openai-codex", "oa-7", "ChatGPT", { auth_invalid: true })] });
    open(daemon.client);
    await user.click(within(await rowMenu(user, "OpenAI")).getByRole("menuitem", { name: "Sign in to ChatGPT again" }));
    await waitFor(() => expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/accounts/openai-codex/oa-7/relogin" })));
    expect(await screen.findByRole("dialog", { name: "Sign in again: ChatGPT" })).toBeInTheDocument();
    await screen.findByRole("link", { name: "Open sign-in page" }, POLL);
  }, 20000);

  it("replaces the key behind an API key row from Reconnect", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ keys: [keyInfo("openai", "k2", "OpenAI")], pools: { openai: [ref("vault", "k2")] } });
    open(daemon.client);
    await user.click(within(await rowMenu(user, "OpenAI")).getByRole("menuitem", { name: "Replace API key" }));
    const dialog = await screen.findByRole("dialog", { name: "Replace the key for OpenAI API key" });
    await user.type(within(dialog).getByLabelText("API key"), "sk-new-1111");
    await user.click(within(dialog).getByRole("button", { name: "Save key" }));
    await waitFor(() => expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "PUT", path: "/v1/settings/keys/openai/k2", body: { key: "sk-new-1111" } })));
  });
});
