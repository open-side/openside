import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Client } from "../api/client";
import { ClientContext } from "../context";
import { json } from "../test/fakeDaemon";
import { guideDismissed } from "./dismissed";
import { OnboardingPage } from "./OnboardingPage";
import { onboardingDaemon, report } from "./testDaemon";

function Where() {
  const location = useLocation();
  return <p data-testid="where">{location.pathname + location.search}</p>;
}

function open(client: Client, step = "welcome") {
  return render(
    <ClientContext.Provider value={client}>
      <MemoryRouter initialEntries={[step === "welcome" ? "/welcome" : `/welcome?step=${step}`]}>
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

const writes = (calls: { method: string }[]) => calls.filter((call) => call.method !== "GET");

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("welcome and navigation", () => {
  it("introduces the steps and walks forward and back through the URL", async () => {
    const user = userEvent.setup();
    const { client } = onboardingDaemon();
    open(client);
    expect(screen.getByRole("heading", { name: "Welcome to openside" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Get started" }));
    expect(await screen.findByRole("heading", { name: "Connect your agents" })).toHaveFocus();
    expect(screen.getByTestId("where")).toHaveTextContent("/welcome?step=agents");
    expect(screen.getByRole("list", { name: "Step 2 of 6: Agents" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(await screen.findByRole("heading", { name: "Welcome to openside" })).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/welcome$/);
  });

  it("starts at the first step for an unknown ?step", () => {
    open(onboardingDaemon().client, "bogus");
    expect(screen.getByRole("heading", { name: "Welcome to openside" })).toBeInTheDocument();
  });

  it("skips the whole guide once and records it on the daemon", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "import");
    await user.click(screen.getByRole("button", { name: "Skip setup" }));
    expect(await screen.findByText("home")).toBeInTheDocument();
    expect(writes(daemon.calls)).toEqual([expect.objectContaining({ method: "PUT", path: "/v1/onboarding", body: { state: "skipped" } })]);
    expect(daemon.state.onboarding).toBe("skipped");
  });

  it("leaves the stored outcome alone when the guide was reopened from Settings", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({ onboarding: "completed" });
    open(daemon.client, "vault");
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
      agents: [{ id: "claude", label: "Claude Code", detected: true, skill_installed: false, skills_dir: "/x/skills" }],
      override: (call) => (call.method === "POST" ? json(400, { code: "skills_dir_unknown", message: "x" }) : undefined),
    });
    open(daemon.client, "agents");
    expect(await screen.findByRole("button", { name: "Skip this step" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Install the browser skill for Claude Code" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("does not know where this agent keeps its skills");
    expect(screen.getByRole("button", { name: "Skip this step" })).toBeInTheDocument();
  });

  it("treats a skill installed from the terminal in the meantime as installed, not as an error", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "agents");
    await screen.findByRole("list", { name: "Agents" });
    daemon.state.agents[0]!.skill_installed = true;
    await user.click(screen.getByRole("button", { name: "Install the browser skill for Codex" }));
    await waitFor(() => expect(screen.getAllByText("Skill installed").length).toBeGreaterThanOrEqual(3));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("explains a missing admin scope", async () => {
    const daemon = onboardingDaemon({ override: (call) => (call.path === "/v1/external-agents" ? json(403, { code: "forbidden", message: "no" }) : undefined) });
    open(daemon.client, "agents");
    expect(await screen.findByText("This device needs the admin scope")).toBeInTheDocument();
  });
});

describe("import step", () => {
  it("only writes after a dry-run preview of the same selection", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "import");
    expect(await screen.findByRole("radio", { name: /Chrome · Work/, checked: true })).toBeChecked();
    for (const name of ["History", "Bookmarks", "Cookies"]) expect(screen.getByRole("checkbox", { name })).toBeChecked();
    const importButton = screen.getByRole("button", { name: "Import" });
    expect(importButton).toBeDisabled();
    expect(screen.getByRole("button", { name: "Skip this step" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const preview = await screen.findByRole("list", { name: "Preview" });
    expect(preview).toHaveTextContent("1,200 history entries to import · 4 skipped");
    expect(preview).toHaveTextContent("77 cookies to import · 3 skipped");
    expect(writes(daemon.calls).at(-1)).toMatchObject({ path: "/v1/import", body: { profile: "chrome:Default", what: ["history", "bookmarks", "cookies"], dry_run: true } });
    expect(importButton).toBeEnabled();

    // A different selection is a different import: it needs its own preview.
    await user.click(screen.getByRole("checkbox", { name: "Cookies" }));
    expect(screen.queryByRole("list", { name: "Preview" })).toBeNull();
    expect(screen.getByRole("button", { name: "Import" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByRole("list", { name: "Preview" });
    await user.click(screen.getByRole("button", { name: "Import" }));
    const done = await screen.findByRole("list", { name: "Imported" });
    expect(done).toHaveTextContent("1,200 imported");
    expect(writes(daemon.calls).at(-1)).toMatchObject({ body: { what: ["history", "bookmarks"], dry_run: false } });
    expect(screen.queryByRole("button", { name: "Import" })).toBeNull();
    expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument();
  });

  it("switches profiles to what that profile has", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon().client, "import");
    await user.click(await screen.findByRole("radio", { name: /Firefox · default/ }));
    expect(screen.getByRole("checkbox", { name: "History" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Bookmarks/ })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: /Cookies/ })).toBeDisabled();
    expect(screen.getAllByText("Not in this profile")).toHaveLength(2);
  });

  it("cannot pick cookies without a Chromium browser to write them", async () => {
    open(onboardingDaemon({ cookiesWritable: false }).client, "import");
    expect(await screen.findByRole("checkbox", { name: /Cookies/ })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: /Cookies/ })).not.toBeChecked();
    expect(screen.getByText("Needs a Chromium browser")).toBeInTheDocument();
  });

  it("explains a failed preview and offers to leave cookies out", async () => {
    const user = userEvent.setup();
    let refuse = true;
    const daemon = onboardingDaemon({
      override: (call) =>
        call.path === "/v1/import" && refuse ? json(422, { code: "keyring_password_required", message: "x" }) : undefined,
    });
    open(daemon.client, "import");
    await user.click(await screen.findByRole("button", { name: "Preview" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("locked by the computer's keyring");
    refuse = false;
    await user.click(screen.getByRole("button", { name: "Leave cookies out" }));
    expect(screen.getByRole("checkbox", { name: "Cookies" })).not.toBeChecked();
    expect(screen.queryByRole("alert")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByRole("list", { name: "Preview" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import" })).toBeEnabled();
  });

  it("says so when the profile has no cookies file instead of showing zeros", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon({
      override: (call) =>
        call.path === "/v1/import" ? json(200, report(true, { cookies: { read: 0, imported: 0, skipped: 0, rejected: [], note: "Not found in this profile." } })) : undefined,
    });
    open(daemon.client, "import");
    await user.click(await screen.findByRole("button", { name: "Preview" }));
    expect(await screen.findByRole("list", { name: "Preview" })).toHaveTextContent("Not found in this profile.");
  });

  it("reports when no browser is installed", async () => {
    open(onboardingDaemon({ profiles: [] }).client, "import");
    expect(await screen.findByText("No browser profiles found")).toBeInTheDocument();
  });
});

describe("vault step", () => {
  it("links to the Vault setup and notices when it is done", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const daemon = onboardingDaemon();
    open(daemon.client, "vault");
    const setup = await screen.findByRole("link", { name: "Set up the vault" });
    expect(setup).toHaveAttribute("href", "/vault");
    expect(screen.getByText("Not set up")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Skip this step" })).toBeInTheDocument();

    daemon.state.vault = "unlocked";
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_600);
    });
    expect(await screen.findByRole("link", { name: "Open the vault" })).toBeInTheDocument();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument();
  });

  it("opens the Vault over the guide so Close returns to this step", async () => {
    const user = userEvent.setup();
    open(onboardingDaemon().client, "vault");
    await user.click(await screen.findByRole("link", { name: "Set up the vault" }));
    expect(await screen.findByText("vault dialog")).toBeInTheDocument();
  });
});

describe("pair step", () => {
  it("shows the command and recognises the phone that appears", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const daemon = onboardingDaemon();
    open(daemon.client, "pair");
    expect(await screen.findByText('openside pair --name "My phone"')).toBeInTheDocument();
    expect(await screen.findByText("Waiting for a new device…")).toBeInTheDocument();
    // The laptop that was already paired is not "the phone".
    expect(screen.queryByText(/is paired/)).toBeNull();

    daemon.state.devices = [...daemon.state.devices, { device_id: "d2", name: "My phone", kind: "person_device", scopes: ["read", "approve"], created_at: "2026-10-12T00:00:00Z" }];
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_100);
    });
    expect(await screen.findByText("My phone is paired")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument();
  });
});

describe("done step", () => {
  it("recaps what was set up and finishes the guide", async () => {
    const user = userEvent.setup();
    const daemon = onboardingDaemon();
    open(daemon.client, "agents");
    await user.click(await screen.findByRole("button", { name: "Install the browser skill for Claude Code" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument());
    // Agents (done), then import, vault and pair are passed over.
    for (let step = 0; step < 4; step++) await user.click(screen.getByRole("button", { name: /Continue|Skip this step/ }));
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
