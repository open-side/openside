import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { PointerEventsCheckLevel, type UserEvent } from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, type Client } from "../api/client";
import type { ImportBatchReport } from "../api/types";
import { ClientContext } from "../context";
import { keyInfo, modelState } from "../models/testDaemon";
import { json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { guideDismissed } from "./dismissed";
import { POLL_MS } from "./ImportStep";
import { OnboardingPage } from "./OnboardingPage";
import { batch, job, onboardingDaemon, type DaemonState } from "./testDaemon";

const grant = vi.hoisted(() => vi.fn());
const pair = vi.hoisted(() => vi.fn());
const deliver = vi.hoisted(() => vi.fn());
const installOffer = vi.hoisted(() => vi.fn());
vi.mock("../import/delivery", async (original) => ({ ...(await original<typeof import("../import/delivery")>()), requestImportPermissions: grant, deliverImport: deliver }));
vi.mock("../desktopNotify", async (original) => ({ ...(await original<typeof import("../desktopNotify")>()), pairExtension: pair }));
vi.mock("../pwa/install", async (original) => ({ ...(await original<typeof import("../pwa/install")>()), currentOffer: installOffer }));

function Where() {
  const location = useLocation();
  return <p data-testid="where">{location.pathname + location.search}</p>;
}

function open(client: Client, step?: string) {
  return render(
    <ClientContext.Provider value={client}>
      <ToastProvider>
        <MemoryRouter initialEntries={[step ? `/welcome?step=${step}` : "/welcome"]}>
          <Where />
          <Routes>
            <Route path="/welcome" element={<OnboardingPage />} />
            <Route path="/settings/*" element={<p>settings</p>} />
            <Route path="/" element={<p>home</p>} />
          </Routes>
        </MemoryRouter>
      </ToastProvider>
    </ClientContext.Provider>,
  );
}

const writes = (calls: Call[]) => calls.filter((call) => call.method !== "GET");
const heading = (name: string) => screen.findByRole("heading", { level: 1, name });

/** jsdom's File has no `arrayBuffer()`, which browsers give every file the person picks. */
function csvFile(text: string): File {
  const bytes = new TextEncoder().encode(text);
  const file = new File([bytes], "passwords.csv", { type: "text/csv" });
  Object.defineProperty(file, "arrayBuffer", { value: async () => bytes.buffer });
  return file;
}

function batchSummary(): ImportBatchReport {
  return { ...batch(false), dry_run: false };
}

beforeEach(async () => {
  localStorage.clear();
  grant.mockReset();
  pair.mockReset();
  installOffer.mockReset().mockReturnValue(null);
  const actual = await vi.importActual<typeof import("../import/delivery")>("../import/delivery");
  deliver.mockReset().mockImplementation(actual.deliverImport);
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("order and navigation", () => {
  it("goes welcome, import, passwords, subscription, done, with a pager on the middle three", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon({ vault: "unlocked" }).client);
    expect(await heading("An AI assistant that works in your browser.")).toHaveFocus();
    expect(screen.queryByRole("list", { name: /^Step/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Get started" }));

    const steps = [
      { title: "Import from another browser", step: "import", pager: "Step 1 of 3" },
      { title: "Let openside fill in your passwords", step: "passwords", pager: "Step 2 of 3" },
      { title: "Choose your AI subscription", step: "subscription", pager: "Step 3 of 3" },
    ];
    for (const { title, step, pager } of steps) {
      expect(await heading(title)).toHaveFocus();
      expect(screen.getByTestId("where")).toHaveTextContent(`/welcome?step=${step}`);
      expect(screen.getByRole("list", { name: pager })).toBeInTheDocument();
      await user.click(await screen.findByRole("button", { name: "Skip" }));
    }
    expect(await heading("You’re all set!")).toHaveFocus();
    expect(screen.getByTestId("where")).toHaveTextContent("/welcome?step=done");
    expect(screen.queryByRole("list", { name: /^Step/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull();
  });

  it("goes back one step through the URL, to the welcome at the start", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon().client, "subscription");
    await heading("Choose your AI subscription");
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(await heading("Let openside fill in your passwords")).toHaveFocus();
    expect(screen.getByTestId("where")).toHaveTextContent("/welcome?step=passwords");
    await user.click(screen.getByRole("button", { name: "Back" }));
    await heading("Import from another browser");
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(await heading("An AI assistant that works in your browser.")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/welcome$/);
  });

  it.each(["bogus", "agents", "models", "pair", "assistant"])("starts at the welcome for an unknown ?step=%s", async (step) => {
    open(onboardingDaemon().client, step);
    expect(await heading("An AI assistant that works in your browser.")).toBeInTheDocument();
  });

  it("skips the whole guide from the welcome and records it on the daemon", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client);
    await user.click(screen.getByRole("button", { name: "Skip setup" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "PUT", path: "/v1/onboarding", body: { state: "skipped" } })]);
    expect(daemon.state.onboarding).toBe("skipped");
  });

  it("leaves the stored outcome alone when the guide was reopened from Settings", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ onboarding: "completed" });
    open(daemon.client);
    await user.click(screen.getByRole("button", { name: "Skip setup" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([]);
    expect(daemon.state.onboarding).toBe("completed");
  });

  it("does not trap a device that cannot record the outcome", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ override: (call) => (call.method === "PUT" ? json(403, { code: "forbidden", message: "no" }) : undefined) });
    open(daemon.client);
    await user.click(screen.getByRole("button", { name: "Skip setup" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(guideDismissed()).toBe(true);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("import step", () => {
  const profileBox = (name: string, checked = true) => screen.findByRole("checkbox", { name, checked });
  const importButton = () => screen.findByRole("button", { name: "Import" });

  it("preselects every browser and every kind but passwords, puts Import in the footer and only previews", async () => {
    const daemon = onboardingDaemon({ vault: "unlocked" });
    open(daemon.client, "import");
    for (const name of ["Import Chrome · Work", "Import Brave · Personal", "Import Firefox · default"]) expect(await profileBox(name)).toBeChecked();
    const chrome = screen.getByRole("checkbox", { name: "Import Chrome · Work" }).closest("label")!;
    expect(chrome).toHaveTextContent("work@example.test");
    // Passwords have their own step, so the browser rows do not count them.
    await waitFor(() => expect(chrome).toHaveTextContent("1,200 history entries · 31 bookmarks · 77 cookies · 3 open tabs"));
    expect(chrome).not.toHaveTextContent("passwords");
    for (const label of ["History", "Bookmarks", "Cookies", "Autofill", "Open tabs", "Search engines", "Site permissions", "Extensions"]) {
      expect(screen.getByRole("checkbox", { name: new RegExp(`^${label}`) })).toBeChecked();
    }
    expect(screen.getByRole("checkbox", { name: /^Passwords/ })).not.toBeChecked();
    const start = await importButton();
    expect(start).toBeEnabled();
    expect(start.closest("footer")).not.toBeNull();
    expect(screen.getByRole("button", { name: "Skip" })).toBeInTheDocument();
    // Looking before writing is the daemon's preview, never a write.
    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import/preview"]);
  });

  it("runs the import, shows progress, then the summary and a Continue button in place of Skip", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      vault: "unlocked",
      jobPolls: [
        job({ completed_profiles: 1, current_profile: "brave:Default" }),
        job({ state: "completed", completed_profiles: 3, summary: { ...batchSummary(), restore_tabs: [{ profile_id: "chrome:Default", url: "https://example.test/a", title: "Example A" }] } }),
      ],
    });
    open(daemon.client, "import");
    await user.click(await importButton());
    const request = writes(daemon.calls).at(-1)!;
    expect(request).toMatchObject({ method: "POST", path: "/v1/import", body: { profiles: null } });
    expect((request.body as { what: string[] }).what).not.toContain("passwords");
    const meter = await screen.findByRole("progressbar", { name: "Import progress" });
    expect(meter).toHaveAttribute("aria-valuenow", "0");
    expect(screen.getByRole("status")).toHaveTextContent("Importing…");
    await waitFor(() => expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "1"), { timeout: POLL_MS * 4 });
    expect(screen.getByRole("status")).toHaveTextContent("Brave · Personal · 1 of 3 profiles");

    const summary = await screen.findByRole("list", { name: "Imported" }, { timeout: POLL_MS * 6 });
    expect(summary).toHaveTextContent("History1,580");
    expect(screen.getByRole("status")).toHaveTextContent("Imported from 3 profiles");
    expect(screen.getByRole("link", { name: "Example A" })).toHaveAttribute("href", "https://example.test/a");
    expect(screen.queryByRole("button", { name: "Import" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Skip" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await heading("Let openside fill in your passwords")).toBeInTheDocument();
  });

  it("lists extensions as names with store links, search engines and permissions, and never installs anything", async () => {
    const user = userEvent.setup();
    const details = {
      extensions: [{ profile_id: "chrome:Default", id: "abc", name: "Dark Reader", store_url: "https://store.example/abc" }, { profile_id: "chrome:Default", id: "def", name: "Local tool", store_url: null }],
      search_engines: [{ profile_id: "chrome:Default", name: "DuckDuckGo", keyword: "ddg", url: "https://duckduckgo.com/?q=%s" }],
      permissions: [{ profile_id: "chrome:Default", origin: "https://meet.example", permission: "camera", setting: "allow" }],
    };
    const daemon = onboardingDaemon({ vault: "unlocked", jobPolls: [job({ state: "completed", completed_profiles: 3, summary: { ...batchSummary(), ...details } })] });
    open(daemon.client, "import");
    await user.click(await importButton());
    await screen.findByRole("list", { name: "Imported" }, { timeout: POLL_MS * 6 });
    expect(screen.getByRole("link", { name: "Dark Reader" })).toHaveAttribute("href", "https://store.example/abc");
    expect(screen.getByText("Local tool").closest("a")).toBeNull();
    expect(screen.getByText("DuckDuckGo · ddg")).toBeInTheDocument();
    expect(screen.getByText("https://meet.example · camera · allow")).toBeInTheDocument();
  });

  it("hands bookmarks and history to the extension after the import", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    deliver.mockResolvedValue({ delivered: 1, pending: 0, extensionMissing: false });
    open(daemon.client, "import");
    await user.click(await importButton());
    expect(await screen.findByText("Your bookmarks and history are in this browser.", {}, { timeout: POLL_MS * 6 })).toBeInTheDocument();
    expect(deliver).toHaveBeenCalledWith(daemon.client, expect.any(Function));
  });

  it("offers 'Sign in to your sites in openside' when cookies need the person's approval, and delivers after it", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    deliver.mockRejectedValue(new ApiError(0, "import_delivery_permission_required", "Allow the sites first."));
    grant.mockResolvedValue({ delivered: 1, pending: 0, extensionMissing: false });
    open(daemon.client, "import");
    await user.click(await importButton());
    await user.click(await screen.findByRole("button", { name: "Sign in to your sites in openside" }, { timeout: POLL_MS * 6 }));
    expect(grant).toHaveBeenCalledOnce();
    expect(await screen.findByText("Your bookmarks and history are in this browser.")).toBeInTheDocument();
  });

  it("offers 'Connect the openside extension' when delivery needs pairing, then pairs and delivers", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    deliver.mockRejectedValueOnce(new ApiError(0, "import_delivery_pairing_required", "Connect the extension first."));
    deliver.mockResolvedValueOnce({ delivered: 1, pending: 0, extensionMissing: false });
    pair.mockResolvedValue({ ok: true });
    open(daemon.client, "import");
    await user.click(await importButton());
    await user.click(await screen.findByRole("button", { name: "Connect the openside extension" }, { timeout: POLL_MS * 6 }));
    expect(pair).toHaveBeenCalledOnce();
    expect(await screen.findByText("Your bookmarks and history are in this browser.")).toBeInTheDocument();
    expect(deliver).toHaveBeenCalledTimes(2);
  });

  it("says to install the extension, not that something failed, when none answers", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    deliver.mockResolvedValue({ delivered: 0, pending: 1, extensionMissing: true });
    open(daemon.client, "import");
    await user.click(await importButton());
    expect(await screen.findByText(/Install the openside extension/, {}, { timeout: POLL_MS * 6 })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("imports only the chosen profiles and kinds", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    open(daemon.client, "import");
    await user.click(await profileBox("Import Firefox · default"));
    await user.click(screen.getByText("Choose what to bring"));
    await user.click(screen.getByRole("checkbox", { name: /^Cookies/ }));
    await user.click(await importButton());
    expect(writes(daemon.calls).at(-1)).toMatchObject({ path: "/v1/import" });
    const body = writes(daemon.calls).at(-1)!.body as { profiles: string[]; what: string[] };
    expect(body.profiles).toEqual(["chrome:Default", "brave:Default"]);
    expect(body.what).not.toContain("cookies");
    expect(body.what).not.toContain("passwords");
    expect(body.what).toContain("history");
  });

  it("will not start with nothing selected", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon({ vault: "unlocked" }).client, "import");
    for (const name of ["Import Chrome · Work", "Import Brave · Personal", "Import Firefox · default"]) await user.click(await profileBox(name));
    expect(await importButton()).toBeDisabled();
    expect(screen.getByText("Choose at least one browser.")).toBeInTheDocument();
  });

  it("sets the vault up inline when passwords are added to the browser import", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "uninitialized" });
    open(daemon.client, "import");
    await profileBox("Import Chrome · Work");
    expect(screen.queryByRole("form", { name: "Vault" })).toBeNull();
    await user.click(screen.getByText("Choose what to bring"));
    await user.click(screen.getByRole("checkbox", { name: /^Passwords/ }));
    const form = await screen.findByRole("form", { name: "Vault" });
    expect(form).toHaveTextContent("Passwords go into an encrypted vault");
    expect(within(form).queryByRole("link")).toBeNull();
    expect(await importButton()).toBeDisabled();
    await user.click(within(form).getByRole("button", { name: "Set up the vault" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "Vault" })).toBeNull());
    expect(writes(daemon.calls).map((call) => call.path)).toContain("/v1/vault/setup");
    await waitFor(async () => expect(await importButton()).toBeEnabled());
  });

  it("asks for a passphrase when the computer has no keyring", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "uninitialized", vaultBackend: "passphrase" });
    open(daemon.client, "import");
    await profileBox("Import Chrome · Work");
    await user.click(screen.getByText("Choose what to bring"));
    await user.click(screen.getByRole("checkbox", { name: /^Passwords/ }));
    const form = await screen.findByRole("form", { name: "Vault" });
    const submit = within(form).getByRole("button", { name: "Set up the vault" });
    expect(submit).toBeDisabled();
    await user.type(within(form).getByLabelText("Passphrase"), "correct horse");
    await user.type(within(form).getByLabelText("Repeat passphrase"), "correct hors");
    expect(submit).toBeDisabled();
    await user.type(within(form).getByLabelText("Repeat passphrase"), "e");
    await user.click(submit);
    await waitFor(() => expect(screen.queryByRole("form", { name: "Vault" })).toBeNull());
    expect(writes(daemon.calls).find((call) => call.path === "/v1/vault/setup")?.body).toEqual({ passphrase: "correct horse" });
  });

  it("can leave passwords out again instead of unlocking the vault", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "locked" });
    open(daemon.client, "import");
    await profileBox("Import Chrome · Work");
    await user.click(screen.getByText("Choose what to bring"));
    await user.click(screen.getByRole("checkbox", { name: /^Passwords/ }));
    expect(await screen.findByRole("form", { name: "Vault" })).toHaveTextContent("Unlock the vault to import passwords");
    await user.click(screen.getByRole("button", { name: "Import without passwords" }));
    expect(screen.queryByRole("form", { name: "Vault" })).toBeNull();
    await user.click(await importButton());
    expect((writes(daemon.calls).at(-1)!.body as { what: string[] }).what).not.toContain("passwords");
  });

  it("explains a failed import and offers to leave cookies out", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      vault: "unlocked",
      override: (call) => (call.path === "/v1/import" ? json(503, { code: "browser_unavailable", message: "Chrome is running." }) : undefined),
    });
    open(daemon.client, "import");
    await user.click(await importButton());
    expect(await screen.findByRole("alert")).toHaveTextContent("No Chromium-compatible browser was found");
    await user.click(screen.getByRole("button", { name: "Leave cookies out" }));
    expect(screen.queryByRole("button", { name: "Leave cookies out" })).toBeNull();
    expect(await importButton()).toBeEnabled();
  });

  it("does not claim nothing was changed when a job fails after some profiles finished", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked", jobPolls: [job({ state: "failed", completed_profiles: 1, summary: null, error: null })] });
    open(daemon.client, "import");
    await user.click(await importButton());
    const alert = await screen.findByRole("alert", {}, { timeout: POLL_MS * 6 });
    expect(alert).toHaveTextContent("Data already imported is kept");
    expect(alert).not.toHaveTextContent("Nothing was changed");
    expect(screen.getByRole("button", { name: "Skip" })).toBeInTheDocument();
  });

  it("asks a remote browser to approve on the computer instead of failing", async () => {
    const daemon = onboardingDaemon({ override: (call) => (call.path === "/v1/import/profiles" ? json(403, { code: "local_access_required", message: "local" }) : undefined) });
    open(daemon.client, "import");
    expect(await screen.findByText(/use openside on the computer that runs it/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Import" })).toBeNull();
  });

  it("offers to ask for access when this device lacks the admin scope", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ override: (call) => (call.path === "/v1/import/profiles" ? json(403, { code: "forbidden", message: "forbidden" }) : undefined) });
    open(daemon.client, "import");
    await user.click(await screen.findByRole("button", { name: "Ask for access" }));
    expect(await screen.findByText(/Waiting for you to approve it on the computer/)).toBeInTheDocument();
    expect(daemon.state.myRequest?.scopes).toEqual(["admin"]);
  });

  it("reports when no browser is installed and offers another look", async () => {
    open(onboardingDaemon({ profiles: [] }).client, "import");
    expect(await screen.findByText("No other browsers found")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Look again" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Skip" })).toBeInTheDocument();
  });
});

describe("passwords step", () => {
  const sheetTitle = "Import credentials to openside Vault";
  const chromePasswords = () => job({ state: "completed", completed_profiles: 1, total_profiles: 1, summary: batch(false, { profiles: ["chrome:Default"], what: ["passwords"] }) });

  async function openSheet(user: UserEvent) {
    await heading("Let openside fill in your passwords");
    await user.click(screen.getByRole("button", { name: "Import" }));
    return screen.findByRole("dialog", { name: sheetTitle });
  }

  it("imports a detected browser's passwords directly and then offers Continue", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked", jobPolls: [chromePasswords()] });
    open(daemon.client, "passwords");
    expect(screen.getByRole("button", { name: "Skip" })).toBeInTheDocument();
    const sheet = await openSheet(user);
    const next = within(sheet).getByRole("button", { name: "Next" });
    expect(next).toBeDisabled();
    const detected = within(sheet).getByRole("radiogroup", { name: "Detected on this device" });
    expect(await within(detected).findByRole("radio", { name: "Chrome · Work" })).toBeInTheDocument();
    expect(within(detected).getByRole("radio", { name: "Brave · Personal" })).toBeInTheDocument();
    // Firefox holds no saved passwords here, so it is not offered.
    expect(within(sheet).queryByRole("radio", { name: "Firefox · default" })).toBeNull();
    await user.click(within(detected).getByRole("radio", { name: "Chrome · Work" }));
    await user.click(next);

    const method = await screen.findByRole("dialog", { name: "Import Chrome passwords" });
    expect(within(method).getByRole("radio", { name: /^Import directly \(Recommended\)/ })).toBeChecked();
    expect(within(method).getByRole("radio", { name: /^Import \.csv file/ })).not.toBeChecked();
    await user.click(within(method).getByRole("button", { name: "Next" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull(), { timeout: POLL_MS * 4 });

    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "POST", path: "/v1/import", body: { profiles: ["chrome:Default"], what: ["passwords"] } })]);
    expect(screen.getByRole("status")).toHaveTextContent("12 passwords added to the vault.");
    expect(screen.queryByRole("button", { name: "Skip" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await heading("Choose your AI subscription")).toBeInTheDocument();
  });

  it("imports a password manager's CSV file read in the browser", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await within(sheet).findByRole("radio", { name: "Chrome · Work" });
    // With browsers detected, the other managers start folded away.
    expect(within(sheet).queryByRole("radio", { name: "Bitwarden" })).toBeNull();
    await user.click(within(sheet).getByRole("button", { name: "Other password managers" }));
    for (const name of ["1Password", "LastPass", "Apple Passwords", "Another app (CSV)"]) expect(within(sheet).getByRole("radio", { name })).toBeInTheDocument();
    await user.click(within(sheet).getByRole("radio", { name: "Bitwarden" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));

    const exportView = await screen.findByRole("dialog", { name: "Export from Bitwarden" });
    expect(exportView).toHaveTextContent("Bitwarden / Import .csv file");
    expect(exportView).toHaveTextContent(/Export vault/);
    expect(within(exportView).getByRole("button", { name: "Choose file" })).toBeEnabled();
    const text = "name,url,username,password\nx,https://x.test,me,secret\n";
    await user.upload(within(exportView).getByLabelText("Passwords CSV file"), csvFile(text));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "POST", path: "/v1/import/passwords", body: { csv: text } })]);
    expect(screen.getByRole("status")).toHaveTextContent("3 passwords added to the vault.");
    expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument();
  });

  it("sets the vault up inside the sheet and carries on with the interrupted import", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "uninitialized", jobPolls: [chromePasswords()] });
    open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await user.click(await within(sheet).findByRole("radio", { name: "Chrome · Work" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Import Chrome passwords" })).getByRole("button", { name: "Next" }));

    const vault = await screen.findByRole("dialog", { name: "Passwords go into an encrypted vault" });
    const form = within(vault).getByRole("form", { name: "Vault" });
    expect(within(vault).getByRole("button", { name: "Not now" })).toBeInTheDocument();
    await user.click(within(form).getByRole("button", { name: "Set up the vault" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull(), { timeout: POLL_MS * 4 });

    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import", "/v1/vault/setup", "/v1/import"]);
    expect(screen.getByRole("status")).toHaveTextContent("12 passwords added to the vault.");
  });

  it("unlocks a locked vault before a CSV import and keeps the file it already read", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "locked", vaultBackend: "passphrase", profiles: [] });
    open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    // No browser to offer: the managers start open.
    expect(await within(sheet).findByText("No browser with saved passwords was found on this computer.")).toBeInTheDocument();
    await user.click(within(sheet).getByRole("radio", { name: "Another app (CSV)" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    const text = "url,username,password\nhttps://a.example,me,pw\n";
    await user.upload(within(await screen.findByRole("dialog", { name: "Export from Another app (CSV)" })).getByLabelText("Passwords CSV file"), csvFile(text));

    const vault = await screen.findByRole("dialog", { name: "Unlock the vault to import passwords" });
    await user.type(within(vault).getByLabelText("Vault passphrase"), "correct horse");
    await user.click(within(vault).getByRole("button", { name: "Unlock the vault" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    const posted = writes(daemon.calls);
    expect(posted.map((call) => call.path)).toEqual(["/v1/import/passwords", "/v1/vault/unlock", "/v1/import/passwords"]);
    expect(posted.at(-1)!.body).toEqual({ csv: text });
    expect(screen.getByRole("status")).toHaveTextContent("3 passwords added to the vault.");
  });

  it("shows a failed direct import inside the sheet and stays open", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      vault: "unlocked",
      override: (call) => (call.path === "/v1/import" ? json(503, { code: "browser_unavailable", message: "Chrome is running." }) : undefined),
    });
    open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await user.click(await within(sheet).findByRole("radio", { name: "Chrome · Work" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Import Chrome passwords" })).getByRole("button", { name: "Next" }));

    const method = await screen.findByRole("dialog", { name: "Import Chrome passwords" });
    expect(await within(method).findByRole("alert")).toHaveTextContent("No Chromium-compatible browser was found");
    expect(screen.queryByText(/added to the vault/)).toBeNull();
  });

  it("closes with Not now without importing anything", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await within(sheet).findByRole("radio", { name: "Chrome · Work" });
    await user.click(within(sheet).getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(writes(daemon.calls)).toEqual([]);
    expect(screen.getByRole("button", { name: "Skip" })).toBeInTheDocument();
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("subscription step", () => {
  it("offers ChatGPT first, Claude next, and signs in to the chosen one", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "subscription");
    expect(await screen.findByRole("radio", { name: /ChatGPT/ })).toBeChecked();
    expect(screen.getByRole("radio", { name: /Claude/ })).not.toBeChecked();
    // Nothing works yet, and openside has no plan to fall back on: the step stays skippable.
    expect(await screen.findByRole("button", { name: "Skip" })).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: /Claude/ }));
    await user.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByRole("dialog", { name: "Connect Claude" })).toBeInTheDocument();
    await waitFor(() => expect(writes(daemon.calls).map((call) => call.path)).toContain("/v1/settings/oauth/accounts/anthropic"));
  });

  it("lists the other sign-ins under More options", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon().client, "subscription");
    await screen.findByRole("radio", { name: /ChatGPT/ });
    await user.click(screen.getByRole("button", { name: "Show more options" }));
    const more = await screen.findByRole("dialog", { name: "More options" });
    for (const name of ["GitHub Copilot", "Gemini", "Antigravity", "Use your own API key"]) expect(within(more).getByRole("button", { name })).toBeInTheDocument();
    await user.click(within(more).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("heading", { level: 1, name: "Choose your AI subscription" })).toBeInTheDocument();
  });

  it("connects an API key from More options and moves on to the end", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "subscription");
    await screen.findByRole("radio", { name: /ChatGPT/ });
    await user.click(screen.getByRole("button", { name: "Show more options" }));
    await user.click(within(await screen.findByRole("dialog", { name: "More options" })).getByRole("button", { name: "Use your own API key" }));
    const dialog = await screen.findByRole("dialog", { name: "Use your own API key" });
    await user.type(within(dialog).getByLabelText("API key"), "sk-ant-abcd1234");
    await user.click(within(dialog).getByRole("button", { name: "Save key" }));
    expect(await heading("You’re all set!")).toBeInTheDocument();
    expect(daemon.state.models.received).toContain("sk-ant-abcd1234");
    expect(screen.queryByText(/sk-ant-abcd1234/)).toBeNull();
  });

  it("says Continue instead of Skip once a provider already works", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ models: modelState({ keys: [keyInfo("openai", "k1", "Main", "wxyz")], pools: { openai: [{ source: "vault", account_id: "k1" }] } }) });
    open(daemon.client, "subscription");
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    expect(screen.queryByRole("button", { name: "Skip" })).toBeNull();
    expect(await heading("You’re all set!")).toBeInTheDocument();
  });
});

describe("done step", () => {
  it("asks about the default browser, opens the browser's settings and finishes the guide", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "done");
    expect(screen.getByRole("checkbox", { name: /Set openside as your default browser/ })).toBeChecked();
    // The launcher row needs the browser's install prompt, which this one does not offer.
    expect(screen.queryByRole("checkbox", { name: /Add openside to your app launcher/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Skip setup" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Get started" }));
    const dialog = await screen.findByRole("dialog", { name: "Make openside your default browser?" });
    expect(writes(daemon.calls)).toEqual([]);
    await user.click(within(dialog).getByRole("button", { name: "Open settings" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ method: "POST", path: "/v1/browser/person/tabs/command", body: { command: "create", url: "chrome://settings" } }),
        expect.objectContaining({ method: "PUT", path: "/v1/onboarding", body: { state: "completed" } }),
      ]),
    );
    expect(daemon.state.onboarding).toBe("completed");
  });

  it("still finishes when the browser is not connected, and says how to reach its settings", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      override: (call) => (call.path === "/v1/browser/person/tabs/command" ? json(409, { code: "person_browser_not_connected", message: "no" }) : undefined),
    });
    open(daemon.client, "done");
    await user.click(screen.getByRole("button", { name: "Get started" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Make openside your default browser?" })).getByRole("button", { name: "Open settings" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(await screen.findByText("Couldn't open chrome://settings. Paste it into the address bar to open it.")).toBeInTheDocument();
    expect(daemon.state.onboarding).toBe("completed");
  });

  it("finishes without opening anything when the person says Not now", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "done");
    await user.click(screen.getByRole("button", { name: "Get started" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Make openside your default browser?" })).getByRole("button", { name: "Not now" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "PUT", path: "/v1/onboarding", body: { state: "completed" } })]);
  });

  it("finishes directly when the default-browser option is unticked", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "done");
    await user.click(screen.getByRole("checkbox", { name: /Set openside as your default browser/ }));
    await user.click(screen.getByRole("button", { name: "Get started" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "PUT", path: "/v1/onboarding", body: { state: "completed" } })]);
  });

  it("offers the app launcher while the browser can install the app, and installs on Get started", async () => {
    const user = userEvent.setup();
    const install = vi.fn(async () => undefined);
    installOffer.mockReturnValue({ kind: "android", install });
    const daemon = onboardingDaemon();
    open(daemon.client, "done");
    expect(screen.getByRole("checkbox", { name: /Add openside to your app launcher/ })).toBeChecked();
    await user.click(screen.getByRole("checkbox", { name: /Set openside as your default browser/ }));
    await user.click(screen.getByRole("button", { name: "Get started" }));
    expect(install).toHaveBeenCalledOnce();
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(daemon.state.onboarding).toBe("completed");
  });

  it("saves the outcome before opening Appearance from its row", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "done");
    await user.click(screen.getByRole("button", { name: /^Customize appearance/ }));
    expect(await screen.findByText("settings")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent("/settings/appearance");
    expect(daemon.state.onboarding).toBe("completed");
  });

  it("opens the CLI setup from its row", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "done");
    await user.click(screen.getByRole("button", { name: /^Set up the openside CLI/ }));
    expect(await screen.findByText("settings")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent("/settings/developers");
    expect(daemon.state.onboarding).toBe("completed");
  });

  it("still leaves when the outcome cannot be saved, and says so", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ override: (call) => (call.method === "PUT" ? json(500, { code: "internal", message: "x" }) : undefined) });
    open(daemon.client, "done");
    await user.click(screen.getByRole("checkbox", { name: /Set openside as your default browser/ }));
    await user.click(screen.getByRole("button", { name: "Get started" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(guideDismissed()).toBe(true);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save that setup is finished. It may open again.");
  });
});

describe("outcome write guard", () => {
  const readFails = (status: number) =>
    onboardingDaemon({ override: (call) => (call.method === "GET" && call.path === "/v1/onboarding" ? json(status, { code: status === 403 ? "forbidden" : "internal", message: "x" }) : undefined) });

  it("dismisses quietly without writing when this device may not read the guide's state", async () => {
    const user = userEvent.setup();
    const daemon = readFails(403);
    open(daemon.client);
    await user.click(screen.getByRole("button", { name: "Skip setup" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([]);
    expect(guideDismissed()).toBe(true);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("does not write over a state it could not read, and says the outcome was not saved", async () => {
    const user = userEvent.setup();
    const daemon = readFails(500);
    open(daemon.client, "done");
    await user.click(screen.getByRole("checkbox", { name: /Set openside as your default browser/ }));
    await user.click(screen.getByRole("button", { name: "Get started" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([]);
    expect(guideDismissed()).toBe(true);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save that setup is finished. It may open again.");
  });
});

describe("passwords sheet hardening", () => {
  const sheetTitle = "Import credentials to openside Vault";
  const chromePasswords = () => job({ state: "completed", completed_profiles: 1, total_profiles: 1, summary: batch(false, { profiles: ["chrome:Default"], what: ["passwords"] }) });
  const directPosts = (calls: Call[]) => writes(calls).filter((call) => call.path === "/v1/import").length;
  const jobReads = (calls: Call[]) => calls.filter((call) => call.method === "GET" && call.path.startsWith("/v1/import/jobs/")).length;

  async function openSheet(user: UserEvent) {
    await heading("Let openside fill in your passwords");
    await user.click(screen.getByRole("button", { name: "Import" }));
    return screen.findByRole("dialog", { name: sheetTitle });
  }

  async function toMethod(user: UserEvent) {
    const sheet = await openSheet(user);
    await user.click(await within(sheet).findByRole("radio", { name: "Chrome · Work" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    return screen.findByRole("dialog", { name: "Import Chrome passwords" });
  }

  /** A file whose read waits for `release`, so a test can act while the sheet is still reading it. */
  function slowCsvFile(text: string) {
    const bytes = new TextEncoder().encode(text);
    const read = Promise.withResolvers<void>();
    const file = new File([bytes], "passwords.csv", { type: "text/csv" });
    Object.defineProperty(file, "arrayBuffer", { value: () => read.promise.then(() => bytes.buffer) });
    return { file, release: read.resolve };
  }

  it("starts one import and one poller for a double click and Enter on Next before the sheet re-renders", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked", jobPolls: [job(), chromePasswords()] });
    open(daemon.client, "passwords");
    const method = await toMethod(user);
    const next = within(method).getByRole("button", { name: "Next" });
    const direct = within(method).getByRole("radio", { name: /^Import directly/ });
    // One act scope keeps React from re-rendering between the three, like events queued faster than a frame.
    act(() => {
      next.click();
      next.click();
      direct.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull(), { timeout: POLL_MS * 4 });

    expect(directPosts(daemon.calls)).toBe(1);
    // One poller reads the running job once and the finished job once.
    expect(jobReads(daemon.calls)).toBe(2);
    expect(screen.getByRole("status")).toHaveTextContent("12 passwords added to the vault.");
  });

  it("keeps Next and Enter off while a refused direct import is still finding out about the vault", async () => {
    const user = userEvent.setup();
    const vaultAnswer = Promise.withResolvers<void>();
    const daemon = onboardingDaemon({
      vault: "locked",
      override: (call) => (call.method === "GET" && call.path === "/v1/vault" ? vaultAnswer.promise.then(() => json(200, { state: "locked", default_policy: "while_unlocked", backend: "secret_service" })) : undefined),
    });
    open(daemon.client, "passwords");
    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));

    // The refusal sends the sheet back to the method view while it asks the daemon about the vault.
    const again = await screen.findByRole("dialog", { name: "Import Chrome passwords" });
    const next = within(again).getByRole("button", { name: "Next" });
    await waitFor(() => expect(next).toBeDisabled());
    await user.click(next);
    within(again).getByRole("radio", { name: /^Import directly/ }).focus();
    await user.keyboard("{Enter}");
    expect(directPosts(daemon.calls)).toBe(1);

    vaultAnswer.resolve();
    expect(await screen.findByRole("dialog", { name: "Unlock the vault to import passwords" })).toBeInTheDocument();
    expect(directPosts(daemon.calls)).toBe(1);
  });

  it("sends only the first file when another is picked while the first is still being read", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked", profiles: [] });
    open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await user.click(await within(sheet).findByRole("radio", { name: "Another app (CSV)" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    const exportView = await screen.findByRole("dialog", { name: "Export from Another app (CSV)" });
    const first = "url,username,password\nhttps://a.example,me,first\n";
    const slow = slowCsvFile(first);
    await user.upload(within(exportView).getByLabelText("Passwords CSV file"), slow.file);
    expect(within(exportView).getByRole("button", { name: "Importing…" })).toBeDisabled();
    await user.upload(within(exportView).getByLabelText("Passwords CSV file"), csvFile("url,username,password\nhttps://b.example,me,second\n"));

    slow.release();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "POST", path: "/v1/import/passwords", body: { csv: first } })]);
  });

  it("is not closed by Escape or an outside click while a direct import runs", async () => {
    const user = userEvent.setup({ pointerEventsCheck: PointerEventsCheckLevel.Never });
    const posted = Promise.withResolvers<void>();
    const daemon = onboardingDaemon({
      vault: "unlocked",
      jobPolls: [chromePasswords()],
      override: (call) => (call.method === "POST" && call.path === "/v1/import" ? posted.promise.then(() => json(202, job({ total_profiles: 1 }))) : undefined),
    });
    open(daemon.client, "passwords");
    // Without an import running, both close the sheet.
    await openSheet(user);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await openSheet(user);
    await user.click(document.querySelector<HTMLElement>(".onb-sheet-backdrop")!);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));
    const running = await screen.findByRole("dialog", { name: "Importing Chrome passwords" });
    await user.keyboard("{Escape}");
    await user.click(document.querySelector<HTMLElement>(".onb-sheet-backdrop")!);
    expect(running).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Importing Chrome passwords" })).toBe(running);

    posted.resolve();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull(), { timeout: POLL_MS * 3 });
    expect(screen.getByRole("status")).toHaveTextContent("12 passwords added to the vault.");
  });

  it("stops polling the job when the guide goes away mid-import", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = onboardingDaemon({ vault: "unlocked", jobPolls: [job()] });
    const view = open(daemon.client, "passwords");
    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));
    await act(() => vi.advanceTimersByTimeAsync(POLL_MS));
    await waitFor(() => expect(jobReads(daemon.calls)).toBe(1));

    view.unmount();
    const seen = daemon.calls.length;
    await vi.advanceTimersByTimeAsync(POLL_MS * 5);
    expect(daemon.calls.length).toBe(seen);
    expect(jobReads(daemon.calls)).toBe(1);
  });

  it("carries on with the interrupted import exactly once after setting the vault up", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = onboardingDaemon({ vault: "uninitialized", jobPolls: [chromePasswords()] });
    open(daemon.client, "passwords");
    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));
    const vault = await screen.findByRole("dialog", { name: "Passwords go into an encrypted vault" });
    await user.click(within(vault).getByRole("button", { name: "Set up the vault" }));
    await act(() => vi.advanceTimersByTimeAsync(POLL_MS));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await act(() => vi.advanceTimersByTimeAsync(POLL_MS * 5));
    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import", "/v1/vault/setup", "/v1/import"]);
    expect(jobReads(daemon.calls)).toBe(1);
    expect(screen.getByRole("status")).toHaveTextContent("12 passwords added to the vault.");
  });

  it("forgets a CSV file once its vault stop is left, so nothing resends it without a new pick", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "locked", vaultBackend: "passphrase", profiles: [] });
    open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await user.click(await within(sheet).findByRole("radio", { name: "Another app (CSV)" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    const first = "url,username,password\nhttps://a.example,me,first\n";
    await user.upload(within(await screen.findByRole("dialog", { name: "Export from Another app (CSV)" })).getByLabelText("Passwords CSV file"), csvFile(first));

    // Back from the vault abandons the import: the export view waits for a file again.
    await user.click(within(await screen.findByRole("dialog", { name: "Unlock the vault to import passwords" })).getByRole("button", { name: "Back" }));
    const exportView = await screen.findByRole("dialog", { name: "Export from Another app (CSV)" });
    expect(within(exportView).getByRole("button", { name: "Choose file" })).toBeEnabled();
    const second = "url,username,password\nhttps://b.example,me,second\n";
    await user.upload(within(exportView).getByLabelText("Passwords CSV file"), csvFile(second));

    // Not now from the vault closes the sheet; unlocking elsewhere and reopening sends nothing by itself.
    await user.click(within(await screen.findByRole("dialog", { name: "Unlock the vault to import passwords" })).getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    daemon.state.vault = "unlocked";
    const reopened = await openSheet(user);
    await user.click(await within(reopened).findByRole("radio", { name: "Another app (CSV)" }));
    await user.click(within(reopened).getByRole("button", { name: "Next" }));
    expect(within(await screen.findByRole("dialog", { name: "Export from Another app (CSV)" })).getByRole("button", { name: "Choose file" })).toBeEnabled();

    expect(writes(daemon.calls).map((call) => [call.path, call.body])).toEqual([
      ["/v1/import/passwords", { csv: first }],
      ["/v1/import/passwords", { csv: second }],
    ]);
  });

  it("posts the direct import once, polls the job until it finishes, then stops", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = onboardingDaemon({ vault: "unlocked", jobPolls: [job(), job(), chromePasswords()] });
    open(daemon.client, "passwords");
    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));
    for (let poll = 0; poll < 3; poll += 1) await act(() => vi.advanceTimersByTimeAsync(POLL_MS));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await act(() => vi.advanceTimersByTimeAsync(POLL_MS * 5));
    const importCalls = daemon.calls.filter((call) => call.path === "/v1/import" || call.path.startsWith("/v1/import/jobs/")).map((call) => `${call.method} ${call.path}`);
    expect(importCalls).toEqual(["POST /v1/import", "GET /v1/import/jobs/job1", "GET /v1/import/jobs/job1", "GET /v1/import/jobs/job1"]);
    expect(screen.getByRole("status")).toHaveTextContent("12 passwords added to the vault.");
  });

  it("shows the local-only notice inside the sheet when a remote browser starts a direct import", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      vault: "unlocked",
      override: (call) => (call.method === "POST" && call.path === "/v1/import" ? json(403, { code: "local_access_required", message: "local" }) : undefined),
    });
    open(daemon.client, "passwords");
    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));

    const sheet = await screen.findByRole("dialog", { name: "Import Chrome passwords" });
    expect(await within(sheet).findByText("To import passwords, use openside on the computer that runs it")).toBeInTheDocument();
    expect(within(sheet).queryByRole("button", { name: "Next" })).toBeNull();
    expect(directPosts(daemon.calls)).toBe(1);
    expect(jobReads(daemon.calls)).toBe(0);
  });

  it("shows the local-only notice inside the sheet when a remote browser sends a CSV file", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      vault: "unlocked",
      profiles: [],
      override: (call) => (call.method === "POST" && call.path === "/v1/import/passwords" ? json(403, { code: "local_access_required", message: "local" }) : undefined),
    });
    open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await user.click(await within(sheet).findByRole("radio", { name: "Another app (CSV)" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    await user.upload(within(await screen.findByRole("dialog", { name: "Export from Another app (CSV)" })).getByLabelText("Passwords CSV file"), csvFile("url,username,password\nhttps://a.example,me,pw\n"));

    const exportView = await screen.findByRole("dialog", { name: "Export from Another app (CSV)" });
    expect(await within(exportView).findByText("To import passwords, use openside on the computer that runs it")).toBeInTheDocument();
    expect(within(exportView).queryByRole("button", { name: "Choose file" })).toBeNull();
    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import/passwords"]);
    expect(screen.queryByText(/added to the vault/)).toBeNull();
  });

  it("sends no CSV file when the guide goes away while the file is still being read", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked", profiles: [] });
    const view = open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await user.click(await within(sheet).findByRole("radio", { name: "Another app (CSV)" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    const exportView = await screen.findByRole("dialog", { name: "Export from Another app (CSV)" });
    const slow = slowCsvFile("url,username,password\nhttps://a.example,me,pw\n");
    await user.upload(within(exportView).getByLabelText("Passwords CSV file"), slow.file);
    // Busy from the pick itself, before the read finishes.
    expect(within(exportView).getByRole("button", { name: "Importing…" })).toBeDisabled();

    view.unmount();
    slow.release();
    const tick = Promise.withResolvers<void>();
    setTimeout(tick.resolve, 0);
    await act(() => tick.promise);
    expect(writes(daemon.calls)).toEqual([]);
  });

  /** Answers the vault's set-up or unlock only once `release` runs, as the daemon would then. */
  function holdVault(path: "/v1/vault/setup" | "/v1/vault/unlock", state: () => DaemonState) {
    const gate = Promise.withResolvers<void>();
    const override = (call: Call) =>
      call.method === "POST" && call.path === path
        ? gate.promise.then(() => {
            state().vault = "unlocked";
            return json(200, { state: "unlocked", default_policy: "while_unlocked", backend: state().vaultBackend });
          })
        : undefined;
    return { override, release: gate.resolve };
  }

  it("stays open while the vault is being set up, then carries on with the import", async () => {
    const user = userEvent.setup({ pointerEventsCheck: PointerEventsCheckLevel.Never });
    const held = holdVault("/v1/vault/setup", () => daemon.state);
    const daemon = onboardingDaemon({ vault: "uninitialized", jobPolls: [chromePasswords()], override: held.override });
    open(daemon.client, "passwords");
    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));
    const vault = await screen.findByRole("dialog", { name: "Passwords go into an encrypted vault" });
    await user.click(within(vault).getByRole("button", { name: "Set up the vault" }));

    const notNow = within(vault).getByRole("button", { name: "Not now" });
    const back = within(vault).getByRole("button", { name: "Back" });
    await waitFor(() => expect(notNow).toBeDisabled());
    expect(back).toBeDisabled();
    await user.keyboard("{Escape}");
    await user.click(document.querySelector<HTMLElement>(".onb-sheet-backdrop")!);
    await user.click(notNow);
    await user.click(back);
    expect(screen.getByRole("dialog", { name: "Passwords go into an encrypted vault" })).toBe(vault);

    held.release();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull(), { timeout: POLL_MS * 4 });
    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import", "/v1/vault/setup", "/v1/import"]);
    expect(screen.getByRole("status")).toHaveTextContent("12 passwords added to the vault.");
  });

  it("sends one vault set-up when the form is submitted twice before it settles", async () => {
    const user = userEvent.setup({ pointerEventsCheck: PointerEventsCheckLevel.Never });
    const held = holdVault("/v1/vault/setup", () => daemon.state);
    const daemon = onboardingDaemon({ vault: "uninitialized", jobPolls: [chromePasswords()], override: held.override });
    open(daemon.client, "passwords");
    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));
    const vault = await screen.findByRole("dialog", { name: "Passwords go into an encrypted vault" });
    const form = vault.querySelector<HTMLFormElement>("form.onb-vault")!;
    // Submit events reach the handler even once the button is disabled, so only the gate's own guard stops the second.
    fireEvent.submit(form);
    fireEvent.submit(form);
    held.release();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull(), { timeout: POLL_MS * 4 });
    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import", "/v1/vault/setup", "/v1/import"]);
  });

  it("starts no direct import when the vault set-up finishes after the guide went away", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const held = holdVault("/v1/vault/setup", () => daemon.state);
    const daemon = onboardingDaemon({ vault: "uninitialized", jobPolls: [chromePasswords()], override: held.override });
    const view = open(daemon.client, "passwords");
    const method = await toMethod(user);
    await user.click(within(method).getByRole("button", { name: "Next" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Passwords go into an encrypted vault" })).getByRole("button", { name: "Set up the vault" }));
    await waitFor(() => expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import", "/v1/vault/setup"]));

    view.unmount();
    held.release();
    await act(() => vi.advanceTimersByTimeAsync(POLL_MS * 5));
    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import", "/v1/vault/setup"]);
    expect(jobReads(daemon.calls)).toBe(0);
  });

  it("sends no CSV file when the vault unlock finishes after the guide went away", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const held = holdVault("/v1/vault/unlock", () => daemon.state);
    const daemon = onboardingDaemon({ vault: "locked", vaultBackend: "passphrase", profiles: [], override: held.override });
    const view = open(daemon.client, "passwords");
    const sheet = await openSheet(user);
    await user.click(await within(sheet).findByRole("radio", { name: "Another app (CSV)" }));
    await user.click(within(sheet).getByRole("button", { name: "Next" }));
    await user.upload(within(await screen.findByRole("dialog", { name: "Export from Another app (CSV)" })).getByLabelText("Passwords CSV file"), csvFile("url,username,password\nhttps://a.example,me,pw\n"));
    const vault = await screen.findByRole("dialog", { name: "Unlock the vault to import passwords" });
    await user.type(within(vault).getByLabelText("Vault passphrase"), "correct horse");
    await user.click(within(vault).getByRole("button", { name: "Unlock the vault" }));
    await waitFor(() => expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import/passwords", "/v1/vault/unlock"]));

    view.unmount();
    held.release();
    await act(() => vi.advanceTimersByTimeAsync(POLL_MS * 5));
    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import/passwords", "/v1/vault/unlock"]);
  });
});

describe("outcome compare-and-set", () => {
  // GET still says pending, but another device records its outcome before this device's PUT lands.
  const lostRace = (code = "onboarding_already_set") =>
    onboardingDaemon({
      override: (call) =>
        call.method === "PUT" && call.path === "/v1/onboarding"
          ? json(409, { code, message: "conflict", state: "completed", updated_at: "2026-10-12T00:00:00Z" })
          : undefined,
    });

  it("counts an outcome another device already recorded as done: home, no alert, nothing dismissed locally", async () => {
    const user = userEvent.setup();
    const daemon = lostRace();
    open(daemon.client);
    await user.click(screen.getByRole("button", { name: "Skip setup" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/$/);
    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "PUT", path: "/v1/onboarding", body: { state: "skipped" } })]);
    expect(guideDismissed()).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("still opens the row's destination when the outcome was already recorded", async () => {
    const user = userEvent.setup();
    const daemon = lostRace();
    open(daemon.client, "done");
    await user.click(screen.getByRole("button", { name: /^Customize appearance/ }));
    expect(await screen.findByText("settings")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent("/settings/appearance");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("treats any other conflict as a failed save", async () => {
    const user = userEvent.setup();
    const daemon = lostRace("invalid_transition");
    open(daemon.client);
    await user.click(screen.getByRole("button", { name: "Skip setup" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(guideDismissed()).toBe(true);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save that setup is finished. It may open again.");
  });
});

function BackButton() {
  const navigate = useNavigate();
  return <button onClick={() => void navigate(-1)}>history back</button>;
}

describe("step canonicalization", () => {
  it.each(["bogus", "agents", "welcome"])("replaces ?step=%s with the bare /welcome instead of adding a history entry", async (step) => {
    const user = userEvent.setup();
    render(
      <ClientContext.Provider value={onboardingDaemon().client}>
        <ToastProvider>
          <MemoryRouter initialEntries={["/", `/welcome?step=${step}`]} initialIndex={1}>
            <Where />
            <BackButton />
            <Routes>
              <Route path="/welcome" element={<OnboardingPage />} />
              <Route path="/" element={<p>home</p>} />
            </Routes>
          </MemoryRouter>
        </ToastProvider>
      </ClientContext.Provider>,
    );
    expect(await heading("An AI assistant that works in your browser.")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent(/^\/welcome$/));
    // A pushed entry would leave the stale URL behind; one Back must land on the page before the guide.
    await user.click(screen.getByRole("button", { name: "history back" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
  });
});
