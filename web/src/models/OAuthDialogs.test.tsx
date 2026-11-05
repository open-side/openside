import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { Client } from "../api/client";
import { ClientContext } from "../context";
import { ToastProvider } from "../ui";
import { ModelsPanel } from "./ModelsPanel";
import { keyInfo, modelsDaemon, oauthAccount } from "./testDaemon";

afterEach(cleanup);

const POLL = { timeout: 5000 };
const PASTED = "https://localhost:53692/callback?code=fixture-code-77&state=s1";
const ref = (source: string, id: string) => ({ source, account_id: id });

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

async function connect(user: ReturnType<typeof userEvent.setup>, choice: RegExp) {
  await user.click(screen.getByRole("button", { name: "Connect account" }));
  const dialog = await screen.findByRole("dialog", { name: "Connect an account" });
  await user.click(await within(dialog).findByRole("radio", { name: choice }));
  return dialog;
}

describe("Connect account", () => {
  it("signs in through the browser with a pasted final address, then lists the account in its rotation", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    await provider("Anthropic");
    const dialog = await connect(user, /Claude \(Pro\/Max\)/);
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Claude (Pro/Max)");
    // The provider's first account always joins its rotation.
    expect(within(dialog).queryByLabelText("Use it in the rotation right away")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Continue" }));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/accounts/anthropic", body: { label: "Claude (Pro/Max)", join_pool: true } }));

    const link = await screen.findByRole("link", { name: "Open sign-in page" }, POLL);
    expect(link).toHaveAttribute("href", "https://auth.example.test/authorize?tx=tx-2");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(screen.getByRole("button", { name: "Copy sign-in link" })).toBeInTheDocument();
    expect(screen.getByText(/Never paste a password or an access token/)).toBeInTheDocument();
    expect(screen.getByText(/Expires in/)).toBeInTheDocument();
    expect(daemon.state.pools.anthropic).toBeUndefined();

    const field = screen.getByLabelText(/^Paste the code or the final page address/);
    await user.type(field, PASTED);
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/transactions/tx-2/input", body: { code: PASTED } }));
    expect(await screen.findByText("Connected. Claude (Pro/Max) is ready to use.", undefined, POLL)).toBeInTheDocument();
    expect(daemon.state.pools.anthropic).toEqual([ref("pi-oauth", "oa-1")]);
    await user.click(screen.getByRole("button", { name: "Done" }));

    await waitFor(() => expect(rowOf("Anthropic", "Claude (Pro/Max)")).toHaveTextContent("Anthropic sign-in"));
    expect(within(rowOf("Anthropic", "Claude (Pro/Max)")).getByText("Ready")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("fixture-code-77");
    expect(Array.from(document.querySelectorAll("input")).every((input) => !input.value.includes("fixture-code-77"))).toBe(true);
  }, 20000);

  it("shows the code for a device sign-in and gives a provider outside the catalog its own card", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    await provider("Anthropic");
    const dialog = await connect(user, /GitHub Copilot/);
    expect(within(dialog).getByRole("radio", { name: /GitHub Copilot/ })).toHaveAccessibleName(/Enter a code/);
    await user.click(within(dialog).getByRole("button", { name: "Continue" }));
    expect(await screen.findByLabelText("Sign-in code", undefined, POLL)).toHaveTextContent("ABCD-1234");
    expect(screen.getByRole("button", { name: "Copy sign-in code" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open sign-in page" })).toHaveAttribute("href", "https://auth.example.test/authorize?tx=tx-2");
    expect(screen.queryByLabelText(/^Paste the code or the final page address/)).toBeNull();

    daemon.state.oauthOutcome["tx-2"] = "ready";
    expect(await screen.findByText("Connected. GitHub Copilot is ready to use.", undefined, POLL)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(rowOf("GitHub Copilot", "GitHub Copilot")).toHaveTextContent("GitHub Copilot sign-in"));
  }, 20000);

  it("adds a later account to the rotation only once the sign-in is ready", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ keys: [keyInfo("anthropic", "k1", "Work")], pools: { anthropic: [ref("vault", "k1")] } });
    open(daemon.client);
    await provider("Anthropic");
    const dialog = await connect(user, /Claude \(Pro\/Max\)/);
    const join = within(dialog).getByLabelText("Use it in the rotation right away");
    expect(join).toBeChecked();
    await user.clear(within(dialog).getByLabelText("Name"));
    await user.type(within(dialog).getByLabelText("Name"), "Team");
    await user.click(within(dialog).getByRole("button", { name: "Continue" }));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/accounts/anthropic", body: { label: "Team", join_pool: true } }));

    await screen.findByRole("link", { name: "Open sign-in page" }, POLL);
    expect(daemon.state.pools.anthropic).toEqual([ref("vault", "k1")]);
    expect(daemon.calls.some((call) => call.method === "PUT" && call.path.startsWith("/v1/settings/pools"))).toBe(false);

    daemon.state.oauthOutcome["tx-2"] = "ready";
    await screen.findByText("Connected. Team is ready to use.", undefined, POLL);
    expect(daemon.state.pools.anthropic).toEqual([ref("vault", "k1"), ref("pi-oauth", "oa-1")]);
    await user.click(screen.getByRole("button", { name: "Done" }));
    const order = await section("Anthropic").findByRole("list", { name: "Anthropic rotation order" });
    await waitFor(() => expect(within(order).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["Work", "Team"]));
  }, 20000);

  it("explains a failed sign-in by its reason and starts a new one on Try again", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    await provider("Anthropic");
    const dialog = await connect(user, /GitHub Copilot/);
    await user.click(within(dialog).getByRole("button", { name: "Continue" }));
    await screen.findByLabelText("Sign-in code", undefined, POLL);
    daemon.state.oauthOutcome["tx-2"] = "oauth_callback_busy";
    expect(await screen.findByText(/Another sign-in is using the sign-in port/, undefined, POLL)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Try again" }));
    const creates = daemon.calls.filter((call) => call.method === "POST" && call.path === "/v1/settings/oauth/accounts/github-copilot");
    expect(creates.map((call) => call.body)).toEqual([
      { label: "GitHub Copilot", join_pool: true },
      { label: "GitHub Copilot", join_pool: true },
    ]);
    await screen.findByLabelText("Sign-in code", undefined, POLL);
    expect(screen.queryByText(/Another sign-in is using the sign-in port/)).toBeNull();
    daemon.state.oauthOutcome["tx-4"] = "ready";
    expect(await screen.findByText("Connected. GitHub Copilot is ready to use.", undefined, POLL)).toBeInTheDocument();
    expect(daemon.state.oauthAccounts.map((account) => account.account_id)).toEqual(["oa-3"]);
  }, 20000);

  it("cancels the running sign-in on the daemon", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    open(daemon.client);
    await provider("Anthropic");
    const dialog = await connect(user, /Claude \(Pro\/Max\)/);
    await user.click(within(dialog).getByRole("button", { name: "Continue" }));
    await screen.findByRole("link", { name: "Open sign-in page" }, POLL);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/transactions/tx-2/cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(daemon.state.oauthTransactions["tx-2"]!.transaction.status).toBe("cancelled");
    expect(daemon.state.oauthAccounts).toEqual([]);
  }, 20000);

  it("ignores a late answer about a sign-in that was already replaced", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon();
    const gate = Promise.withResolvers<void>();
    daemon.state.oauthGate["tx-2"] = gate.promise;
    open(daemon.client);
    await provider("Anthropic");
    const dialog = await connect(user, /Claude \(Pro\/Max\)/);
    await user.click(within(dialog).getByRole("button", { name: "Continue" }));
    // The first poll is held with the "open the sign-in page" state; the next one reports the failure.
    await waitFor(() => expect(daemon.calls.some((call) => call.method === "GET" && call.path === "/v1/settings/oauth/transactions/tx-2")).toBe(true), POLL);
    daemon.state.oauthOutcome["tx-2"] = "oauth_failed";
    expect(await screen.findByText("The sign-in did not finish. Try again.", undefined, POLL)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(screen.getByText("Starting sign-in…")).toBeInTheDocument());
    gate.resolve();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("link", { name: "Open sign-in page" })).toBeNull();
    expect(screen.getByText("Starting sign-in…")).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: "Open sign-in page" }, POLL)).toHaveAttribute("href", "https://auth.example.test/authorize?tx=tx-4");
  }, 20000);
});

describe("Signed-in accounts", () => {
  it("signs an account in again when its sign-in stopped working", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ oauthAccounts: [oauthAccount("anthropic", "oa-9", "Claude Team", { auth_invalid: true })], pools: { anthropic: [ref("pi-oauth", "oa-9")] } });
    open(daemon.client);
    await provider("Anthropic");
    const row = rowOf("Anthropic", "Claude Team");
    expect(within(row).getByText("Sign in again", { selector: "span" })).toBeInTheDocument();
    await user.click(within(row).getByRole("button", { name: "Sign in again Claude Team" }));
    const dialog = await screen.findByRole("dialog", { name: "Sign in again: Claude Team" });
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/settings/oauth/accounts/anthropic/oa-9/relogin", body: {} }));
    await within(dialog).findByRole("link", { name: "Open sign-in page" }, POLL);
    daemon.state.oauthOutcome["tx-1"] = "ready";
    expect(await within(dialog).findByText("Connected. Claude Team is ready to use.", undefined, POLL)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(within(rowOf("Anthropic", "Claude Team")).getByText("Ready")).toBeInTheDocument());
    expect(daemon.state.pools.anthropic).toEqual([ref("pi-oauth", "oa-9")]);
  }, 20000);

  it("renames, switches off and removes a signed-in account without claiming provider revocation", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ oauthAccounts: [oauthAccount("anthropic", "oa-9", "Claude Team")], pools: { anthropic: [ref("pi-oauth", "oa-9")] } });
    open(daemon.client);
    await provider("Anthropic");

    const toggle = section("Anthropic").getByRole("switch", { name: "Use Claude Team" });
    await user.click(toggle);
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "PATCH", path: "/v1/settings/oauth/accounts/anthropic/oa-9", body: { enabled: false } }));

    await user.click(section("Anthropic").getByRole("button", { name: "Manage Claude Team" }));
    const manage = await screen.findByRole("dialog", { name: "Manage Claude Team" });
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "GET", path: "/v1/settings/oauth/accounts/anthropic/oa-9" }));
    expect(within(manage).getByRole("button", { name: "Sign in again" })).toBeInTheDocument();
    const name = within(manage).getByLabelText("Name");
    await user.clear(name);
    await user.type(name, "Claude Work");
    await user.click(within(manage).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(daemon.state.oauthAccounts[0]!.label).toBe("Claude Work"));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "PATCH", path: "/v1/settings/oauth/accounts/anthropic/oa-9", body: { label: "Claude Work" } }));

    await waitFor(() => expect(rowOf("Anthropic", "Claude Work")).toBeInTheDocument());
    await user.click(section("Anthropic").getByRole("button", { name: "Manage Claude Work" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Manage Claude Work" })).getByRole("button", { name: "Remove" }));
    const confirm = screen.getByRole("dialog");
    expect(confirm).toHaveTextContent("removes this account and its sign-in");
    expect(confirm.textContent).not.toMatch(/revoke/i);
    await user.click(within(confirm).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(daemon.state.oauthAccounts).toEqual([]));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "DELETE", path: "/v1/settings/oauth/accounts/anthropic/oa-9" }));
    expect(daemon.state.pools.anthropic).toEqual([]);
  }, 20000);
});
