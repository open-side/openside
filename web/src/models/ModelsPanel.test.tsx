import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { Client } from "../api/client";
import { ClientContext } from "../context";
import { chooseOption } from "../test/select";
import { ToastProvider } from "../ui";
import { ModelsPanel } from "./ModelsPanel";
import { keyInfo, modelsDaemon, SECRET, subscriptionAccount } from "./testDaemon";

afterEach(cleanup);

function open(client: Client) {
  return render(
    <ClientContext.Provider value={client}>
      <ToastProvider>
        <ModelsPanel />
      </ToastProvider>
    </ClientContext.Provider>,
  );
}

const provider = (name: string) => screen.findByRole("region", { name: `Provider ${name}` });
const section = (name: string) => within(screen.getByRole("region", { name: `Provider ${name}` }));
const rowOf = (name: string, label: string) => within(section(name).getByRole("list", { name: `Accounts for ${name}` })).getByText(label).closest("article") as HTMLElement;
/** The provider-level badge in the card header (accounts have their own). */
const status = (name: string) => screen.getByRole("region", { name: `Provider ${name}` }).querySelector(".section-list-actions .badge");
const ref = (source: string, id: string) => ({ source, account_id: id });

describe("ModelsPanel", () => {
  it("shows each provider with no account yet, how to add one and why its models are unavailable", async () => {
    open(modelsDaemon().client);
    const anthropic = await provider("Anthropic");
    expect(within(anthropic).getByText("Not connected")).toBeInTheDocument();
    expect(within(anthropic).getByText(/No account yet/)).toBeInTheDocument();
    expect(within(anthropic).getByRole("button", { name: "Add Anthropic API key" })).toBeInTheDocument();
    expect(within(anthropic).getByRole("button", { name: "Add Claude Code account" })).toBeInTheDocument();
    // The CLI-less providers offer keys only.
    expect(within(await provider("Google")).queryByRole("button", { name: /account$/ })).toBeNull();

    const models = within(anthropic).getByRole("list", { name: "Anthropic models" });
    expect(within(models).getByText("Claude Sonnet 5.5")).toBeInTheDocument();
    expect(within(models).getAllByText("Unavailable")).toHaveLength(2);
    expect(within(models).getAllByText(/No credential yet/)).toHaveLength(2);
    expect(screen.getByRole("status")).toHaveTextContent("No model is ready yet");
  });

  it("adds a first key into the rotation without ever rendering it back, and the provider becomes ready", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    await user.click(within(await provider("OpenAI")).getByRole("button", { name: "Add OpenAI API key" }));
    const field = screen.getByLabelText("API key");
    expect(field).toHaveAttribute("type", "password");
    expect(field).toHaveAttribute("autocomplete", "new-password");
    await user.type(screen.getByLabelText("Name"), "Work");
    await user.type(field, SECRET);
    await user.click(screen.getByRole("button", { name: "Save key" }));

    await waitFor(() => expect(status("OpenAI")).toHaveTextContent("Ready"));
    const row = rowOf("OpenAI", "Work");
    expect(within(row).getByLabelText("Key ending in 9f3a")).toHaveTextContent("9f3a");
    expect(daemon.state.received).toEqual([SECRET]);
    expect(daemon.calls.find((call) => call.method === "POST")).toMatchObject({ path: "/v1/settings/keys/openai", body: { label: "Work", key: SECRET } });
    // The first account joins the rotation on its own; later ones are offered the choice.
    expect(daemon.state.pools.openai).toEqual([ref("vault", "key-1")]);
    expect(document.body.textContent).not.toContain(SECRET);
    expect(Array.from(document.querySelectorAll("input")).every((input) => !input.value.includes(SECRET))).toBe(true);
  });

  it("explains a locked vault in the key dialog and keeps the dialog open without echoing the key", async () => {
    const user = userEvent.setup();
    open(modelsDaemon({ vaultLocked: true }).client);
    await user.click(within(await provider("Google")).getByRole("button", { name: "Add Google API key" }));
    await user.type(screen.getByLabelText("Name"), "Team");
    await user.type(screen.getByLabelText("API key"), SECRET);
    await user.click(screen.getByRole("button", { name: "Save key" }));
    expect(await screen.findByText(/vault is locked or not set up/)).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Add Google API key" })).toBeInTheDocument();
  });

  it("keeps a second key out of the rotation until chosen, then adds it to the end", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ keys: [keyInfo("anthropic", "k1", "Work", "w4Zq")], pools: { anthropic: [ref("vault", "k1")] } });
    open(daemon.client);
    await user.click(within(await provider("Anthropic")).getByRole("button", { name: "Add Anthropic API key" }));
    await user.type(screen.getByLabelText("Name"), "Spare");
    await user.type(screen.getByLabelText("API key"), "sk-spare-1234");
    await user.click(screen.getByLabelText("Use it in the rotation right away"));
    await user.click(screen.getByRole("button", { name: "Save key" }));
    await waitFor(() => expect(rowOf("Anthropic", "Spare")).toHaveTextContent("Not in rotation"));
    expect(daemon.state.pools.anthropic).toEqual([ref("vault", "k1")]);

    await chooseOption(user, section("Anthropic").getByLabelText("Add an account to the Anthropic rotation"), /Spare/);
    await waitFor(() => expect(daemon.state.pools.anthropic).toEqual([ref("vault", "k1"), ref("vault", "key-1")]));
    const order = section("Anthropic").getByRole("list", { name: "Anthropic rotation order" });
    expect(within(order).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["Work", "Spare"]);
  });

  it("reorders and removes accounts from the rotation", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({
      keys: [keyInfo("anthropic", "k1", "Work"), keyInfo("anthropic", "k2", "Personal", "abcd")],
      pools: { anthropic: [ref("vault", "k1"), ref("vault", "k2")] },
    });
    open(daemon.client);
    await provider("Anthropic");
    await user.click(section("Anthropic").getByRole("button", { name: "Move Work down" }));
    await waitFor(() => expect(daemon.state.pools.anthropic).toEqual([ref("vault", "k2"), ref("vault", "k1")]));
    await user.click(section("Anthropic").getByRole("button", { name: "Remove Personal from the rotation" }));
    await waitFor(() => expect(daemon.state.pools.anthropic).toEqual([ref("vault", "k1")]));
    await waitFor(() => expect(rowOf("Anthropic", "Personal")).toHaveTextContent("Not in rotation"));
  });

  it("says so when no account is in the rotation", async () => {
    const daemon = modelsDaemon({ keys: [keyInfo("anthropic", "k1", "Work")], pools: { anthropic: [] } });
    open(daemon.client);
    await provider("Anthropic");
    expect(section("Anthropic").getByText(/No account is in the rotation, so Anthropic cannot be used/)).toBeInTheDocument();
    expect(status("Anthropic")).toHaveTextContent("Not connected");
  });

  it("renames an account, changes its reserve and replaces its key without showing the new value", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ keys: [keyInfo("anthropic", "k1", "Work", "w4Zq")], pools: { anthropic: [ref("vault", "k1")] } });
    open(daemon.client);
    await provider("Anthropic");
    await user.click(section("Anthropic").getByRole("button", { name: "Manage Work" }));
    const dialog = screen.getByRole("dialog", { name: "Manage Work" });
    const name = within(dialog).getByLabelText("Name");
    await user.clear(name);
    await user.type(name, "Work (team)");
    const reserve = within(dialog).getByLabelText(/Keep in reserve/);
    await user.clear(reserve);
    await user.type(reserve, "25");
    await user.type(within(dialog).getByLabelText("Replace key (optional)"), SECRET);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    await waitFor(() => expect(daemon.state.keys[0]).toMatchObject({ label: "Work (team)", reserve_percent: 25, last_four: SECRET.slice(-4) }));
    expect(daemon.calls.filter((call) => call.method === "PUT")).toEqual([expect.objectContaining({ path: "/v1/settings/keys/anthropic/k1", body: { key: SECRET, label: "Work (team)" } })]);
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "PATCH", path: "/v1/settings/keys/anthropic/k1", body: { reserve_percent: 25 } }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.body.textContent).not.toContain(SECRET);
  });

  it("removes an account only after confirmation", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ keys: [keyInfo("anthropic", "k1", "Work")], pools: { anthropic: [ref("vault", "k1")] } });
    open(daemon.client);
    const anthropic = await provider("Anthropic");
    expect(status("Anthropic")).toHaveTextContent("Ready");
    await user.click(within(anthropic).getByRole("button", { name: "Manage Work" }));
    await user.click(within(screen.getByRole("dialog", { name: "Manage Work" })).getByRole("button", { name: "Remove" }));
    expect(daemon.calls.some((call) => call.method === "DELETE")).toBe(false);
    expect(screen.getByRole("dialog")).toHaveTextContent("does not revoke the key with the provider");
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(daemon.state.keys).toEqual([]));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "DELETE", path: "/v1/settings/keys/anthropic/k1" }));
    await waitFor(() => expect(status("Anthropic")).toHaveTextContent("Not connected"));
  });

  it("switches one account off without touching the others", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({
      keys: [keyInfo("anthropic", "k1", "Work"), keyInfo("anthropic", "k2", "Personal")],
      pools: { anthropic: [ref("vault", "k1"), ref("vault", "k2")] },
    });
    open(daemon.client);
    await provider("Anthropic");
    const toggle = section("Anthropic").getByRole("switch", { name: "Use Work" });
    await user.click(toggle);
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "PATCH", path: "/v1/settings/keys/anthropic/k1", body: { enabled: false } }));
    expect(section("Anthropic").getByRole("switch", { name: "Use Personal" })).toBeChecked();
  });

  it("adds a subscription account, shows its own sign-in command and notices the sign-in", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    await user.click(within(await provider("Anthropic")).getByRole("button", { name: "Add Claude Code account" }));
    await user.type(screen.getByLabelText("Name"), "Team");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    const dialog = await screen.findByRole("dialog", { name: "Add Claude Code account" });
    expect(await within(dialog).findByText(/claude auth login/)).toHaveTextContent("CLAUDE_CONFIG_DIR=");
    expect(within(dialog).getByText("Waiting for the sign-in…")).toBeInTheDocument();
    expect(daemon.state.pools.anthropic).toEqual([ref("claude-code", "acct-1")]);
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/subscriptions/claude-code", body: { label: "Team" } }));

    daemon.state.signInLands.push("acct-1");
    expect(await within(dialog).findByText("Signed in. This account is ready to use.", undefined, { timeout: 6000 })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(status("Anthropic")).toHaveTextContent("Ready"));
  }, 15000);

  it("offers a sign-in for an account that is not signed in yet", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({
      subscriptions: [subscriptionAccount("claude-code", "a1", "Team", { logged_in: false, available: false, reason: "Not signed in." })],
      pools: { anthropic: [ref("claude-code", "a1")] },
    });
    open(daemon.client);
    await provider("Anthropic");
    const row = rowOf("Anthropic", "Team");
    expect(within(row).getByText("Not signed in", { selector: "span" })).toBeInTheDocument();
    await user.click(within(row).getByRole("button", { name: "Sign in Team" }));
    expect(within(screen.getByRole("dialog", { name: "Sign in: Team" })).getByText(/CLAUDE_CONFIG_DIR=.* claude auth login/)).toBeInTheDocument();
  });

  it("treats the terminal's own sign-in as an account that can be renamed but not rewritten", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ subscriptions: [subscriptionAccount("codex-cli", "default", "Terminal", { managed: false })], pools: { openai: [ref("codex-cli", "default")] } });
    open(daemon.client);
    await provider("OpenAI");
    expect(rowOf("OpenAI", "Terminal")).toHaveTextContent("your terminal's sign-in");
    await user.click(section("OpenAI").getByRole("button", { name: "Manage Terminal" }));
    const dialog = screen.getByRole("dialog", { name: "Manage Terminal" });
    expect(within(dialog).getByText(/only reads it and never changes it/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText("Replace key (optional)")).toBeNull();
    expect(within(dialog).getByRole("button", { name: "Stop using" })).toBeInTheDocument();
  });

  it("sets the reasoning model from available models only and lists fallbacks in order", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ keys: [keyInfo("openai", "k1", "Main", "1111")], pools: { openai: [ref("vault", "k1")] } });
    open(daemon.client);
    const select = await screen.findByLabelText("Reasoning model");
    await user.click(select);
    const sonnet = await screen.findByRole("option", { name: /Claude Sonnet 5.5/ });
    expect(sonnet).toHaveAttribute("aria-disabled", "true");
    expect(sonnet).toHaveTextContent("(unavailable)");
    await user.click(screen.getByRole("option", { name: /^GPT-6.1 Sol/ }));
    await waitFor(() => expect(daemon.state.defaultModel).toBe("openai:gpt-6.1-sol"));
    expect(daemon.calls).toContainEqual(
      expect.objectContaining({ method: "PUT", path: "/v1/settings/models", body: { default_model: "openai:gpt-6.1-sol", fast_model: null, fallback_models: [] } }),
    );

    await chooseOption(user, await screen.findByLabelText("Add a fallback"), /GPT-6 Luna/);
    const chips = await screen.findByRole("group", { name: "Fallback models" });
    expect(within(chips).getByText("GPT-6 Luna")).toBeInTheDocument();
    expect(within(chips).queryByText("openai:gpt-6-luna")).toBeNull();
    await user.click(within(chips).getByRole("button", { name: "Remove fallback GPT-6 Luna" }));
    await waitFor(() => expect(daemon.state.fallbacks).toEqual([]));
  });

  it("offers the fast model as the reasoning model or any available one, recommended first, and keeps it when other settings change", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({
      keys: [keyInfo("openai", "k1", "Main", "1111")],
      pools: { openai: [ref("vault", "k1")] },
      defaultModel: "openai:gpt-6.1-sol",
    });
    open(daemon.client);
    const fast = await screen.findByLabelText("Fast model");
    expect(fast).toHaveTextContent("Same as reasoning model");
    expect(screen.getByText("Answers quickly and hands bigger work to the reasoning model.")).toBeInTheDocument();
    expect(screen.getByText("Does the thinking for tasks and teammates.")).toBeInTheDocument();

    await user.click(fast);
    const listbox = await screen.findByRole("listbox");
    const names = within(listbox).getAllByRole("option").map((option) => option.textContent);
    // The same-as option first, then the recommended fast models (labelled by provider), then the rest by provider.
    expect(names.slice(0, 3)).toEqual(["Same as reasoning model", expect.stringContaining("Claude Haiku 5.5"), expect.stringContaining("GPT-6 Luna")]);
    expect(within(listbox).getByText("Recommended")).toBeInTheDocument();
    expect(within(listbox).getAllByRole("option", { name: /GPT-6 Luna/ })).toHaveLength(1);
    await user.click(within(listbox).getByRole("option", { name: /GPT-6 Luna/ }));
    await waitFor(() => expect(daemon.state.fastModel).toBe("openai:gpt-6-luna"));
    expect(daemon.calls).toContainEqual(
      expect.objectContaining({ method: "PUT", path: "/v1/settings/models", body: { default_model: "openai:gpt-6.1-sol", fast_model: "openai:gpt-6-luna", fallback_models: [] } }),
    );

    // A fallback change re-sends the fast model rather than clearing it.
    await chooseOption(user, await screen.findByLabelText("Add a fallback"), /GPT-6 Luna/);
    await waitFor(() => expect(daemon.state.fallbacks).toEqual(["openai:gpt-6-luna"]));
    expect(daemon.state.fastModel).toBe("openai:gpt-6-luna");

    await chooseOption(user, screen.getByLabelText("Fast model"), "Same as reasoning model");
    await waitFor(() => expect(daemon.state.fastModel).toBeNull());
  });

  it("lists the recommended reasoning models first in the reasoning select", async () => {
    const user = userEvent.setup();
    open(modelsDaemon({ keys: [keyInfo("openai", "k1", "Main", "1111")], pools: { openai: [ref("vault", "k1")] } }).client);
    await user.click(await screen.findByLabelText("Reasoning model"));
    const listbox = await screen.findByRole("listbox");
    const names = within(listbox).getAllByRole("option").map((option) => option.textContent);
    expect(names.slice(0, 2)).toEqual([expect.stringContaining("Claude Sonnet 5.5"), expect.stringContaining("GPT-6.1 Sol")]);
    expect(within(listbox).getByText("Recommended")).toBeInTheDocument();
    // Each option carries its tier as a tag and the curated strengths as a muted line.
    const sonnet = within(listbox).getByRole("option", { name: /Claude Sonnet 5.5/ });
    expect(sonnet).toHaveTextContent("Reasoning");
    expect(sonnet).toHaveTextContent("Anthropic · Multi-step research, coding");
    expect(within(listbox).getByRole("option", { name: /GPT-6 Astra/ })).toHaveTextContent("Frontier");
    expect(within(listbox).getByRole("option", { name: /Gemini 2.5 Pro/ })).toHaveTextContent("Not in current catalog");
  });

  it("shows broker accounts with usage and rate-limit blocks, and the credential sources", async () => {
    const daemon = modelsDaemon({
      pools: { anthropic: [ref("omp-broker", "b1")] },
      extra: {
        anthropic: [{ source: "omp-broker", account_id: "b1", label: "w***@example.com", usage_windows: [{ name: "5h", used_percent: 82.4, reset_at_ms: null }], blocked_until_ms: Date.now() + 3_600_000 }],
      },
      accounts: { sources: [{ name: "env", kind: "env", available: false, reason: "No provider key in the environment.", accounts: [] }], cached_for_ms: 0, truncated: false },
    });
    open(daemon.client);
    const anthropic = await provider("Anthropic");
    expect(rowOf("Anthropic", "w***@example.com")).toBeInTheDocument();
    const meter = within(anthropic).getByRole("meter", { name: "5h remaining" });
    expect(Number(meter.getAttribute("aria-valuenow"))).toBeCloseTo(17.6);
    expect(within(anthropic).getByText(/^18% left/)).toBeInTheDocument();
    expect(within(anthropic).getByText(/Rate limited until/)).toBeInTheDocument();
    // Accounts configured outside the UI have no key or sign-in controls.
    expect(within(rowOf("Anthropic", "w***@example.com")).queryByRole("button")).toBeNull();
    const sources = screen.getByRole("list", { name: "Credential sources" });
    expect(within(sources).getByText("No provider key in the environment.")).toBeInTheDocument();
  });

  it("gives a limited device status only: no account, rotation or model controls", async () => {
    const daemon = modelsDaemon({
      admin: false,
      keys: [keyInfo("anthropic", "k1", "Work", SECRET.slice(-4))],
      pools: { anthropic: [ref("vault", "k1")] },
    });
    open(daemon.client);
    const anthropic = await provider("Anthropic");
    await waitFor(() => expect(screen.getByRole("note")).toHaveTextContent("cannot change them"));
    expect(rowOf("Anthropic", "Work")).toBeInTheDocument();
    expect(status("Anthropic")).toHaveTextContent("Ready");
    expect(within(anthropic).getByRole("list", { name: "Anthropic rotation order" })).toBeInTheDocument();
    // Key metadata is Admin-only, so no suffix reaches this device.
    expect(document.querySelector(".key-mask")).toBeNull();
    expect(screen.queryByRole("button", { name: /API key|account|Manage|Move|Remove/ })).toBeNull();
    expect(screen.queryByRole("switch")).toBeNull();
    expect(screen.queryByLabelText("Fast model")).toBeNull();
    expect(screen.queryByLabelText("Reasoning model")).toBeNull();
    expect(screen.getByText("Same as reasoning model")).toBeInTheDocument();
    expect(daemon.calls.filter((call) => call.method !== "GET")).toEqual([]);
  });
});
