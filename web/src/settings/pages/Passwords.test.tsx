import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VaultItem, VaultSettings, VaultStatus } from "../../api/types";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { chooseOption } from "../../test/select";
import { ToastProvider } from "../../ui";
import { PasswordsSettings } from "./Passwords";
import { readCsv } from "./passwordsData";

const POLICIES = ["always", "while_unlocked", "never"] as const;
const SEED = "JBSWY3DPEHPK3PXP";
const CSV = "name,url,username,password\nbank,https://bank.example,me,EXPORTED-SECRET\n";
const KEY = "ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567-ABCD-EFGH-IJKL-MNOP-QRST";

interface Options {
  status?: Partial<VaultStatus>;
  settings?: Partial<VaultSettings>;
  forbidden?: boolean;
  remote?: boolean;
  /** Answers for one route, by `METHOD /path`, before the default handling. */
  override?: Record<string, (call: Call) => Response | Promise<Response> | undefined>;
}

/**
 * The daemon's `normalize_domain` (openside-vault autofill.rs): hosts, URLs and `*.` wildcards
 * become a lowercase host; IP literals, credentials and single labels other than localhost are refused.
 */
function normalizeDomain(input: string): string | null {
  const value = input.trim().replace(/^\*\./, "");
  if (!value || value.length > 253) return null;
  let url: URL;
  try {
    url = new URL(value.includes("://") ? value : `https://${value}`);
  } catch {
    return null;
  }
  if (url.username || url.password) return null;
  const host = url.hostname.replace(/\.+$/, "").toLowerCase();
  if (/^\d+(\.\d+){3}$/.test(host) || host.startsWith("[")) return null;
  if (!host || (!host.includes(".") && host !== "localhost")) return null;
  return host.split(".").every((label) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label)) ? host : null;
}

/** A response the test resolves later, to order daemon answers by hand. */
const deferred = () => Promise.withResolvers<Response>();

const SETTINGS: VaultSettings = {
  auto_lock: "one_week",
  biometric_unlock: false,
  biometrics: { available: false, mechanism: null, reason: "no_sensor" },
  autofill: { enabled: true, auto_submit: true, allow_insecure: false, excluded_domains: [] },
  default_policy: "while_unlocked",
  disable_in_incognito: true,
  recovery: { configured: true, current: true, created_at: "2026-11-01T10:00:00Z" },
};

function daemon({ status: initial = {}, settings: partial = {}, forbidden = false, remote = false, override = {} }: Options) {
  const status: VaultStatus = { state: "unlocked", backend: "secret_service", reason: null, default_policy: "while_unlocked", ...initial };
  let settings: VaultSettings = { ...SETTINGS, ...partial };
  const items: VaultItem[] = [
    { id: "bank", origins: ["https://bank.example"], effective_policy: "while_unlocked", has_totp: true, passkey_rp_ids: [] },
    { id: "mail", origins: ["https://mail.example"], effective_policy: "never", policy: "never", has_totp: false, passkey_rp_ids: ["mail.example"] },
  ];
  return fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    const route = `${call.method} ${url.pathname}`;
    const answer = override[route]?.(call);
    if (answer) return answer;
    if (route === "GET /v1/devices/me") return json(200, { device_id: "phone", scopes: ["steer"], request: null });
    if (forbidden) return json(403, { code: "forbidden", message: "forbidden" });
    const available = status.state === "unlocked";
    const body: Record<string, unknown> = call.body && typeof call.body === "object" ? { ...call.body } : {};
    switch (route) {
      case "GET /v1/remote/status":
        return json(200, { configure_allowed: !remote, modes: [], configuration: {} });
      case "GET /v1/sync":
        return json(200, { configured: true, set_up: true, unlocked: true, exhausted: false, last_sync: new Date(Date.now() - 32 * 60_000).toISOString(), last_run: null, location: null });
      case "POST /v1/sync/run":
        return json(200, {});
      case "GET /v1/vault":
        return json(200, status);
      case "GET /v1/vault/settings":
        return json(200, settings);
      case "PATCH /v1/vault/settings": {
        // A copy, so the recorded call keeps what the page sent.
        const autofill = body.autofill ? { ...(body.autofill as Record<string, unknown>) } : null;
        const domains: unknown = autofill?.excluded_domains;
        if (autofill && Array.isArray(domains)) {
          const normalized = domains.map((domain) => normalizeDomain(String(domain)));
          if (normalized.some((domain) => domain === null)) return json(400, { code: "invalid_domain", message: "invalid_domain" });
          autofill.excluded_domains = [...new Set(normalized)];
        }
        const next = { ...settings };
        for (const [key, value] of Object.entries(body)) if (value !== null && key !== "autofill") Object.assign(next, { [key]: value });
        if (autofill) for (const [key, value] of Object.entries(autofill)) if (value !== null) next.autofill = { ...next.autofill, [key]: value };
        settings = next;
        return json(200, settings);
      }
      case "GET /v1/vault/biometrics":
        return json(200, settings.biometrics);
      case "POST /v1/vault/export":
        return json(200, { format: "csv", filename: "openside-passwords.csv", content: CSV, exported: 2, skipped: 0 });
      case "POST /v1/vault/clear":
        return json(200, { removed: 3 });
      case "POST /v1/vault/recovery-key":
        return json(200, { recovery_key: KEY, created_at: "2026-11-06T10:00:00Z" });
      case "POST /v1/import/passwords":
        return json(200, { read: 3, imported: 2, skipped: 1, rejected: [], note: null });
      case "POST /v1/vault/setup":
      case "POST /v1/vault/unlock":
        if (status.backend === "passphrase" && body.passphrase !== "correct horse") return json(400, { code: "wrong_passphrase", message: "wrong_passphrase" });
        status.state = "unlocked";
        return json(200, status);
      case "POST /v1/vault/lock":
        status.state = "locked";
        return json(200, status);
      case "GET /v1/vault/items":
        return json(200, { available, default_policy: "while_unlocked", items: available ? items : [] });
      case "POST /v1/vault/items": {
        if (items.some((item) => item.id === body.id)) return json(409, { code: "item_exists", message: "item_exists" });
        const origins = Array.isArray(body.origins) ? body.origins.map(String) : [];
        const item: VaultItem = { id: String(body.id), origins, effective_policy: "while_unlocked", has_totp: Boolean(body.totp), passkey_rp_ids: [] };
        items.push(item);
        status.state = "unlocked";
        return json(201, item);
      }
    }
    if (url.pathname.startsWith("/v1/vault/items/")) {
      const id = decodeURIComponent(url.pathname.split("/").pop() ?? "");
      const index = items.findIndex((item) => item.id === id);
      if (call.method === "DELETE") {
        items.splice(index, 1);
        return new Response(null, { status: 204 });
      }
      const item = items[index]!;
      const policy = POLICIES.find((value) => value === body.policy);
      item.policy = policy ?? null;
      item.effective_policy = policy ?? "while_unlocked";
      return json(200, item);
    }
    return json(404, { code: "not_found", message: route });
  });
}

function setup(options: Options = {}) {
  const fake = daemon(options);
  render(
    <MemoryRouter>
      <ToastProvider>
        <ClientContext.Provider value={fake.client}>
          <PasswordsSettings />
        </ClientContext.Provider>
      </ToastProvider>
    </MemoryRouter>,
  );
  return fake;
}

/** Waits for the settings to load, which every row control depends on. */
async function loaded() {
  await vi.waitFor(() => expect(screen.getByRole("switch", { name: "Enable Autofill" })).toBeEnabled());
}

async function openRow(user: UserEvent, name: RegExp) {
  await loaded();
  await user.click(screen.getByRole("button", { name }));
  return screen.findByRole("dialog");
}

/** jsdom's File has no `arrayBuffer()`, which browsers give every file the person picks. */
function csvFile(bytes: Uint8Array): File {
  const file = new File([bytes], "passwords.csv", { type: "text/csv" });
  Object.defineProperty(file, "arrayBuffer", { value: async () => bytes.buffer });
  return file;
}

/** The vault state card on the page, once the status has loaded. */
function stateCard(state: VaultStatus["state"]): Promise<HTMLElement> {
  return vi.waitFor(() => {
    const card = document.querySelector<HTMLElement>(`[data-section=passwords] > section [data-vault-state=${state}]`);
    if (!card) throw new Error(`no ${state} card yet`);
    return card;
  });
}

const posts = (calls: Call[], path: string) => calls.filter((call) => call.method === "POST" && call.path === path);

describe("PasswordsSettings", () => {
  let clicked: HTMLAnchorElement[];
  beforeEach(() => {
    clicked = [];
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: vi.fn(() => "blob:file"), revokeObjectURL: vi.fn() }));
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      clicked.push(this);
    });
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("shows Aside's five sections with their rows", async () => {
    setup();
    await loaded();
    expect(screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent)).toEqual([
      "Password managers",
      "General",
      "Data management",
      "Autofill",
      "Danger zone",
    ]);
    expect(screen.getByRole("switch", { name: "Use default password manager" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Use default password manager" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: "Disable in incognito" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Disable in incognito" })).toBeDisabled();
    expect(await screen.findByRole("button", { name: /Vaults 1 vault, 2 items/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Regenerate" })).toBeEnabled();
    expect(await screen.findByText(/Last synced|synced/i)).toBeInTheDocument();
    // An unlocked vault shows no status card on the page, as in Aside.
    expect(document.querySelector("[data-vault-state]")).toBeNull();
  });

  it("offers the nine auto-lock times and saves the choice", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await loaded();
    const select = screen.getByRole("combobox", { name: "Auto lock" });
    expect(select).toHaveTextContent("1 week");
    await user.click(select);
    expect((await screen.findAllByRole("option")).map((option) => option.textContent)).toEqual([
      "Immediately",
      "1 minute",
      "5 minutes",
      "15 minutes",
      "1 hour",
      "4 hours",
      "1 day",
      "1 week",
      "Never",
    ]);
    await user.click(screen.getByRole("option", { name: "1 hour" }));
    await vi.waitFor(() => expect(calls.find((call) => call.method === "PATCH")).toBeTruthy());
    expect(calls.find((call) => call.method === "PATCH")).toMatchObject({
      path: "/v1/vault/settings",
      body: { auto_lock: "one_hour", autofill: null, biometric_unlock: null, default_policy: null, disable_in_incognito: null },
    });
    expect(select).toHaveTextContent("1 hour");
  });

  it("rolls a refused change back and says why", async () => {
    const user = userEvent.setup();
    setup({ override: { "PATCH /v1/vault/settings": () => json(403, { code: "person_access_required", message: "x" }) } });
    await loaded();
    const policy = screen.getByRole("combobox", { name: "Access policy for AI agents" });
    await chooseOption(user, policy, "Always");
    expect(await screen.findByText(/Only you can make this change/)).toBeInTheDocument();
    await vi.waitFor(() => expect(policy).toHaveTextContent("While unlocked"));
  });

  it("keeps the latest choice when an earlier save settles later", async () => {
    const user = userEvent.setup();
    const answers = [deferred(), deferred()];
    const { calls } = setup({ override: { "PATCH /v1/vault/settings": () => answers[calls.filter((call) => call.method === "PATCH").length - 1]!.promise } });
    await loaded();
    const select = screen.getByRole("combobox", { name: "Auto lock" });
    await chooseOption(user, select, "1 hour");
    await chooseOption(user, select, "4 hours");
    expect(select).toHaveTextContent("4 hours");
    // Saves go out one at a time: the second waits for the first.
    expect(calls.filter((call) => call.method === "PATCH")).toHaveLength(1);
    answers[0]!.resolve(json(200, { ...SETTINGS, auto_lock: "one_hour" }));
    await vi.waitFor(() => expect(calls.filter((call) => call.method === "PATCH")).toHaveLength(2));
    expect(select).toHaveTextContent("4 hours");
    answers[1]!.resolve(json(200, { ...SETTINGS, auto_lock: "four_hours" }));
    await vi.waitFor(() => expect(calls.filter((call) => call.method === "PATCH").map((call) => call.body)).toMatchObject([{ auto_lock: "one_hour" }, { auto_lock: "four_hours" }]));
    expect(select).toHaveTextContent("4 hours");
  });

  it("rolls back only the refused field, keeping a change saved meanwhile", async () => {
    const user = userEvent.setup();
    setup({
      override: {
        "PATCH /v1/vault/settings": (call) =>
          call.body && typeof call.body === "object" && "auto_lock" in call.body && call.body.auto_lock !== null ? json(400, { code: "bad_request", message: "nope" }) : undefined,
      },
    });
    await loaded();
    const insecure = screen.getByRole("switch", { name: "Allow on insecure sites" });
    await user.click(insecure);
    await chooseOption(user, screen.getByRole("combobox", { name: "Auto lock" }), "1 hour");
    await vi.waitFor(() => expect(screen.getByRole("combobox", { name: "Auto lock" })).toHaveTextContent("1 week"));
    expect(insecure).toBeChecked();
    expect(await screen.findByText(/^Couldn't change this setting/)).toBeInTheDocument();
  });

  it("restores the old value locally when the save and the reload both fail", async () => {
    const user = userEvent.setup();
    let reads = 0;
    const { calls } = setup({
      override: {
        "PATCH /v1/vault/settings": () => json(500, { code: "internal", message: "down" }),
        "GET /v1/vault/settings": () => (++reads > 1 ? json(500, { code: "internal", message: "down" }) : undefined),
      },
    });
    await loaded();
    const policy = screen.getByRole("combobox", { name: "Access policy for AI agents" });
    await chooseOption(user, policy, "Never");
    await vi.waitFor(() => expect(calls.some((call) => call.method === "PATCH")).toBe(true));
    await vi.waitFor(() => expect(policy).toHaveTextContent("While unlocked"));
  });

  it("exports a passphrase vault with the passphrase and downloads the CSV without showing it", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ status: { backend: "passphrase" } });
    const dialog = await openRow(user, /^Export/);
    expect(within(dialog).getByText("This file is not encrypted")).toBeInTheDocument();
    const field = within(dialog).getByLabelText(/Vault passphrase/);
    await user.type(field, "correct horse");
    await user.click(within(dialog).getByRole("button", { name: "Export CSV" }));
    await vi.waitFor(() => expect(clicked).toHaveLength(1));
    expect(posts(calls, "/v1/vault/export")[0]?.body).toEqual({ confirm: true, passphrase: "correct horse", biometric: false });
    expect(clicked[0]!.download).toBe("openside-passwords.csv");
    expect(await screen.findByText("Exported 2 sign-ins")).toBeInTheDocument();
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.body.textContent).not.toContain("EXPORTED-SECRET");
    expect(calls.some((call) => call.path === "/v1/vault/biometrics")).toBe(false);
    // The download's object URL is released once the browser has taken the file.
    await vi.waitFor(() => expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:file"), { timeout: 2000 });
  });

  it("asks a biometric keyring vault for a fingerprint check instead of a passphrase", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ settings: { biometric_unlock: true, biometrics: { available: true, mechanism: "fprintd", reason: null } } });
    const dialog = await openRow(user, /^Export/);
    expect(await within(dialog).findByText(/fingerprint or Touch ID check on this computer/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/Vault passphrase/)).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Export CSV" }));
    await vi.waitFor(() => expect(posts(calls, "/v1/vault/export")).toHaveLength(1));
    expect(posts(calls, "/v1/vault/export")[0]!.body).toEqual({ confirm: true, passphrase: null, biometric: true });
  });

  it("explains a refused re-authentication and clears the passphrase", async () => {
    const user = userEvent.setup();
    setup({ status: { backend: "passphrase" }, override: { "POST /v1/vault/export": () => json(409, { code: "reauth_required", message: "x" }) } });
    const dialog = await openRow(user, /^Export/);
    const field = within(dialog).getByLabelText(/Vault passphrase/);
    await user.type(field, "stale");
    await user.click(within(dialog).getByRole("button", { name: "Export CSV" }));
    expect(await within(dialog).findByText("Enter the vault passphrase to continue.")).toBeInTheDocument();
    expect(field).toHaveValue("");
    expect(clicked).toHaveLength(0);
  });

  it("says why a biometric vault can't be confirmed when this computer has no fingerprint check", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ settings: { biometric_unlock: true, biometrics: { available: false, mechanism: null, reason: "not_enrolled" } } });
    const dialog = await openRow(user, /^Export/);
    expect(await within(dialog).findByText("Add a fingerprint in this computer's settings first.")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Export CSV" })).toBeDisabled();
    expect(within(dialog).queryByLabelText(/Vault passphrase/)).toBeNull();
    expect(posts(calls, "/v1/vault/export")).toHaveLength(0);
  });

  it("words a refused re-authentication for the check this vault uses", async () => {
    const user = userEvent.setup();
    const refuse = { "POST /v1/vault/export": () => json(409, { code: "reauth_required", message: "x" }) };
    setup({ settings: { biometric_unlock: true, biometrics: { available: true, mechanism: "fprintd", reason: null } }, override: refuse });
    let dialog = await openRow(user, /^Export/);
    await user.click(await within(dialog).findByRole("button", { name: "Export CSV" }));
    expect(await within(dialog).findByText("Confirm with a fingerprint or Touch ID check to continue.")).toBeInTheDocument();
    cleanup();

    setup({ override: refuse });
    dialog = await openRow(user, /^Export/);
    await user.click(await within(dialog).findByRole("button", { name: "Export CSV" }));
    expect(await within(dialog).findByText(/Confirm it's you first/)).toBeInTheDocument();
  });

  it("exports a keyring vault with only the confirmation", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const dialog = await openRow(user, /^Export/);
    expect(within(dialog).queryByLabelText(/Vault passphrase/)).toBeNull();
    await user.click(await within(dialog).findByRole("button", { name: "Export CSV" }));
    await vi.waitFor(() => expect(clicked).toHaveLength(1));
    expect(posts(calls, "/v1/vault/export")[0]!.body).toEqual({ confirm: true, passphrase: null, biometric: false });
  });

  it("exports a passphrase vault unlocked moments ago without asking the passphrase again", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ status: { backend: "passphrase" } });
    const dialog = await openRow(user, /^Export/);
    expect(within(dialog).getByText(/unlocked the vault in the last 5 minutes/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Export CSV" }));
    await vi.waitFor(() => expect(clicked).toHaveLength(1));
    expect(posts(calls, "/v1/vault/export")[0]!.body).toEqual({ confirm: true, passphrase: null, biometric: false });
  });

  it.each([
    ["a failed fingerprint check", 401, "biometric_failed", "The fingerprint check didn't pass. Try again."],
    ["too many failed attempts", 429, "too_many_attempts", "Too many failed attempts. Wait a minute and try again."],
  ])("explains %s and keeps the dialog open", async (_, status, code, message) => {
    const user = userEvent.setup();
    setup({
      settings: { biometric_unlock: true, biometrics: { available: true, mechanism: "fprintd", reason: null } },
      override: { "POST /v1/vault/export": () => json(status, { code, message: code }) },
    });
    const dialog = await openRow(user, /^Export/);
    await user.click(await within(dialog).findByRole("button", { name: "Export CSV" }));
    expect(await within(dialog).findByText(message)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Export CSV" })).toBeEnabled();
    expect(clicked).toHaveLength(0);
  });

  it("never downloads an export that arrives after the dialog closed", async () => {
    const user = userEvent.setup();
    const answer = deferred();
    const { calls } = setup({ override: { "POST /v1/vault/export": () => answer.promise } });
    const dialog = await openRow(user, /^Export/);
    await user.click(await within(dialog).findByRole("button", { name: "Export CSV" }));
    await vi.waitFor(() => expect(posts(calls, "/v1/vault/export")).toHaveLength(1));
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    answer.resolve(json(200, { format: "csv", filename: "openside-passwords.csv", content: CSV, exported: 2, skipped: 0 }));
    await answer.promise;
    await vi.waitFor(() => expect(URL.createObjectURL).not.toHaveBeenCalled());
    expect(clicked).toHaveLength(0);
    expect(screen.queryByText("Exported 2 sign-ins")).toBeNull();
    expect(document.body.textContent).not.toContain("EXPORTED-SECRET");
  });

  it("refreshes after a clear that finished once the dialog closed, without reporting it there", async () => {
    const user = userEvent.setup();
    const answer = deferred();
    const { calls } = setup({ override: { "POST /v1/vault/clear": () => answer.promise } });
    await loaded();
    await user.click(screen.getByRole("button", { name: "Clear" }));
    const dialog = await screen.findByRole("dialog", { name: "Clear vault data?" });
    await user.click(await within(dialog).findByRole("button", { name: "Clear" }));
    await vi.waitFor(() => expect(posts(calls, "/v1/vault/clear")).toHaveLength(1));
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const reads = calls.filter((call) => call.path === "/v1/vault/items").length;
    answer.resolve(json(200, { removed: 3 }));
    await vi.waitFor(() => expect(calls.filter((call) => call.path === "/v1/vault/items").length).toBeGreaterThan(reads));
    expect(screen.queryByText("Removed 3 items")).toBeNull();
  });

  it("switches to the local-only explanation when the daemon refuses this device", async () => {
    const user = userEvent.setup();
    setup({ override: { "POST /v1/vault/export": () => json(403, { code: "local_access_required", message: "x" }) } });
    const dialog = await openRow(user, /^Export/);
    await user.click(await within(dialog).findByRole("button", { name: "Export CSV" }));
    expect(await within(dialog).findByText("To export passwords, use openside on the computer that runs it")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Export CSV" })).toBeNull();
  });

  it("imports a UTF-16 CSV, shows the result and refreshes the item count", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const dialog = await openRow(user, /^Import/);
    expect(within(dialog).getByText(/Bitwarden, 1Password or LastPass/)).toBeInTheDocument();
    const text = "url,username,password\nhttps://a.example,me,pw\n";
    const bytes = new Uint8Array(2 + text.length * 2);
    bytes.set([0xff, 0xfe]);
    for (let index = 0; index < text.length; index++) bytes[2 + index * 2] = text.charCodeAt(index);
    const input = dialog.querySelector<HTMLInputElement>("input[type=file]")!;
    expect(input).toHaveAttribute("accept", ".csv,text/csv");
    const before = calls.filter((call) => call.path === "/v1/vault/items").length;
    await user.upload(input, csvFile(bytes));
    expect(await within(dialog).findByText("Added 2, skipped 1.")).toBeInTheDocument();
    expect(posts(calls, "/v1/import/passwords")[0]!.body).toEqual({ csv: text });
    expect(calls.filter((call) => call.path === "/v1/vault/items").length).toBeGreaterThan(before);
  });

  it("asks to unlock the vault before importing into a locked one", async () => {
    const user = userEvent.setup();
    setup({ override: { "POST /v1/import/passwords": () => json(409, { code: "vault_locked", message: "x" }) } });
    const dialog = await openRow(user, /^Import/);
    await user.upload(dialog.querySelector<HTMLInputElement>("input[type=file]")!, csvFile(new TextEncoder().encode("url,username,password\n")));
    expect(await within(dialog).findByText("Unlock the openside Vault before importing saved passwords.")).toBeInTheDocument();
  });

  it("shows the daemon's refusal of a file that isn't a password CSV", async () => {
    const user = userEvent.setup();
    setup({ override: { "POST /v1/import/passwords": () => json(400, { code: "invalid_password_csv", message: "x" }) } });
    const dialog = await openRow(user, /^Import/);
    await user.upload(dialog.querySelector<HTMLInputElement>("input[type=file]")!, csvFile(new TextEncoder().encode("just,some\nrandom\"text")));
    expect(await within(dialog).findByText("Choose a browser-exported CSV file with URL, username and password columns.")).toBeInTheDocument();
  });

  it("opens the import from Connect external password manager with a note", async () => {
    const user = userEvent.setup();
    setup();
    const dialog = await openRow(user, /Connect external password manager/);
    expect(within(dialog).getByText(/can't connect to another password manager yet/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Choose CSV file" })).toBeInTheDocument();
  });

  it("shows a recovery key once: closing discards it and reopening starts over", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    let dialog = await openRow(user, /^Regenerate$/);
    expect(within(dialog).getByText(/stops working right away/)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(KEY);
    await user.click(await within(dialog).findByRole("button", { name: "Regenerate" }));
    expect(await within(dialog).findByText(KEY)).toBeInTheDocument();
    expect(within(dialog).getByText(/only time the key is shown/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Download .txt" }));
    expect(clicked[0]!.download).toBe("openside-recovery-key.txt");
    await user.click(within(dialog).getByRole("button", { name: "Done" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.body.textContent).not.toContain(KEY);

    await user.click(screen.getByRole("button", { name: "Regenerate" }));
    dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/stops working right away/)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(KEY);
    expect(posts(calls, "/v1/vault/recovery-key")).toHaveLength(1);
  });

  it("drops a recovery key that arrives after the dialog closed", async () => {
    const user = userEvent.setup();
    const answer = deferred();
    const { calls } = setup({ override: { "POST /v1/vault/recovery-key": () => answer.promise } });
    let dialog = await openRow(user, /^Regenerate$/);
    await user.click(await within(dialog).findByRole("button", { name: "Regenerate" }));
    await vi.waitFor(() => expect(posts(calls, "/v1/vault/recovery-key")).toHaveLength(1));
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await user.click(screen.getByRole("button", { name: "Regenerate" }));
    dialog = await screen.findByRole("dialog");
    answer.resolve(json(200, { recovery_key: KEY, created_at: "2026-11-06T10:00:00Z" }));
    // The late answer still refreshes the row (the old key no longer works) but is never shown.
    await vi.waitFor(() => expect(calls.filter((call) => call.path === "/v1/vault/settings" && call.method === "GET").length).toBeGreaterThan(1));
    expect(within(dialog).getByText(/stops working right away/)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(KEY);
  });

  it("clears vault data only after Clear is pressed in the confirmation", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await loaded();
    await user.click(screen.getByRole("button", { name: "Clear" }));
    const dialog = await screen.findByRole("dialog", { name: "Clear vault data?" });
    expect(within(dialog).getByText(/can't be undone/)).toBeInTheDocument();
    expect(posts(calls, "/v1/vault/clear")).toHaveLength(0);
    await user.click(await within(dialog).findByRole("button", { name: "Clear" }));
    await vi.waitFor(() => expect(posts(calls, "/v1/vault/clear")).toHaveLength(1));
    expect(posts(calls, "/v1/vault/clear")[0]!.body).toEqual({ confirm: true, passphrase: null, biometric: false });
    expect(await screen.findByText("Removed 3 items")).toBeInTheDocument();
  });

  it("explains on another device that local-only actions work on the computer", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ remote: true, settings: { biometrics: { available: true, mechanism: "fprintd", reason: null } } });
    await loaded();
    await vi.waitFor(() => expect(screen.getByRole("switch", { name: "Unlock with Biometrics" })).toBeDisabled());
    expect(screen.getByText("Turn this on from the computer running openside.")).toBeInTheDocument();
    for (const [row, what] of [
      [/^Export/, "export passwords"],
      [/^Import/, "import passwords"],
      [/^Regenerate$/, "create a recovery key"],
      [/^Clear$/, "clear vault data"],
    ] as const) {
      const dialog = await openRow(user, row);
      expect(within(dialog).getByText(`To ${what}, use openside on the computer that runs it`)).toBeInTheDocument();
      await user.keyboard("{Escape}");
      await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    }
    expect(calls.some((call) => call.method === "POST")).toBe(false);
  });

  it("explains why biometrics are unavailable on this computer", async () => {
    setup();
    await loaded();
    expect(screen.getByRole("switch", { name: "Unlock with Biometrics" })).toBeDisabled();
    expect(screen.getByText("No fingerprint sensor was found on this computer.")).toBeInTheDocument();
  });

  it("adds and removes excluded domains, showing a refused domain inline", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await loaded();
    const toggle = screen.getByRole("button", { name: /Excluded domains/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const input = screen.getByRole("textbox", { name: "Domain to exclude" });
    await user.type(input, "1.2.3.4{Enter}");
    expect(await screen.findByText("Enter a domain such as example.com.")).toBeInTheDocument();
    expect(input).toHaveValue("1.2.3.4");
    await user.clear(input);
    // The daemon stores the host of a pasted URL, and the domain a wildcard covers.
    await user.type(input, "https://Bank.example/login{Enter}");
    const list = await screen.findByRole("list", { name: "Excluded domains" });
    await vi.waitFor(() => expect(within(list).getByText("bank.example")).toBeInTheDocument());
    expect(input).toHaveValue("");
    await user.type(input, "*.mail.example{Enter}");
    await vi.waitFor(() => expect(within(list).getByText("mail.example")).toBeInTheDocument());
    await user.type(input, "localhost{Enter}");
    await vi.waitFor(() => expect(within(list).getByText("localhost")).toBeInTheDocument());
    expect(screen.queryByRole("alert")).toBeNull();
    // Trimmed, lowercased and without the trailing dot it is a domain already listed, so the list keeps one.
    await user.type(input, "  MAIL.example.  {Enter}");
    await vi.waitFor(() => expect(input).toHaveValue(""));
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["bank.example", "mail.example", "localhost"]);
    await user.click(within(list).getByRole("button", { name: "Remove bank.example" }));
    await vi.waitFor(() => expect(within(list).queryByText("bank.example")).toBeNull());
    expect(calls.filter((call) => call.method === "PATCH").map((call) => call.body)).toMatchObject([
      { autofill: { excluded_domains: ["1.2.3.4"] } },
      { autofill: { excluded_domains: ["https://Bank.example/login"] } },
      { autofill: { excluded_domains: ["bank.example", "*.mail.example"] } },
      { autofill: { excluded_domains: ["bank.example", "mail.example", "localhost"] } },
      { autofill: { excluded_domains: ["bank.example", "mail.example", "localhost", "MAIL.example."] } },
      { autofill: { excluded_domains: ["mail.example", "localhost"] } },
    ]);
  });

  it("adds two domains in a row without losing the first", async () => {
    const user = userEvent.setup();
    const answers = [deferred(), deferred()];
    const { calls } = setup({ override: { "PATCH /v1/vault/settings": () => answers[calls.filter((call) => call.method === "PATCH").length - 1]!.promise } });
    await loaded();
    await user.click(screen.getByRole("button", { name: /Excluded domains/ }));
    const input = screen.getByRole("textbox", { name: "Domain to exclude" });
    await user.type(input, "a.example{Enter}");
    await user.type(input, "{Control>}a{/Control}b.example{Enter}");
    answers[0]!.resolve(json(200, { ...SETTINGS, autofill: { ...SETTINGS.autofill, excluded_domains: ["a.example"] } }));
    await vi.waitFor(() => expect(calls.filter((call) => call.method === "PATCH")).toHaveLength(2));
    answers[1]!.resolve(json(200, { ...SETTINGS, autofill: { ...SETTINGS.autofill, excluded_domains: ["a.example", "b.example"] } }));
    const list = await screen.findByRole("list", { name: "Excluded domains" });
    await vi.waitFor(() => expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["a.example", "b.example"]));
    expect(calls[calls.findLastIndex((call) => call.method === "PATCH")]!.body).toMatchObject({ autofill: { excluded_domains: ["a.example", "b.example"] } });
  });

  it("saves autofill switches as autofill changes", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await loaded();
    await user.click(screen.getByRole("switch", { name: "Allow on insecure sites" }));
    await vi.waitFor(() => expect(calls.find((call) => call.method === "PATCH")).toBeTruthy());
    expect(calls.find((call) => call.method === "PATCH")!.body).toMatchObject({
      auto_lock: null,
      autofill: { allow_insecure: true, auto_submit: null, enabled: null, excluded_domains: null },
    });
    expect(screen.getByRole("switch", { name: "Allow on insecure sites" })).toBeChecked();
  });

  describe("Vaults dialog", () => {
    it("shows item metadata and changes a per-item policy", async () => {
      const user = userEvent.setup();
      const { calls } = setup();
      const dialog = await openRow(user, /^Vaults/);
      const items = await within(dialog).findByRole("list", { name: "Vault items" });
      const bank = within(items).getByText("bank").closest("article") as HTMLElement;
      expect(bank).toHaveAttribute("data-item", "bank");
      expect(within(bank).getByText("one-time codes")).toBeInTheDocument();
      expect(within(bank).getByRole("combobox")).toHaveTextContent("Default (while unlocked)");
      expect(within(bank).queryByText(/enforced:/)).toBeNull();
      await chooseOption(user, within(bank).getByLabelText("Use policy for bank"), "Never");
      await vi.waitFor(() => expect(within(bank).getByLabelText("Use policy for bank")).toHaveTextContent("Never"));
      expect(calls.find((call) => call.method === "PATCH")).toMatchObject({ path: "/v1/vault/items/bank", body: { policy: "never" } });
      // The audit log stays in the CLI, as Aside's Passwords page has none.
      expect(calls.some((call) => call.path.startsWith("/v1/vault/audit"))).toBe(false);
    });

    it("searches items by id and website", async () => {
      const user = userEvent.setup();
      setup();
      const dialog = await openRow(user, /^Vaults/);
      await within(dialog).findByRole("list", { name: "Vault items" });
      await user.type(within(dialog).getByRole("searchbox", { name: "Search sign-ins" }), "mail.ex");
      const items = within(dialog).getByRole("list", { name: "Vault items" });
      expect(within(items).queryByText("bank")).toBeNull();
      expect(within(items).getByText("mail")).toBeInTheDocument();
      await user.type(within(dialog).getByRole("searchbox", { name: "Search sign-ins" }), "zzz");
      expect(within(dialog).getByText("Nothing matches “mail.exzzz”.")).toBeInTheDocument();
    });

    it("adds an authenticator from a setup key without showing the key again", async () => {
      const user = userEvent.setup();
      const { calls } = setup({ status: { state: "uninitialized" } });
      const dialog = await openRow(user, /^Vaults/);
      expect(within(dialog).getByText(/Adding the first item also sets it up/)).toBeInTheDocument();
      await user.click(within(dialog).getByRole("button", { name: "Authenticator" }));
      const form = await screen.findByRole("form", { name: "New authenticator" });
      await user.type(within(form).getByLabelText(/Website/), "https://github.com");
      expect(within(form).getByLabelText("Item id")).toHaveAttribute("placeholder", "otp-github-com");
      await user.type(within(form).getByLabelText("Setup key or otpauth:// link"), SEED);
      await user.click(within(form).getByRole("button", { name: "Add" }));

      const items = await within(dialog).findByRole("list", { name: "Vault items" });
      const added = await within(items).findByText("otp-github-com");
      expect(within(added.closest("article") as HTMLElement).getByText("one-time codes")).toBeInTheDocument();
      expect(posts(calls, "/v1/vault/items")[0]?.body).toEqual({ id: "otp-github-com", origins: ["https://github.com"], username: null, password: null, totp: SEED, policy: null });
      expect(document.body.textContent).not.toContain(SEED);
    });

    it("clears the setup key after a failed save", async () => {
      const user = userEvent.setup();
      setup();
      const dialog = await openRow(user, /^Vaults/);
      await user.click(within(dialog).getByRole("button", { name: "Authenticator" }));
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
      const dialog = await openRow(user, /^Vaults/);
      const items = await within(dialog).findByRole("list", { name: "Vault items" });
      await user.click(within(items).getByRole("button", { name: "Options for mail" }));
      await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
      expect(calls.some((call) => call.method === "DELETE")).toBe(false);
      await user.click(await screen.findByRole("button", { name: "Delete" }));
      await vi.waitFor(() => expect(within(within(dialog).getByRole("list", { name: "Vault items" })).queryByText("mail")).toBeNull());
      expect(calls.filter((call) => call.method === "DELETE").map((call) => call.path)).toEqual(["/v1/vault/items/mail"]);
    });
  });

  it("sets up a passphrase vault from the page, then locks and unlocks it", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ status: { state: "uninitialized", backend: "passphrase" } });
    const card = await stateCard("uninitialized");
    expect(await screen.findByRole("button", { name: /Vaults No vault yet/ })).toBeInTheDocument();
    await user.click(within(card).getByRole("button", { name: "Set up" }));
    const form = await screen.findByRole("form", { name: "Set up vault" });
    const button = within(form).getByRole("button", { name: "Set up vault" });
    await user.type(within(form).getByLabelText("New passphrase (8+ characters)"), "correct horse");
    await user.type(within(form).getByLabelText("Repeat passphrase"), "correct hors");
    expect(within(form).getByText("The passphrases do not match.")).toBeInTheDocument();
    expect(button).toBeDisabled();
    await user.type(within(form).getByLabelText("Repeat passphrase"), "e");
    await user.click(button);
    await vi.waitFor(() => expect(document.querySelector("[data-vault-state]")).toBeNull());
    expect(posts(calls, "/v1/vault/setup")[0]?.body).toEqual({ passphrase: "correct horse" });

    const dialog = await openRow(user, /^Vaults/);
    await user.click(within(dialog).getByRole("button", { name: "Lock" }));
    await user.keyboard("{Escape}");
    const locked = await stateCard("locked");
    await user.click(within(locked).getByRole("button", { name: "Unlock" }));
    const unlock = await screen.findByRole("form", { name: "Unlock" });
    await user.type(within(unlock).getByLabelText("Passphrase"), "wrong passphrase");
    await user.click(within(unlock).getByRole("button", { name: "Unlock" }));
    expect(await screen.findByText("Wrong passphrase.")).toBeInTheDocument();
    expect(within(unlock).getByLabelText("Passphrase")).toHaveValue("");
    await user.type(within(unlock).getByLabelText("Passphrase"), "correct horse");
    await user.click(within(unlock).getByRole("button", { name: "Unlock" }));
    await vi.waitFor(() => expect(document.querySelector("[data-vault-state]")).toBeNull());
  });

  it("explains a locked vault and offers its recovery key", async () => {
    const user = userEvent.setup();
    setup({ status: { state: "locked" } });
    const card = await stateCard("locked");
    expect(within(card).getByRole("button", { name: "Unlock" })).toBeInTheDocument();
    expect(await within(card).findByRole("button", { name: "Use recovery key" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Vaults 1 vault, locked/ })).toBeInTheDocument();
    const dialog = await openRow(user, /^Vaults/);
    expect(await within(dialog).findByText("Items appear here once the vault is set up and unlocked.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Login" })).toBeNull();
  });

  it("asks for the recovery key on the computer running openside", async () => {
    const user = userEvent.setup();
    setup({ status: { state: "locked" } });
    const card = await stateCard("locked");
    await user.click(await within(card).findByRole("button", { name: "Use recovery key" }));
    const dialog = await screen.findByRole("dialog", { name: "Open with recovery key" });
    expect(within(dialog).getByLabelText("Recovery key")).toBeInTheDocument();
  });

  it("never takes the recovery key on another device, from the page or the Vaults dialog", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ status: { state: "locked" }, remote: true });
    await vi.waitFor(() => expect(calls.some((call) => call.path === "/v1/remote/status")).toBe(true));
    const card = await stateCard("locked");
    await user.click(await within(card).findByRole("button", { name: "Use recovery key" }));
    let dialog = await screen.findByRole("dialog", { name: "Open with recovery key" });
    expect(within(dialog).getByText("To open the vault with its recovery key, use openside on the computer that runs it")).toBeInTheDocument();
    expect(within(dialog).queryByLabelText("Recovery key")).toBeNull();
    await user.keyboard("{Escape}");
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    const vaults = await openRow(user, /^Vaults/);
    await user.click(await within(vaults).findByRole("button", { name: "Use recovery key" }));
    dialog = await screen.findByRole("dialog", { name: "Open with recovery key" });
    expect(within(dialog).queryByLabelText("Recovery key")).toBeNull();
    expect(within(dialog).getByText(/use openside on the computer that runs it/)).toBeInTheDocument();
    expect(posts(calls, "/v1/vault/recover")).toHaveLength(0);
  });

  it("explains an unavailable vault and points at the terminal check", async () => {
    setup({ status: { state: "unavailable", backend: null, reason: "unsupported_platform" } });
    expect(await screen.findByText(/Set vault\.unlock = "passphrase" in config\.toml/)).toBeInTheDocument();
    expect(screen.getByText("openside doctor").closest("p")).toHaveTextContent("To check this computer from a terminal:");
    expect(screen.queryByText(/connections\.vault\.|passwords\./)).toBeNull();
  });

  it("lets a device without the admin scope ask for it", async () => {
    setup({ forbidden: true });
    expect((await screen.findAllByRole("button", { name: "Ask for access" })).length).toBeGreaterThan(0);
  });
});

describe("readCsv", () => {
  const text = "url,username,password\nhttps://café.example,ünï,pw\n";
  const utf16 = (littleEndian: boolean) => {
    const bytes = new Uint8Array(2 + text.length * 2);
    const view = new DataView(bytes.buffer);
    view.setUint16(0, 0xfeff, littleEndian);
    for (let index = 0; index < text.length; index++) view.setUint16(2 + index * 2, text.charCodeAt(index), littleEndian);
    return bytes;
  };

  it.each([
    ["UTF-8", new TextEncoder().encode(text)],
    ["UTF-8 with a byte-order mark", new Uint8Array([0xef, 0xbb, 0xbf, ...new TextEncoder().encode(text)])],
    ["UTF-16LE with a byte-order mark", utf16(true)],
    ["UTF-16BE with a byte-order mark", utf16(false)],
  ])("reads %s without the byte-order mark", async (_, bytes) => {
    expect(await readCsv(csvFile(bytes))).toBe(text);
  });
});
