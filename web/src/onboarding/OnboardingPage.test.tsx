import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, type Client } from "../api/client";
import type { ImportBatchReport } from "../api/types";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { agentSummary } from "../test/agents";
import { json, type Call } from "../test/fakeDaemon";
import { keyInfo, modelState } from "../models/testDaemon";
import { offStatus, tailscaleOn } from "../remote/testDaemon";
import { guideDismissed } from "./dismissed";
import { OnboardingPage } from "./OnboardingPage";
import { batch, job, onboardingDaemon } from "./testDaemon";
import { POLL_MS } from "./ImportStep";

const grant = vi.hoisted(() => vi.fn());
const pair = vi.hoisted(() => vi.fn());
const deliver = vi.hoisted(() => vi.fn());
vi.mock("../import/delivery", async (original) => ({ ...(await original<typeof import("../import/delivery")>()), requestImportPermissions: grant, deliverImport: deliver }));
vi.mock("../desktopNotify", async (original) => ({ ...(await original<typeof import("../desktopNotify")>()), pairExtension: pair }));

function Where() {
  const location = useLocation();
  return <p data-testid="where">{location.pathname + location.search}</p>;
}

function open(client: Client, step = "import") {
  return render(
    <ClientContext.Provider value={client}>
      <MemoryRouter initialEntries={[step === "import" ? "/welcome" : `/welcome?step=${step}`]}>
        <Where />
        <Routes>
          <Route path="/welcome" element={<OnboardingPage />} />
          <Route path="/vault" element={<p>vault dialog</p>} />
          <Route path="/" element={<p>home</p>} />
        </Routes>
      </MemoryRouter>
    </ClientContext.Provider>,
  );
}

const writes = (calls: Call[]) => calls.filter((call) => call.method !== "GET");

beforeEach(async () => {
  localStorage.clear();
  grant.mockReset();
  pair.mockReset();
  const actual = await vi.importActual<typeof import("../import/delivery")>("../import/delivery");
  deliver.mockReset().mockImplementation(actual.deliverImport);
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("order and navigation", () => {
  it("starts with the browser import and walks forward and back through the URL", async () => {
    const user = userEvent.setup();
    const { client } = onboardingDaemon({ vault: "unlocked" });
    open(client);
    expect(screen.getByRole("heading", { name: "Bring your browser with you" })).toHaveFocus();
    expect(screen.getByRole("list", { name: "Step 1 of 6: Browser" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Skip for now" }));
    expect(await screen.findByRole("heading", { name: "Connect a model" })).toHaveFocus();
    expect(screen.getByTestId("where")).toHaveTextContent("/welcome?step=models");
    expect(screen.getByRole("list", { name: "Step 2 of 6: Models" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(await screen.findByRole("heading", { name: "Bring your browser with you" })).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/welcome$/);
  });

  it("goes browser, models, agents, phone, assistant, done", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon().client);
    const headings: string[] = [];
    for (let step = 0; step < 5; step++) {
      headings.push(screen.getByRole("heading", { level: 1 }).textContent ?? "");
      await user.click(await screen.findByRole("button", { name: /Continue|Skip for now|Skip this step/ }));
      await screen.findByRole("heading", { level: 1, name: (name) => name !== headings.at(-1) });
    }
    expect(headings).toEqual(["Bring your browser with you", "Connect a model", "Connect your agents", "Pair your phone", "Name your assistant"]);
    expect(screen.getByRole("heading", { name: "You’re all set" })).toBeInTheDocument();
  });

  it("starts at the first step for an unknown ?step", () => {
    open(onboardingDaemon().client, "bogus");
    expect(screen.getByRole("heading", { name: "Bring your browser with you" })).toBeInTheDocument();
  });

  it("has no welcome or vault step any more", () => {
    open(onboardingDaemon().client, "vault");
    expect(screen.getByRole("heading", { name: "Bring your browser with you" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Welcome to openside" })).toBeNull();
  });

  it("skips the whole guide once and records it on the daemon", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "models");
    await user.click(screen.getByRole("button", { name: "Skip setup" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "PUT", path: "/v1/onboarding", body: { state: "skipped" } })]);
    expect(daemon.state.onboarding).toBe("skipped");
  });

  it("leaves the stored outcome alone when the guide was reopened from Settings", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ onboarding: "completed" });
    open(daemon.client, "models");
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

describe("models step", () => {
  it("leads a user with no usable model to connect one, and lets them skip", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "models");
    const anthropic = await screen.findByRole("region", { name: "Provider Anthropic" });
    expect(within(anthropic).getByText("Not connected")).toBeInTheDocument();
    expect(within(anthropic).getByRole("button", { name: "Add Claude Code account" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("No model is ready yet");
    expect(screen.getByRole("button", { name: "Skip this step" })).toBeInTheDocument();

    await user.click(within(anthropic).getByRole("button", { name: "Add Anthropic API key" }));
    await user.type(screen.getByLabelText("Name"), "Personal");
    await user.type(screen.getByLabelText("API key"), "sk-ant-abcd1234");
    await user.click(screen.getByRole("button", { name: "Save key" }));
    await waitFor(() => expect(screen.getByRole("region", { name: "Provider Anthropic" }).querySelector(".section-list-actions .badge")).toHaveTextContent("Ready"));
    expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument();
    expect(screen.queryByText(/sk-ant-abcd1234/)).toBeNull();
  });

  it("shows the recap row once a model is ready", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ models: modelState({ keys: [keyInfo("openai", "k1", "Main", "wxyz")], pools: { openai: [{ source: "vault", account_id: "k1" }] } }) });
    open(daemon.client, "models");
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    for (let step = 0; step < 3; step++) await user.click(screen.getByRole("button", { name: /Continue|Skip this step/ }));
    expect(await screen.findByRole("heading", { name: "You’re all set" })).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Summary" })).getByText("Models").closest("li")).toHaveTextContent("OpenAI ready");
  });
});

describe("agents step", () => {
  it("lists agents by what was found and installs the skill for one", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "agents");
    const list = await screen.findByRole("list", { name: "Agents" });
    const claude = within(list).getByText("Claude Code").closest("article") as HTMLElement;
    const codex = within(list).getByText("Codex").closest("article") as HTMLElement;
    const omp = within(list).getByText("omp").closest("article") as HTMLElement;
    expect(within(claude).getByText("Found")).toBeInTheDocument();
    expect(within(codex).getByText("Not found")).toBeInTheDocument();
    expect(within(omp).getByText("Skill installed")).toBeInTheDocument();
    expect(within(omp).queryByRole("button")).toBeNull();
    expect(within(claude).getByText("~/.claude/skills")).toBeInTheDocument();
    expect(within(codex).getByRole("button", { name: "Install the browser skill for Codex" })).toHaveTextContent("Install anyway");

    // Installed already (omp) counts as done, so Continue replaces Skip.
    expect(await screen.findByRole("button", { name: "Continue" })).toBeInTheDocument();
    await user.click(within(claude).getByRole("button", { name: "Install the browser skill for Claude Code" }));
    await waitFor(() => expect(within(list).getAllByText("Skill installed")).toHaveLength(2));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/external-agents/claude/browser-skill", body: { force: false } }));
  });

  it("offers Skip while nothing is installed and shows an install error", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      agents: [{ id: "claude", label: "Claude Code", detected: true, skill_installed: false, skills_dir: "/x/skills", custom: false }],
      override: (call) => (call.method === "POST" ? json(400, { code: "skills_dir_unknown", message: "x" }) : undefined),
    });
    open(daemon.client, "agents");
    expect(await screen.findByRole("button", { name: "Skip this step" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Install the browser skill for Claude Code" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("does not know where this agent keeps its skills");
    expect(screen.getByRole("button", { name: "Skip this step" })).toBeInTheDocument();
  });

  it("treats a skill installed another way in the meantime as installed, not as an error", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "agents");
    await screen.findByRole("list", { name: "Agents" });
    daemon.state.agents[0]!.skill_installed = true;
    await user.click(screen.getByRole("button", { name: "Install the browser skill for Codex" }));
    await waitFor(() => expect(screen.getAllByText("Skill installed").length).toBeGreaterThanOrEqual(3));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("offers to ask for access when this device lacks the admin scope", async () => {
    const daemon = onboardingDaemon({ override: (call) => (call.path === "/v1/external-agents" ? json(403, { code: "forbidden", message: "no" }) : undefined) });
    open(daemon.client, "agents");
    expect(await screen.findByRole("region", { name: "Access needed" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Ask for access" })).toBeInTheDocument();
  });
});

describe("import step", () => {
  const profileBox = (name: string, checked = true) => screen.findByRole("checkbox", { name, checked });

  it("preselects every browser and kind, with counts per profile, and a single primary button", async () => {
    const daemon = onboardingDaemon({ vault: "unlocked" });
    open(daemon.client);
    for (const name of ["Import Chrome · Work", "Import Brave · Personal", "Import Firefox · default"]) expect(await profileBox(name)).toBeChecked();
    const chrome = screen.getByRole("checkbox", { name: "Import Chrome · Work" }).closest("label")!;
    expect(chrome).toHaveTextContent("work@example.test");
    expect(chrome).toHaveTextContent("1,200 history entries · 31 bookmarks · 77 cookies · 12 passwords · 3 open tabs");
    expect(screen.getByRole("checkbox", { name: "Import Firefox · default" }).closest("label")).toHaveTextContent("80 history entries · 3 bookmarks");
    for (const { label } of [{ label: "History" }, { label: "Bookmarks" }, { label: "Cookies" }, { label: "Passwords" }, { label: "Autofill" }, { label: "Open tabs" }, { label: "Search engines" }, { label: "Site permissions" }, { label: "Extensions" }]) {
      expect(screen.getByRole("checkbox", { name: new RegExp(`^${label}`) })).toBeChecked();
    }
    expect(screen.getByRole("button", { name: "Import everything" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Skip for now" })).toBeInTheDocument();
    expect(screen.queryByText(/openside import/)).toBeNull();
    // Looking before writing is the daemon's preview, never a write.
    expect(writes(daemon.calls).map((call) => call.path)).toEqual(["/v1/import/preview"]);
  });

  it("runs everything with one click, shows progress, then the summary and a Continue button", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      vault: "unlocked",
      jobPolls: [
        job({ completed_profiles: 1, current_profile: "brave:Default" }),
        job({ state: "completed", completed_profiles: 3, summary: { ...batchSummary(), restore_tabs: [{ profile_id: "chrome:Default", url: "https://example.test/a", title: "Example A" }] } }),
      ],
    });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
    expect(writes(daemon.calls).at(-1)).toMatchObject({ method: "POST", path: "/v1/import", body: { profiles: null, what: null } });
    const meter = await screen.findByRole("progressbar", { name: "Import progress" });
    expect(meter).toHaveAttribute("aria-valuenow", "0");
    expect(screen.getByRole("status")).toHaveTextContent("Importing…");
    await waitFor(() => expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "1"), { timeout: POLL_MS * 4 });
    expect(screen.getByRole("status")).toHaveTextContent("Brave · Personal · 1 of 3 profiles");

    const summary = await screen.findByRole("list", { name: "Imported" }, { timeout: POLL_MS * 6 });
    expect(summary).toHaveTextContent("History1,580");
    expect(screen.getByRole("status")).toHaveTextContent("Imported from 3 profiles");
    expect(screen.getByRole("link", { name: "Example A" })).toHaveAttribute("href", "https://example.test/a");
    expect(screen.queryByRole("button", { name: "Import everything" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByRole("heading", { name: "Connect a model" })).toBeInTheDocument();
  });

  it("lists extensions as names with store links, search engines and permissions, and never installs anything", async () => {
    const user = userEvent.setup();
    const details = {
      extensions: [{ profile_id: "chrome:Default", id: "abc", name: "Dark Reader", store_url: "https://store.example/abc" }, { profile_id: "chrome:Default", id: "def", name: "Local tool", store_url: null }],
      search_engines: [{ profile_id: "chrome:Default", name: "DuckDuckGo", keyword: "ddg", url: "https://duckduckgo.com/?q=%s" }],
      permissions: [{ profile_id: "chrome:Default", origin: "https://meet.example", permission: "camera", setting: "allow" }],
    };
    const daemon = onboardingDaemon({ vault: "unlocked", jobPolls: [job({ state: "completed", completed_profiles: 3, summary: { ...batchSummary(), ...details } })] });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
    await screen.findByRole("list", { name: "Imported" }, { timeout: POLL_MS * 6 });
    expect(screen.getByRole("link", { name: "Dark Reader" })).toHaveAttribute("href", "https://store.example/abc");
    expect(screen.getByText("Local tool").closest("a")).toBeNull();
    expect(screen.getByText("DuckDuckGo · ddg")).toBeInTheDocument();
    expect(screen.getByText("https://meet.example · camera · allow")).toBeInTheDocument();
  });

  it("remembers what was brought over for the final recap", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon({ vault: "unlocked" }).client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
    await user.click(await screen.findByRole("button", { name: "Continue" }, { timeout: POLL_MS * 6 }));
    for (let step = 0; step < 4; step++) await user.click(await screen.findByRole("button", { name: /Continue|Skip this step/ }));
    expect(within(await screen.findByRole("list", { name: "Summary" })).getByText("Browser data").closest("li")).toHaveTextContent("Brought over: 3 profiles · 1,715 items");
  });

  it("hands bookmarks and history to the extension after the import", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    deliver.mockResolvedValue({ delivered: 1, pending: 0, extensionMissing: false });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
    expect(await screen.findByText("Your bookmarks and history are in this browser.", {}, { timeout: POLL_MS * 6 })).toBeInTheDocument();
    expect(deliver).toHaveBeenCalledWith(daemon.client, expect.any(Function));
  });

  it("offers 'Sign in to your sites in openside' when cookies need the person's approval, and delivers after it", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    deliver.mockRejectedValue(new ApiError(0, "import_delivery_permission_required", "Allow the sites first."));
    grant.mockResolvedValue({ delivered: 1, pending: 0, extensionMissing: false });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
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
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
    await user.click(await screen.findByRole("button", { name: "Connect the openside extension" }, { timeout: POLL_MS * 6 }));
    expect(pair).toHaveBeenCalledOnce();
    expect(await screen.findByText("Your bookmarks and history are in this browser.")).toBeInTheDocument();
    expect(deliver).toHaveBeenCalledTimes(2);
  });

  it("says to install the extension, not that something failed, when none answers", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    deliver.mockResolvedValue({ delivered: 0, pending: 1, extensionMissing: true });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
    expect(await screen.findByText(/Install the openside extension/, {}, { timeout: POLL_MS * 6 })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("imports only the chosen profiles and kinds", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    open(daemon.client);
    await user.click(await profileBox("Import Firefox · default"));
    await user.click(screen.getByText("Choose what to bring"));
    await user.click(screen.getByRole("checkbox", { name: /^Cookies/ }));
    expect(screen.getByRole("button", { name: "Import selected" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Import selected" }));
    expect(writes(daemon.calls).at(-1)).toMatchObject({ path: "/v1/import" });
    const body = writes(daemon.calls).at(-1)!.body as { profiles: string[]; what: string[] };
    expect(body.profiles).toEqual(["chrome:Default", "brave:Default"]);
    expect(body.what).not.toContain("cookies");
    expect(body.what).toContain("passwords");
  });

  it("will not start with nothing selected", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon({ vault: "unlocked" }).client);
    for (const name of ["Import Chrome · Work", "Import Brave · Personal", "Import Firefox · default"]) await user.click(await profileBox(name));
    expect(screen.getByRole("button", { name: "Import selected" })).toBeDisabled();
    expect(screen.getByText("Choose at least one browser.")).toBeInTheDocument();
  });

  it("sets the vault up inline when passwords need it, and imports without leaving", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "uninitialized" });
    open(daemon.client);
    const form = await screen.findByRole("form", { name: "Vault" });
    expect(form).toHaveTextContent("Passwords go into an encrypted vault");
    expect(within(form).queryByRole("link")).toBeNull();
    expect(screen.queryByText(/openside vault/)).toBeNull();
    expect(screen.getByRole("button", { name: "Import everything" })).toBeDisabled();
    await user.click(within(form).getByRole("button", { name: "Set up the vault" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "Vault" })).toBeNull());
    expect(writes(daemon.calls).map((call) => call.path)).toContain("/v1/vault/setup");
    expect(await screen.findByRole("button", { name: "Import everything" })).toBeEnabled();
  });

  it("asks for a passphrase when the computer has no keyring", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "uninitialized", vaultBackend: "passphrase" });
    open(daemon.client);
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

  it("can leave passwords out instead of setting the vault up", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "locked" });
    open(daemon.client);
    expect(await screen.findByRole("form", { name: "Vault" })).toHaveTextContent("Unlock the vault to import passwords");
    await user.click(screen.getByRole("button", { name: "Import without passwords" }));
    expect(screen.queryByRole("form", { name: "Vault" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Import selected" }));
    expect((writes(daemon.calls).at(-1)!.body as { what: string[] }).what).not.toContain("passwords");
  });

  it("imports a passwords CSV from a file, secondary to the main import", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked" });
    open(daemon.client);
    await screen.findByRole("button", { name: "Import everything" });
    const file = new File(["name,url,username,password\nx,https://x.test,me,secret\n"], "passwords.csv", { type: "text/csv" });
    await user.upload(screen.getByLabelText("Passwords CSV file"), file);
    expect(await screen.findByText(/3 passwords added to the vault/)).toBeInTheDocument();
    expect(writes(daemon.calls).at(-1)).toMatchObject({ path: "/v1/import/passwords", body: { csv: expect.stringContaining("https://x.test") } });
  });

  it("explains a failed import and offers to leave cookies out", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      vault: "unlocked",
      override: (call) => (call.path === "/v1/import" ? json(503, { code: "browser_unavailable", message: "Chrome is running." }) : undefined),
    });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("No Chromium-compatible browser was found");
    await user.click(screen.getByRole("button", { name: "Leave cookies out" }));
    expect(screen.getByRole("button", { name: "Import selected" })).toBeEnabled();
  });

  it("does not claim nothing was changed when a job fails after some profiles finished", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ vault: "unlocked", jobPolls: [job({ state: "failed", completed_profiles: 1, summary: null, error: null })] });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Import everything" }));
    const alert = await screen.findByRole("alert", {}, { timeout: POLL_MS * 6 });
    expect(alert).toHaveTextContent("Data already imported is kept");
    expect(alert).not.toHaveTextContent("Nothing was changed");
  });

  it("asks a remote browser to approve on the computer instead of failing", async () => {
    const daemon = onboardingDaemon({ override: (call) => (call.path === "/v1/import/profiles" ? json(403, { code: "local_access_required", message: "local" }) : undefined) });
    open(daemon.client);
    expect(await screen.findByText(/use openside on the computer that runs it/)).toBeInTheDocument();
  });

  it("offers to ask for access when this device lacks the admin scope", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ override: (call) => (call.path === "/v1/import/profiles" ? json(403, { code: "forbidden", message: "forbidden" }) : undefined) });
    open(daemon.client);
    await user.click(await screen.findByRole("button", { name: "Ask for access" }));
    expect(await screen.findByText(/Waiting for you to approve it on the computer/)).toBeInTheDocument();
    expect(daemon.state.myRequest?.scopes).toEqual(["admin"]);
  });

  it("reports when no browser is installed and offers another look", async () => {
    open(onboardingDaemon({ profiles: [] }).client);
    expect(await screen.findByText("No other browsers found")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Look again" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Skip for now" })).toBeInTheDocument();
  });
});

function batchSummary(): ImportBatchReport {
  return { ...batch(false), dry_run: false };
}

describe("pair step", () => {
  it("offers to ask for access, never a command, when this device may not set up pairing", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "pair");
    expect(await screen.findByRole("region", { name: "Access needed" })).toBeInTheDocument();
    expect(screen.queryByText(/openside pair/)).toBeNull();
    await user.click(await screen.findByRole("button", { name: "Ask for access" }));
    expect(await screen.findByText(/Waiting for you to approve it on the computer/)).toBeInTheDocument();
    expect(daemon.state.myRequest).toMatchObject({ scopes: ["admin"] });
  });

  it("starts the phone pairing from the screen once access is there", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon({ remote: tailscaleOn() }).client, "pair");
    await user.click(await screen.findByRole("button", { name: "Show QR code" }));
    expect(await screen.findByRole("img", { name: /QR code to connect/ })).toBeInTheDocument();
  });
});

describe("pair step with remote access", () => {
  it("shows the QR flow and recognises the phone", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = onboardingDaemon({ remote: tailscaleOn() });
    open(daemon.client, "pair");
    await user.click(await screen.findByRole("button", { name: "Show QR code" }));
    expect(await screen.findByRole("img", { name: /QR code to connect/ })).toBeInTheDocument();
    expect(screen.queryByText(/openside pair/)).toBeNull();

    daemon.state.devices = [...daemon.state.devices, { device_id: "d2", name: "My phone", kind: "person_device", scopes: ["read"], created_at: "2026-10-13T00:00:00Z" }];
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_100);
    });
    expect(await screen.findByText("My phone is connected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument();
  });

  it("sends the person to Remote access when no connection is on", async () => {
    open(onboardingDaemon({ remote: offStatus() }).client, "pair");
    expect(await screen.findByRole("link", { name: "Set up remote access" })).toHaveAttribute("href", "/remote");
  });

  it("explains that pairing happens on the computer, without a command, for a browser that is not on it", async () => {
    open(onboardingDaemon({ remote: tailscaleOn({ pairing_allowed: false }) }).client, "pair");
    expect(await screen.findByText(/use openside on the computer that runs it/)).toBeInTheDocument();
    expect(screen.queryByText(/openside pair/)).toBeNull();
  });
});

describe("assistant step", () => {
  function openWithChief(client: Client) {
    const team = { agents: [agentSummary("chief", "Juno")], error: null, offline: false, refresh: vi.fn() };
    render(
      <ClientContext.Provider value={client}>
        <AgentsContext.Provider value={team}>
          <MemoryRouter initialEntries={["/welcome?step=assistant"]}>
            <Routes>
              <Route path="/welcome" element={<OnboardingPage />} />
            </Routes>
          </MemoryRouter>
        </AgentsContext.Provider>
      </ClientContext.Provider>,
    );
    return team;
  }

  it("starts from the current name and renames the assistant", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ override: (call) => (call.method === "PATCH" ? json(200, agentSummary("chief", "Mo")) : undefined) });
    const team = openWithChief(daemon.client);
    const name = screen.getByRole("textbox", { name: "Name" });
    expect(name).toHaveValue("Juno");
    await user.clear(name);
    await user.type(name, "Mo");
    await user.click(screen.getByRole("button", { name: "Call them Mo" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument());
    expect(writes(daemon.calls).find((call) => call.method === "PATCH")).toMatchObject({ path: "/v1/agents/chief", body: { name: "Mo", initials: "M" } });
    expect(team.refresh).toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(within(await screen.findByRole("list", { name: "Summary" })).getByText("Assistant").closest("li")).toHaveTextContent("Mo");
  });

  it("keeps the name without a request", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    openWithChief(daemon.client);
    await user.click(screen.getByRole("button", { name: "Keep Juno" }));
    expect(await screen.findByRole("button", { name: "Continue" })).toBeInTheDocument();
    expect(writes(daemon.calls).filter((call) => call.path.startsWith("/v1/agents"))).toEqual([]);
  });
});

describe("done step", () => {
  it("recaps what was set up and finishes the guide", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "agents");
    await user.click(await screen.findByRole("button", { name: "Install the browser skill for Claude Code" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument());
    // Agents (done), then the phone and the assistant's name are passed over.
    for (let step = 0; step < 3; step++) await user.click(screen.getByRole("button", { name: /Continue|Skip this step/ }));
    expect(await screen.findByRole("heading", { name: "You’re all set" })).toBeInTheDocument();
    const recap = screen.getByRole("list", { name: "Summary" });
    expect(within(recap).getByText("Browser skill for Claude Code, omp")).toBeInTheDocument();
    expect(within(recap).getByText("Browser data").closest("li")).toHaveTextContent("Skipped");
    expect(screen.queryByRole("button", { name: "Skip setup" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Open openside" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(daemon.state.onboarding).toBe("completed");
  });

  it("still leaves when the outcome cannot be saved, and says so", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ override: (call) => (call.method === "PUT" ? json(500, { code: "internal", message: "x" }) : undefined) });
    open(daemon.client, "done");
    await user.click(screen.getByRole("button", { name: "Open openside" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(guideDismissed()).toBe(true);
  });
});
