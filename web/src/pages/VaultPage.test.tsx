import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import type { VaultAuditRecord, VaultItem, VaultStatus } from "../api/types";
import { ClientContext } from "../context";
import { chooseOption } from "../test/select";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { VaultPage } from "./VaultPage";

const RECORDS: VaultAuditRecord[] = [
  { at: "2026-10-12T08:00:00Z", who: "opensided", what: "fill", item: "bank", origin: "https://bank.example", result: "filled", session_id: "s1", tool_call_id: "c1" },
  { at: "2026-10-12T08:05:00Z", who: "opensided", what: "fill", item: "bank", origin: "https://bank-login.example", result: "denied", detail: "origin_mismatch", session_id: "s2", tool_call_id: "c2" },
  { at: "2026-10-12T08:06:00Z", who: "local", what: "policy", item: "mail", origin: "-", result: "updated", detail: "never" },
];

const POLICIES = ["always", "while_unlocked", "never"] as const;
const SEED = "JBSWY3DPEHPK3PXP";

function daemon(initial: Partial<VaultStatus> = {}, forbidden = false) {
  const status: VaultStatus = { state: "unlocked", backend: "secret_service", reason: null, default_policy: "while_unlocked", ...initial };
  const items: VaultItem[] = [
    { id: "bank", origins: ["https://bank.example"], effective_policy: "while_unlocked", has_totp: true, passkey_rp_ids: [] },
    { id: "mail", origins: ["https://mail.example"], effective_policy: "never", policy: "never", has_totp: false, passkey_rp_ids: ["mail.example"] },
  ];
  return fakeClient((call: Call) => {
    if (forbidden) return json(403, { code: "forbidden", message: "forbidden" });
    const url = new URL(call.path, "http://daemon.test");
    const available = status.state === "unlocked";
    const body: Record<string, unknown> = call.body && typeof call.body === "object" ? { ...call.body } : {};
    switch (`${call.method} ${url.pathname}`) {
      case "GET /v1/vault":
        return json(200, status);
      case "POST /v1/vault/setup":
      case "POST /v1/vault/unlock": {
        if (status.backend === "passphrase" && body.passphrase !== "correct horse") return json(400, { code: "wrong_passphrase", message: "wrong_passphrase" });
        status.state = "unlocked";
        return json(200, status);
      }
      case "POST /v1/vault/lock":
        status.state = "locked";
        return json(200, status);
      case "GET /v1/vault/items":
        return json(200, { available, default_policy: "while_unlocked", items: available ? items : [] });
      case "POST /v1/vault/items": {
        if (items.some((i) => i.id === body.id)) return json(409, { code: "item_exists", message: "item_exists" });
        const origins = Array.isArray(body.origins) ? body.origins.map(String) : [];
        const item: VaultItem = { id: String(body.id), origins, effective_policy: "while_unlocked", has_totp: Boolean(body.totp), passkey_rp_ids: [] };
        items.push(item);
        status.state = "unlocked";
        return json(201, item);
      }
      case "GET /v1/vault/audit": {
        const item = url.searchParams.get("item");
        const session = url.searchParams.get("session");
        const since = url.searchParams.get("since");
        const records = RECORDS.filter(
          (r) => (!item || r.item === item) && (!session || r.session_id === session) && (!since || r.at >= since),
        );
        return json(200, { records });
      }
    }
    const id = decodeURIComponent(url.pathname.split("/").pop() ?? "");
    const index = items.findIndex((i) => i.id === id);
    if (call.method === "DELETE") {
      items.splice(index, 1);
      return new Response(null, { status: 204 });
    }
    const item = items[index]!;
    const policy = POLICIES.find((value) => value === body.policy);
    item.policy = policy ?? null;
    item.effective_policy = policy ?? "while_unlocked";
    return json(200, item);
  });
}

function setup(status: Partial<VaultStatus> = {}, forbidden = false) {
  const fake = daemon(status, forbidden);
  render(
    <MemoryRouter>
      <ClientContext.Provider value={fake.client}>
        <VaultPage />
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

describe("VaultPage", () => {
  it("shows item metadata and changes a per-item policy", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const items = await screen.findByRole("list", { name: "Vault items" });
    const bank = within(items).getByText("bank").closest("article") as HTMLElement;
    expect(bank).toHaveAttribute("data-item", "bank");
    expect(within(bank).getByText("one-time codes")).toBeInTheDocument();
    expect(within(bank).getByRole("combobox")).toHaveTextContent("Default (while unlocked)");
    expect(within(bank).queryByText(/enforced:/)).toBeNull();
    await chooseOption(user, within(bank).getByLabelText("Use policy for bank"), "Never");
    await vi.waitFor(() => expect(within(bank).getByLabelText("Use policy for bank")).toHaveTextContent("Never"));
    expect(calls.find((c) => c.method === "PATCH")).toMatchObject({ path: "/v1/vault/items/bank", body: { policy: "never" } });
  });

  it("lists audit records newest first, highlights denials and filters by item, session and time", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const audit = await screen.findByRole("list", { name: "Audit records" });
    const rows = within(audit).getAllByRole("listitem");
    expect(rows.map((row) => row.dataset.result)).toEqual(["updated", "denied", "filled"]);
    expect(within(rows[1]!).getByText(/origin_mismatch/)).toBeInTheDocument();
    expect(within(rows[1]!).getByText(/session s2 \/ c2/)).toBeInTheDocument();

    const filters = screen.getByRole("form", { name: "Audit filters" });
    await user.type(within(filters).getByLabelText("Item"), "bank");
    await user.type(within(filters).getByLabelText("Session"), "s2");
    await user.click(within(filters).getByRole("button", { name: "Filter" }));
    await vi.waitFor(() => expect(within(screen.getByRole("list", { name: "Audit records" })).getAllByRole("listitem")).toHaveLength(1));
    expect(calls.at(-1)?.path).toBe("/v1/vault/audit?item=bank&session=s2");

    await user.clear(within(filters).getByLabelText("Session"));
    await user.type(within(filters).getByLabelText("Since"), "2030-01-01T00:00");
    await user.click(within(filters).getByRole("button", { name: "Filter" }));
    expect(await screen.findByText("No audit records match.")).toBeInTheDocument();
    expect(new URL(calls.at(-1)!.path, "http://x").searchParams.get("since")).toBe(new Date("2030-01-01T00:00").toISOString());
  });

  it("sets up a passphrase vault, then locks and unlocks it with the passphrase", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ state: "uninitialized", backend: "passphrase" });
    const form = await screen.findByRole("form", { name: "Set up vault" });
    const button = within(form).getByRole("button", { name: "Set up vault" });
    await user.type(within(form).getByLabelText("New passphrase (8+ characters)"), "correct horse");
    await user.type(within(form).getByLabelText("Repeat passphrase"), "correct hors");
    expect(within(form).getByText("The passphrases do not match.")).toBeInTheDocument();
    expect(button).toBeDisabled();
    await user.type(within(form).getByLabelText("Repeat passphrase"), "e");
    await user.click(button);
    expect(await screen.findByText("Unlocked")).toBeInTheDocument();
    expect(calls.find((c) => c.path === "/v1/vault/setup")?.body).toEqual({ passphrase: "correct horse" });

    await user.click(screen.getByRole("button", { name: "Lock" }));
    const unlock = await screen.findByRole("form", { name: "Unlock" });
    await user.type(within(unlock).getByLabelText("Passphrase"), "wrong passphrase");
    await user.click(within(unlock).getByRole("button", { name: "Unlock" }));
    expect(await screen.findByText("Wrong passphrase.")).toBeInTheDocument();
    expect(within(unlock).getByLabelText("Passphrase")).toHaveValue("");
    await user.type(within(unlock).getByLabelText("Passphrase"), "correct horse");
    await user.click(within(unlock).getByRole("button", { name: "Unlock" }));
    expect(await screen.findByText("Unlocked")).toBeInTheDocument();
    expect(await screen.findByRole("list", { name: "Vault items" })).toBeInTheDocument();
  });

  it("adds an authenticator from a setup key without showing the key again", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ state: "uninitialized" });
    expect(await screen.findByText(/Adding the first item also sets it up/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Authenticator" }));
    const form = await screen.findByRole("form", { name: "New authenticator" });
    await user.type(within(form).getByLabelText(/Website/), "https://github.com");
    expect(within(form).getByLabelText("Item id")).toHaveAttribute("placeholder", "otp-github-com");
    await user.type(within(form).getByLabelText("Setup key or otpauth:// link"), SEED);
    await user.click(within(form).getByRole("button", { name: "Add" }));

    const items = await screen.findByRole("list", { name: "Vault items" });
    const added = await within(items).findByText("otp-github-com");
    expect(within(added.closest("article") as HTMLElement).getByText("one-time codes")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST" && c.path === "/v1/vault/items")?.body).toEqual({
      id: "otp-github-com",
      origins: ["https://github.com"],
      username: null,
      password: null,
      totp: SEED,
      policy: null,
    });
    expect(document.body.textContent).not.toContain(SEED);
  });

  it("clears the setup key after a failed save", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(await screen.findByRole("button", { name: "Authenticator" }));
    const form = await screen.findByRole("form", { name: "New authenticator" });
    await user.type(within(form).getByLabelText(/Website/), "https://bank.example");
    await user.type(within(form).getByLabelText("Item id"), "bank");
    const key = within(form).getByLabelText("Setup key or otpauth:// link");
    await user.type(key, SEED);
    await user.click(within(form).getByRole("button", { name: "Add" }));

    expect(await within(form).findByRole("alert")).toBeInTheDocument();
    expect(key).toHaveValue("");
    expect(within(form).getByLabelText("Item id")).toHaveValue("bank");
  });

  it("deletes an item only after confirmation", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const items = await screen.findByRole("list", { name: "Vault items" });
    await user.click(within(items).getByRole("button", { name: "Options for mail" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await user.click(await screen.findByRole("button", { name: "Delete" }));
    await vi.waitFor(() => expect(within(screen.getByRole("list", { name: "Vault items" })).queryByText("mail")).toBeNull());
    expect(calls.filter((c) => c.method === "DELETE").map((c) => c.path)).toEqual(["/v1/vault/items/mail"]);
  });

  it("explains a locked vault", async () => {
    setup({ state: "locked" });
    expect(await screen.findByText("Items appear here once the vault is set up and unlocked.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Unlock" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Login" })).toBeNull();
  });

  it("sends an unavailable vault to Configuration instead of a terminal command", async () => {
    setup({ state: "unavailable", backend: null, reason: "unsupported_platform" });
    expect(await screen.findByText(/Choose the passphrase unlock method in Configuration/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open Configuration" })).toHaveAttribute("href", "/config");
    expect(screen.getByText("openside doctor").closest("p")).toHaveTextContent("To check this computer from a terminal:");
    expect(screen.queryByText(/vault\.unlock/)).toBeNull();
  });

  it("lets a device without the admin scope ask for it", async () => {
    setup({}, true);
    expect((await screen.findAllByRole("button", { name: "Ask for access" })).length).toBeGreaterThan(0);
    expect(screen.queryByText(/needs the admin scope/)).toBeNull();
  });
});
