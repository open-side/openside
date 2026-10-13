import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary, ChannelAccount, ChannelAccountOptions } from "../api/types";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { chooseOption } from "../test/select";
import { ToastProvider } from "../ui";
import { ChannelsPanel } from "./ChannelsPanel";

afterEach(cleanup);

const TOKEN = "xoxb-SECRET-token-9f3a";
const SIGNING = "signing-SECRET-4b7c";

const NO_OPTIONS: ChannelAccountOptions = { allowed_chats: [], allowed_users: [], allowed_workspaces: [], allowed_channels: [], allowed_guilds: [], api_base: null, listen: null, poll_timeout_secs: null, public_key: null };

const AGENTS = [
  { id: "chief", kind: "chief", name: "openside", initials: "o", color: "#2f6fed", status: "active", persona: { role: "Your assistant" } },
  { id: "mina", kind: "teammate", name: "Mina", initials: "M", color: "#a34dff", status: "active", persona: { role: "Researcher" } },
  { id: "old", kind: "teammate", name: "Otto", initials: "O", color: "#888888", status: "retired", persona: { role: "Retired" } },
] as unknown as AgentSummary[];

function account(patch: Partial<ChannelAccount> & Pick<ChannelAccount, "id" | "kind" | "label">): ChannelAccount {
  return {
    enabled: true,
    agent_id: null,
    effective_agent_id: "chief",
    options: { ...NO_OPTIONS, allowed_channels: ["C0123"], allowed_chats: ["42"] },
    secret_present: true,
    credential_source: "vault",
    status: "connected",
    restart_required: false,
    receiver_address: null,
    webhook_url: null,
    ...patch,
  };
}

const PUBLIC_URL = "https://box.example.ts.net/v1/hooks/slack/work";

function seed(): ChannelAccount[] {
  return [
    account({ id: "personal", kind: "telegram", label: "Personal bot" }),
    account({ id: "work", kind: "slack", label: "Work Slack", agent_id: "mina", effective_agent_id: "mina", webhook_url: PUBLIC_URL, options: { ...NO_OPTIONS, api_base: "https://slack.example/api", allowed_workspaces: ["T01"], allowed_channels: ["C01"] } }),
    account({ id: "team", kind: "slack", label: "Team Slack", enabled: false, effective_agent_id: null, status: "disabled", webhook_url: "http://127.0.0.1:41233/v1/hooks/slack/team" }),
    account({ id: "legacy-discord", kind: "discord", label: "Community bot", credential_source: "legacy", status: "restart_required", restart_required: true, webhook_url: null, options: { ...NO_OPTIONS, allowed_guilds: ["g1"], public_key: "ab".repeat(32) } }),
  ];
}

interface Handlers {
  accounts: ChannelAccount[];
  fail?: (call: Call) => Response | undefined;
}

function open(initial: Partial<Handlers> = {}) {
  const state: Handlers = { accounts: seed(), ...initial };
  const onRemote = vi.fn();
  const fake = fakeClient((call) => {
    const refused = state.fail?.(call);
    if (refused) return refused;
    const path = call.path;
    if (call.method === "GET" && path === "/v1/settings/channels") return json(200, { accounts: state.accounts });
    const match = /^\/v1\/settings\/channels\/(\w+)\/accounts(?:\/([\w-]+))?(\/test)?$/.exec(path);
    if (match) {
      const [, kind, id, test] = match;
      const body = call.body as Record<string, unknown>;
      if (call.method === "POST" && !id) {
        const created = account({ id: String(body.id), kind: kind!, label: String(body.label), enabled: Boolean(body.enabled), agent_id: (body.agent_id as string | null) ?? null, options: body.options as ChannelAccountOptions, effective_agent_id: body.enabled ? "chief" : null, status: body.enabled ? "connected" : "disabled" });
        state.accounts = [...state.accounts, created];
        return json(201, created);
      }
      if (call.method === "PATCH" && id) {
        state.accounts = state.accounts.map((item) => {
          if (item.id !== id) return item;
          const enabled = typeof body.enabled === "boolean" ? body.enabled : item.enabled;
          return { ...item, ...(body as Partial<ChannelAccount>), enabled, status: enabled ? "connected" : "disabled", credential_source: body.credentials ? "vault" : item.credential_source };
        });
        return json(200, state.accounts.find((item) => item.id === id));
      }
      if (call.method === "DELETE" && id) {
        state.accounts = state.accounts.filter((item) => item.id !== id);
        return new Response(null, { status: 204 });
      }
      if (call.method === "POST" && test) return json(200, { account_id: id, kind, ok: true, status: "connected", code: "ok", restart_required: false });
    }
    return json(404, { code: "not_found", message: "nope" });
  });
  render(
    <ClientContext.Provider value={fake.client}>
      <AgentsContext.Provider value={{ agents: AGENTS, error: null, offline: false, refresh: () => undefined }}>
        <ToastProvider>
          <ChannelsPanel onRemote={onRemote} />
        </ToastProvider>
      </AgentsContext.Provider>
    </ClientContext.Provider>,
  );
  return { ...fake, state, onRemote };
}

const group = (name: string) => screen.findByRole("region", { name });
const row = async (service: string, label: string) => within(await group(service)).getByText(label).closest("article") as HTMLElement;
const writes = (calls: Call[]) => calls.filter((call) => call.method !== "GET");

describe("ChannelsPanel", () => {
  it("lists every account under its service with status, the teammate it answers as and the callback address", async () => {
    open();
    const slack = await group("Slack");
    expect(within(slack).getByText("Work Slack")).toBeInTheDocument();
    expect(within(slack).getByText("Team Slack")).toBeInTheDocument();
    const work = await row("Slack", "Work Slack");
    expect(within(work).getByText("Connected")).toBeInTheDocument();
    expect(within(work).getByText("Mina")).toBeInTheDocument();
    expect(within(work).getByText(PUBLIC_URL)).toBeInTheDocument();
    expect(within(work).getByRole("button", { name: "Copy callback address for Work Slack" })).toBeInTheDocument();

    const team = await row("Slack", "Team Slack");
    expect(within(team).getByText("Off")).toBeInTheDocument();
    expect(within(team).getByText(/Will answer as/)).toHaveTextContent("Will answer as openside once it connects");

    // Telegram is polled, so it has no callback address.
    const telegram = await row("Telegram", "Personal bot");
    expect(within(telegram).getByText("Connected")).toBeInTheDocument();
    expect(within(telegram).queryByText(/Request URL/)).toBeNull();
    expect(within(await group("Discord")).getByText("Restart needed")).toBeInTheDocument();
    expect(screen.getByText(/openside to restart/)).toBeInTheDocument();
  });

  it("explains that a callback address on this computer is unreachable and offers remote access", async () => {
    const user = userEvent.setup();
    const { onRemote } = open();
    const team = await row("Slack", "Team Slack");
    expect(within(team).getByText(/only works on this computer/)).toBeInTheDocument();
    await user.click(within(team).getByRole("button", { name: "Set up remote access" }));
    expect(onRemote).toHaveBeenCalledOnce();
    expect(within(await row("Slack", "Work Slack")).queryByText(/only works on this computer/)).toBeNull();
  });

  it("copies the callback address", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("isSecureContext", true);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    open();
    await user.click(within(await row("Slack", "Work Slack")).getByRole("button", { name: "Copy callback address for Work Slack" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(PUBLIC_URL));
    expect(await screen.findByText("Address copied.")).toBeInTheDocument();
    vi.unstubAllGlobals();
  });

  it("adds a second Slack account without touching the first, and never keeps the credentials", async () => {
    const user = userEvent.setup();
    const daemon = open();
    await user.click(within(await group("Slack")).getByRole("button", { name: "Add account" }));
    const dialog = await screen.findByRole("dialog", { name: "Add Slack account" });
    await user.type(within(dialog).getByLabelText("Name"), "Support Slack");
    // The id follows the name until the person edits it.
    expect(within(dialog).getByLabelText("Account id")).toHaveValue("support-slack");
    await user.type(within(dialog).getByLabelText("Allowed workspaces"), "T09");
    await user.type(within(dialog).getByLabelText("Allowed channels"), "C09, C10");
    const token = within(dialog).getByLabelText("Bot token");
    expect(token).toHaveAttribute("type", "password");
    expect(token).toHaveAttribute("autocomplete", "new-password");
    await user.type(token, TOKEN);
    await user.type(within(dialog).getByLabelText("Signing secret"), SIGNING);
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Answers as" }), /Mina/);
    await user.click(within(dialog).getByRole("button", { name: "Add account" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add Slack account" })).toBeNull());
    const [post] = writes(daemon.calls);
    expect(post).toMatchObject({
      method: "POST",
      path: "/v1/settings/channels/slack/accounts",
      body: {
        id: "support-slack",
        label: "Support Slack",
        enabled: true,
        agent_id: "mina",
        options: { allowed_workspaces: ["T09"], allowed_channels: ["C09", "C10"], allowed_users: [], api_base: null },
        credentials: { token: TOKEN, signing_secret: SIGNING },
      },
    });
    const slack = await group("Slack");
    expect(within(slack).getByText("Support Slack")).toBeInTheDocument();
    expect(within(slack).getByText("Work Slack")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(TOKEN);
    expect(document.body.textContent).not.toContain(SIGNING);
    expect(Array.from(document.querySelectorAll("input")).every((input) => !input.value.includes("SECRET"))).toBe(true);
  });

  it("empties the credential fields before the request and keeps them empty when it is refused", async () => {
    const user = userEvent.setup();
    const daemon = open({ fail: (call) => (call.method === "POST" ? json(409, { code: "vault_unavailable", message: "raw upstream text" }) : undefined) });
    await user.click(within(await group("Telegram")).getByRole("button", { name: "Add account" }));
    const dialog = await screen.findByRole("dialog", { name: "Add Telegram account" });
    await user.type(within(dialog).getByLabelText("Name"), "Second bot");
    await user.type(within(dialog).getByLabelText("Allowed chats"), "7");
    await user.type(within(dialog).getByLabelText("Bot token"), "123456:TELEGRAM-SECRET");
    await user.click(within(dialog).getByRole("button", { name: "Add account" }));

    expect(await within(dialog).findByText(/vault is locked or not set up/)).toBeInTheDocument();
    expect(within(dialog).queryByText(/raw upstream text/)).toBeNull();
    expect(within(dialog).getByLabelText("Bot token")).toHaveValue("");
    expect(JSON.stringify(daemon.calls.map((call) => call.body))).toContain("TELEGRAM-SECRET");
  });

  it("asks for the Discord public key and both Slack secrets before sending anything", async () => {
    const user = userEvent.setup();
    const daemon = open();
    await user.click(within(await group("Slack")).getByRole("button", { name: "Add account" }));
    let dialog = await screen.findByRole("dialog", { name: "Add Slack account" });
    await user.type(within(dialog).getByLabelText("Name"), "Half");
    await user.type(within(dialog).getByLabelText("Allowed channels"), "C1");
    await user.type(within(dialog).getByLabelText("Bot token"), TOKEN);
    await user.click(within(dialog).getByRole("button", { name: "Add account" }));
    expect(await within(dialog).findByText("Paste both the bot token and the signing secret.")).toBeInTheDocument();
    // Nothing was cleared or sent: the person can finish the form.
    expect(within(dialog).getByLabelText("Bot token")).toHaveValue(TOKEN);
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add Slack account" })).toBeNull());

    await user.click(within(await group("Discord")).getByRole("button", { name: "Add account" }));
    dialog = await screen.findByRole("dialog", { name: "Add Discord account" });
    await user.type(within(dialog).getByLabelText("Name"), "Guild bot");
    await user.type(within(dialog).getByLabelText("Allowed servers"), "g2");
    await user.type(within(dialog).getByLabelText("Bot token"), "discord-token");
    await user.click(within(dialog).getByRole("button", { name: "Add account" }));
    expect(await within(dialog).findByText(/application's public key/)).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([]);
  });

  it("refuses to turn an account on without an allow-list, but saves it off", async () => {
    const user = userEvent.setup();
    const daemon = open();
    await user.click(within(await group("Telegram")).getByRole("button", { name: "Add account" }));
    const dialog = await screen.findByRole("dialog", { name: "Add Telegram account" });
    await user.type(within(dialog).getByLabelText("Name"), "Quiet");
    await user.type(within(dialog).getByLabelText("Bot token"), "123:abc");
    await user.click(within(dialog).getByRole("button", { name: "Add account" }));
    expect(await within(dialog).findByText(/Add at least one allowed chat/)).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([]);
    await user.click(within(dialog).getByRole("switch", { name: "Turn on Quiet" }));
    await user.type(within(dialog).getByLabelText("Bot token"), "123:abc");
    await user.click(within(dialog).getByRole("button", { name: "Add account" }));
    await waitFor(() => expect(writes(daemon.calls)[0]?.body).toMatchObject({ enabled: false, id: "quiet" }));
  });

  it("edits only what changed and keeps file-only options and the stored credentials", async () => {
    const user = userEvent.setup();
    const daemon = open();
    await user.click(within(await row("Slack", "Work Slack")).getByRole("button", { name: "Edit Work Slack" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit Work Slack" });
    // The existing credentials are never shown; replacing them is explicit.
    expect(within(dialog).queryByLabelText("Bot token")).toBeNull();
    expect(within(dialog).getByText(PUBLIC_URL)).toBeInTheDocument();
    await user.clear(within(dialog).getByLabelText("Name"));
    await user.type(within(dialog).getByLabelText("Name"), "Work workspace");
    await user.type(within(dialog).getByLabelText("Allowed channels"), ", C02");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit Work Slack" })).toBeNull());
    expect(writes(daemon.calls)).toEqual([
      expect.objectContaining({
        method: "PATCH",
        path: "/v1/settings/channels/slack/accounts/work",
        body: { label: "Work workspace", options: { ...NO_OPTIONS, api_base: "https://slack.example/api", allowed_workspaces: ["T01"], allowed_channels: ["C01", "C02"] } },
      }),
    ]);
    expect(await group("Slack").then((slack) => within(slack).getByText("Work workspace"))).toBeInTheDocument();
  });

  it("clears the teammate assignment with an empty agent id and replaces credentials only when asked", async () => {
    const user = userEvent.setup();
    const daemon = open();
    await user.click(within(await row("Slack", "Work Slack")).getByRole("button", { name: "Edit Work Slack" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit Work Slack" });
    await chooseOption(user, within(dialog).getByRole("combobox", { name: "Answers as" }), /openside/);
    await user.click(within(dialog).getByRole("button", { name: "Replace credentials" }));
    await user.type(within(dialog).getByLabelText("Bot token"), TOKEN);
    await user.type(within(dialog).getByLabelText("Signing secret"), SIGNING);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(writes(daemon.calls)).toHaveLength(1));
    expect(writes(daemon.calls)[0]?.body).toEqual({ agent_id: "", credentials: { token: TOKEN, signing_secret: SIGNING } });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit Work Slack" })).toBeNull());
    expect(document.body.textContent).not.toContain(TOKEN);
  });

  it("turns one account on or off from its row", async () => {
    const user = userEvent.setup();
    const daemon = open();
    await user.click(within(await row("Slack", "Work Slack")).getByRole("switch", { name: "Turn on Work Slack" }));
    await waitFor(() => expect(writes(daemon.calls)).toHaveLength(1));
    expect(writes(daemon.calls)[0]).toMatchObject({ method: "PATCH", path: "/v1/settings/channels/slack/accounts/work", body: { enabled: false } });
    await waitFor(() => expect(screen.getAllByText("Off")).toHaveLength(2));
  });

  it("shows a refusal from the daemon in the client's words", async () => {
    const user = userEvent.setup();
    open({ fail: (call) => (call.method === "PATCH" ? json(409, { code: "invalid_channel_account", message: "daemon text" }) : undefined) });
    await user.click(within(await row("Slack", "Team Slack")).getByRole("switch", { name: "Turn on Team Slack" }));
    expect(await screen.findByText(/at least one allowed chat, channel or user/)).toBeInTheDocument();
    expect(screen.queryByText("daemon text")).toBeNull();
  });

  it("tests the connection from the row menu and in the dialog", async () => {
    const user = userEvent.setup();
    const daemon = open();
    const personal = await row("Telegram", "Personal bot");
    await user.click(within(personal).getByRole("button", { name: "Options for Personal bot" }));
    await user.click(await screen.findByRole("menuitem", { name: "Test connection" }));
    expect(await within(personal).findByText("The service accepted this account's credentials.")).toBeInTheDocument();
    expect(daemon.calls.find((call) => call.path.endsWith("/test"))).toMatchObject({ method: "POST", path: "/v1/settings/channels/telegram/accounts/personal/test" });

    await user.click(within(personal).getByRole("button", { name: "Edit Personal bot" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit Personal bot" });
    await user.click(within(dialog).getByRole("button", { name: "Test connection" }));
    expect(await within(dialog).findByRole("status")).toHaveTextContent("The service accepted this account's credentials.");
  });

  it("reports a refused credential in words and offers to reconnect", async () => {
    const user = userEvent.setup();
    const expired = account({ id: "old", kind: "telegram", label: "Old bot", status: "expired", effective_agent_id: null });
    const daemon = open({ accounts: [expired], fail: (call) => (call.path.endsWith("/test") ? json(200, { account_id: "old", kind: "telegram", ok: false, status: "expired", code: "channel_credentials_expired", restart_required: false }) : undefined) });
    const old = await row("Telegram", "Old bot");
    expect(within(old).getByText("Sign in again")).toBeInTheDocument();
    await user.click(within(old).getByRole("button", { name: "Options for Old bot" }));
    await user.click(await screen.findByRole("menuitem", { name: "Test connection" }));
    expect(await within(old).findByText("The service refused these credentials. Replace them to reconnect.")).toBeInTheDocument();
    await user.click(within(old).getByRole("button", { name: "Reconnect Old bot" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit Old bot" });
    // An expired account opens straight on the credential fields.
    expect(within(dialog).getByLabelText("Bot token")).toBeInTheDocument();
    expect(daemon.calls.filter((call) => call.method !== "GET" && !call.path.endsWith("/test"))).toEqual([]);
  });

  it("removes one account after confirming and leaves the rest", async () => {
    const user = userEvent.setup();
    const daemon = open();
    const work = await row("Slack", "Work Slack");
    await user.click(within(work).getByRole("button", { name: "Options for Work Slack" }));
    await user.click(await screen.findByRole("menuitem", { name: "Remove" }));
    const confirm = await screen.findByRole("dialog", { name: "Remove Work Slack?" });
    expect(within(confirm).getByText(/Only this Slack account stops/)).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([]);
    await user.click(within(confirm).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(writes(daemon.calls)).toHaveLength(1));
    expect(writes(daemon.calls)[0]).toMatchObject({ method: "DELETE", path: "/v1/settings/channels/slack/accounts/work" });
    await waitFor(() => expect(screen.queryByText("Work Slack")).toBeNull());
    expect(screen.getByText("Team Slack")).toBeInTheDocument();
    expect(screen.getByText("Personal bot")).toBeInTheDocument();
  });

  it("points legacy accounts at the vault and migrates them in place", async () => {
    const user = userEvent.setup();
    const daemon = open();
    const legacy = await row("Discord", "Community bot");
    expect(within(legacy).getByText(/still reads its credentials from config.toml/)).toBeInTheDocument();
    await user.click(within(legacy).getByRole("button", { name: "Move to vault" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit Community bot" });
    expect(within(dialog).getByLabelText("Application public key")).toHaveValue("ab".repeat(32));
    await user.type(within(dialog).getByLabelText("Bot token"), "discord-token");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(writes(daemon.calls)).toHaveLength(1));
    expect(writes(daemon.calls)[0]).toMatchObject({ method: "PATCH", path: "/v1/settings/channels/discord/accounts/legacy-discord", body: { credentials: { token: "discord-token", signing_secret: null } } });
  });

  it("shows an empty service in one line and offers to add the first account", async () => {
    open({ accounts: [] });
    const discord = await group("Discord");
    expect(within(discord).getByText("No Discord accounts yet.")).toBeInTheDocument();
    expect(within(discord).getByRole("button", { name: "Add account" })).toBeInTheDocument();
  });

  it("is read only on a device without the admin scope", async () => {
    localStorage.setItem("openside.scopes", JSON.stringify(["read"]));
    open();
    const work = await row("Slack", "Work Slack");
    expect(screen.getByText(/can see the accounts but cannot change them/)).toBeInTheDocument();
    expect(within(work).getByText("Connected")).toBeInTheDocument();
    expect(within(work).queryByRole("switch")).toBeNull();
    expect(within(work).queryByRole("button", { name: /Edit/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add account" })).toBeNull();
    // The callback address stays visible so it can be copied from this device.
    expect(within(work).getByText(PUBLIC_URL)).toBeInTheDocument();
  });
});
