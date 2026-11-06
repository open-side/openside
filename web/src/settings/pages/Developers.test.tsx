import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { PointerEventsCheckLevel, type UserEvent } from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CliDefaults, ExternalAgent, Preferences } from "../../api/types";
import { ClientContext } from "../../context";
import { offStatus, tailscaleOn } from "../../remote/testDaemon";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { chooseOption } from "../../test/select";
import { ToastProvider } from "../../ui";
import { DevelopersSettings } from "./Developers";

const PREFERENCES: Preferences = {
  agent_tab_cleanup_minutes: 15,
  completion_sound: true,
  computer_use: false,
  confirmation: "ask_first",
  follow_up: "queue",
  mcp_server: false,
  name: null,
  notifications: ["needs_attention", "task_completed", "routine_completed"],
  save_sessions: true,
};

interface Options {
  agents?: ExternalAgent[];
  cli?: CliDefaults;
  /** Answers a call before the default routes; return undefined to fall through. */
  override?: (call: Call) => Response | undefined;
}

function daemon(options: Options = {}) {
  const agents: ExternalAgent[] = options.agents ?? [
    { id: "claude", label: "Claude Code", detected: true, skill_installed: true, skills_dir: "~/.claude/skills", custom: false },
    { id: "codex", label: "Codex", detected: true, skill_installed: false, skills_dir: "~/.codex/skills", custom: false },
    { id: "omp", label: "omp", detected: false, skill_installed: false, skills_dir: null, custom: false },
    { id: "my-agent", label: "my-agent", detected: false, skill_installed: false, skills_dir: "~/agents/skills", custom: true },
  ];
  let preferences = PREFERENCES;
  let cli: CliDefaults = options.cli ?? { profile: "shared", profile_is_default: true };
  return fakeClient((call) => {
    const forced = options.override?.(call);
    if (forced) return forced;
    const path = new URL(call.path, "http://daemon.test").pathname;
    const key = `${call.method} ${path}`;
    if (key === "GET /v1/settings/preferences") return json(200, preferences);
    if (key === "PATCH /v1/settings/preferences") {
      preferences = { ...preferences, ...(call.body as Partial<Preferences>) };
      return json(200, preferences);
    }
    if (key === "GET /v1/settings/cli") return json(200, cli);
    if (key === "PATCH /v1/settings/cli") {
      const { profile } = call.body as { profile: CliDefaults["profile"] | null };
      cli = { profile: profile ?? "shared", profile_is_default: profile === null };
      return json(200, cli);
    }
    if (key === "GET /v1/remote/status") return json(200, offStatus({ enabled: false }));
    if (key === "POST /v1/remote/enable") return json(200, offStatus());
    if (key === "GET /v1/external-agents") return json(200, { agents });
    const skill = path.match(/^\/v1\/external-agents\/([^/]+)\/browser-skill$/);
    if (call.method === "POST" && skill) {
      const agent = agents.find((candidate) => candidate.id === skill[1])!;
      agent.skill_installed = true;
      return json(200, { agent: agent.id, path: `${agent.skills_dir}/openside`, files: 2 });
    }
    if (call.method === "PUT" && path.startsWith("/v1/external-agents/")) {
      const id = path.split("/").pop()!;
      const { skills_dir } = call.body as { skills_dir: string };
      const known = agents.find((agent) => agent.id === id);
      if (known) known.skills_dir = skills_dir;
      else agents.push({ id, label: id, detected: false, skill_installed: false, skills_dir, custom: true });
      return json(200, agents.find((agent) => agent.id === id));
    }
    if (call.method === "DELETE" && path.startsWith("/v1/external-agents/")) {
      agents.splice(
        agents.findIndex((agent) => agent.id === path.split("/").pop()),
        1,
      );
      return json(200, { agents });
    }
    return json(404, { code: "not_found", message: "nope" });
  });
}

function open(options: Options = {}) {
  const fake = daemon(options);
  render(
    <MemoryRouter>
      <ClientContext.Provider value={fake.client}>
        <ToastProvider>
          <DevelopersSettings />
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

const row = (name: string) => screen.getByRole("group", { name });
const sent = (calls: Call[], method: string, path: string) => calls.filter((call) => call.method === method && call.path === path).map((call) => call.body);
const refuseLocal = () => json(403, { code: "local_access_required", message: "local" });

/** Opens "Add skill to…" once the agents have loaded. */
async function skillMenu(user: UserEvent) {
  const trigger = await within(row("openside Skills")).findByRole("button", { name: /Add skill to/ });
  await waitFor(() => expect(trigger).toBeEnabled());
  await user.click(trigger);
  return screen.findByRole("menu", { name: "Coding agents" });
}

// While a submenu is open Base UI turns pointer events off on the parent popup, and jsdom has no
// layout to tell that the submenu sits outside it; this pointer skips that check for submenu rows.
const submenuPointer = () => userEvent.setup({ pointerEventsCheck: PointerEventsCheckLevel.Never });

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Settings › Developers", () => {
  it("shows the CLI hero, then Settings, Skill & MCP, Remote Control and FAQs in that order", async () => {
    open();
    expect(await screen.findByRole("heading", { name: "openside CLI" })).toBeInTheDocument();
    await screen.findByRole("heading", { name: "Remote Control" });
    const sections = screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent);
    expect(sections).toEqual(["openside CLI", "Settings", "Skill & MCP", "Remote Control", "FAQs"]);
    expect(screen.getByRole("button", { name: "Reinstall" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "More CLI options" })).toBeInTheDocument();
  });

  it("copies the setup command and opens the CLI guide from the Reinstall menu", async () => {
    const user = userEvent.setup();
    vi.stubGlobal("isSecureContext", true);
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    const opened = vi.spyOn(window, "open").mockReturnValue(null);
    open();
    await user.click(await screen.findByRole("button", { name: "More CLI options" }));
    await user.click(await screen.findByRole("menuitem", { name: "Copy setup command" }));
    expect(writeText).toHaveBeenCalledWith("openside skills install");
    expect(await screen.findByText("Setup command copied")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "More CLI options" }));
    await user.click(await screen.findByRole("menuitem", { name: "Open CLI guide" }));
    expect(opened).toHaveBeenCalledWith("https://github.com/open-side/openside/blob/master/docs/reference/cli.md", "_blank", "noopener,noreferrer");
  });

  it("reinstalls the browser skill with force for every agent that has it", async () => {
    const user = userEvent.setup();
    const { calls } = open();
    const reinstall = await screen.findByRole("button", { name: "Reinstall" });
    await waitFor(() => expect(reinstall).toBeEnabled());
    await user.click(reinstall);
    expect(await screen.findByText("Browser skill reinstalled for Claude Code.")).toBeInTheDocument();
    expect(calls.filter((call) => call.method === "POST").map((call) => [call.path, call.body])).toEqual([["/v1/external-agents/claude/browser-skill", { force: true }]]);
  });

  it("reinstalls for every agent found when none has the skill yet", async () => {
    const user = userEvent.setup();
    const { calls } = open({
      agents: [
        { id: "claude", label: "Claude Code", detected: true, skill_installed: false, skills_dir: "~/.claude/skills", custom: false },
        { id: "codex", label: "Codex", detected: true, skill_installed: false, skills_dir: "~/.codex/skills", custom: false },
        { id: "omp", label: "omp", detected: false, skill_installed: false, skills_dir: "~/.omp/skills", custom: false },
      ],
    });
    const reinstall = await screen.findByRole("button", { name: "Reinstall" });
    await waitFor(() => expect(reinstall).toBeEnabled());
    await user.click(reinstall);
    expect(await screen.findByText("Browser skill reinstalled for Claude Code, Codex.")).toBeInTheDocument();
    expect(calls.filter((call) => call.method === "POST").map((call) => call.path)).toEqual(["/v1/external-agents/claude/browser-skill", "/v1/external-agents/codex/browser-skill"]);
  });

  it("saves Save sessions and puts it back when the daemon refuses", async () => {
    const user = userEvent.setup();
    let refuse = false;
    const { calls } = open({ override: (call) => (refuse && call.method === "PATCH" ? json(500, { code: "internal", message: "disk full" }) : undefined) });
    const toggle = await screen.findByRole("switch", { name: "Save sessions" });
    await waitFor(() => expect(toggle).toBeChecked());
    await user.click(toggle);
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(sent(calls, "PATCH", "/v1/settings/preferences")).toEqual([{ save_sessions: false }]);

    refuse = true;
    await user.click(toggle);
    expect(await screen.findByText(/disk full/)).toBeInTheDocument();
    await waitFor(() => expect(toggle).not.toBeChecked());
  });

  it("shows the account's first profile by default and saves a chosen or reset default profile", async () => {
    const user = userEvent.setup();
    const { calls } = open();
    const select = await within(row("Default profile")).findByRole("combobox");
    await waitFor(() => expect(select).toHaveTextContent("Account's first profile"));
    await chooseOption(user, select, /^Isolated/);
    await waitFor(() => expect(select).toHaveTextContent("Isolated"));
    await chooseOption(user, select, /^Account's first profile/);
    await waitFor(() => expect(select).toHaveTextContent("Account's first profile"));
    expect(sent(calls, "PATCH", "/v1/settings/cli")).toEqual([{ profile: "isolated" }, { profile: null }]);
  });

  it("explains that the default profile and the MCP server can only change on the computer itself", async () => {
    const user = userEvent.setup();
    open({ cli: { profile: "incognito", profile_is_default: false }, override: (call) => (call.method === "PATCH" ? refuseLocal() : undefined) });
    const select = await within(row("Default profile")).findByRole("combobox");
    await waitFor(() => expect(select).toHaveTextContent("Incognito"));
    await chooseOption(user, select, /^Shared/);
    expect(await screen.findByText(/change the default profile/)).toBeInTheDocument();
    expect(select).toHaveTextContent("Incognito");

    const mcp = screen.getByRole("switch", { name: "Enable openside MCP server" });
    await waitFor(() => expect(mcp).toBeEnabled());
    await user.click(mcp);
    expect(await screen.findByText(/turn the MCP server on or off/)).toBeInTheDocument();
    expect(mcp).not.toBeChecked();
  });

  it("turns the MCP server on with local access and shows how to set up a client", async () => {
    const user = userEvent.setup();
    const { calls } = open();
    const mcp = await screen.findByRole("switch", { name: "Enable openside MCP server" });
    await waitFor(() => expect(mcp).toBeEnabled());
    await user.click(mcp);
    await waitFor(() => expect(mcp).toBeChecked());
    expect(sent(calls, "PATCH", "/v1/settings/preferences")).toEqual([{ mcp_server: true }]);

    await user.click(screen.getByRole("button", { name: "How to set up the MCP server" }));
    const help = await screen.findByRole("dialog", { name: "Set up the MCP server" });
    expect(within(help).getByText(/"command": "openside"/)).toBeInTheDocument();
    expect(within(help).getByText(/--host/)).toBeInTheDocument();
  });

  it("lists every agent with its skill state in Add skill to… and installs the skill for one", async () => {
    const user = userEvent.setup();
    const { calls } = open();
    const menu = await skillMenu(user);
    expect(within(menu).getByRole("menuitemcheckbox", { name: "Claude Code" })).toHaveAttribute("aria-checked", "true");
    const codex = within(menu).getByRole("menuitemcheckbox", { name: "Codex" });
    expect(codex).toHaveAttribute("aria-checked", "false");
    const omp = within(menu).getByRole("menuitemcheckbox", { name: /^omp/ });
    expect(omp).toHaveAttribute("aria-disabled", "true");
    expect(within(omp).getByText("Not found")).toBeInTheDocument();

    await user.click(codex);
    expect(await screen.findByText("Browser skill installed for Codex.")).toBeInTheDocument();
    expect(sent(calls, "POST", "/v1/external-agents/codex/browser-skill")).toEqual([{ force: false }]);
    const again = await skillMenu(user);
    expect(within(again).getByRole("menuitemcheckbox", { name: "Codex" })).toHaveAttribute("aria-checked", "true");
  });

  it("adds another agent by its skills folder from Add skill to…", async () => {
    const user = userEvent.setup();
    const { calls } = open();
    await user.click(within(await skillMenu(user)).getByRole("menuitem", { name: "Add another agent…" }));
    const dialog = await screen.findByRole("dialog", { name: "Add an agent" });
    await user.type(within(dialog).getByLabelText("Agent name"), "gemini-cli");
    await user.type(within(dialog).getByLabelText("Skills folder"), "~/.gemini/skills");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(calls.find((call) => call.method === "PUT")).toMatchObject({ path: "/v1/external-agents/gemini-cli", body: { skills_dir: "~/.gemini/skills" } });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add an agent" })).toBeNull());
    expect(within(await skillMenu(user)).getByRole("menuitemcheckbox", { name: /^gemini-cli/ })).toBeInTheDocument();
  });

  it("changes the folder of an added agent but offers no such change for a built-in one", async () => {
    const user = userEvent.setup();
    const { calls } = open();
    const menu = await skillMenu(user);
    expect(within(menu).queryByRole("menuitem", { name: "Claude Code" })).toBeNull();
    await user.click(within(menu).getByRole("menuitem", { name: "my-agent" }));
    const actions = await screen.findByRole("menu", { name: "my-agent" });
    await submenuPointer().click(within(actions).getByRole("menuitem", { name: "Change skills folder" }));
    const dialog = await screen.findByRole("dialog", { name: "Skills folder of my-agent" });
    expect(within(dialog).getByLabelText("Agent name")).toHaveAttribute("readonly");
    const folder = within(dialog).getByLabelText("Skills folder");
    await user.clear(folder);
    await user.type(folder, "~/agents/new-skills");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.find((call) => call.method === "PUT")?.body).toEqual({ skills_dir: "~/agents/new-skills" }));
  });

  it("forgets an added agent after a confirmation", async () => {
    const user = userEvent.setup();
    const { calls } = open();
    await user.click(within(await skillMenu(user)).getByRole("menuitem", { name: "my-agent" }));
    const actions = await screen.findByRole("menu", { name: "my-agent" });
    await submenuPointer().click(within(actions).getByRole("menuitem", { name: "Forget agent" }));
    const confirm = await screen.findByRole("dialog", { name: "Forget my-agent?" });
    expect(calls.some((call) => call.method === "DELETE")).toBe(false);
    await user.click(within(confirm).getByRole("button", { name: "Forget" }));
    await waitFor(() => expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/external-agents/my-agent")).toBe(true));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Forget my-agent?" })).toBeNull());
    expect(within(await skillMenu(user)).queryByRole("menuitemcheckbox", { name: /^my-agent/ })).toBeNull();
  });

  it("says a name is invalid before asking the daemon, and explains local-only refusals", async () => {
    const user = userEvent.setup();
    const { calls } = open({ override: (call) => (call.method === "PUT" ? refuseLocal() : undefined) });
    await user.click(within(await skillMenu(user)).getByRole("menuitem", { name: "Add another agent…" }));
    const dialog = await screen.findByRole("dialog", { name: "Add an agent" });
    await user.type(within(dialog).getByLabelText("Agent name"), "Bad Name");
    await user.type(within(dialog).getByLabelText("Skills folder"), "~/x");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("lowercase letters, digits and hyphens");
    expect(calls.some((call) => call.method === "PUT")).toBe(false);

    const name = within(dialog).getByLabelText("Agent name");
    await user.clear(name);
    await user.type(name, "good-name");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByText(/To change an agent, use openside on the computer that runs it/)).toBeInTheDocument();
  });

  it("opens the agent link for another computer in a dialog", async () => {
    const user = userEvent.setup();
    open();
    await user.click(await screen.findByRole("button", { name: /^Connect an agent on another computer/ }));
    expect(await screen.findByRole("dialog", { name: "Connect an agent on another computer" })).toBeInTheDocument();
  });

  it("turns Remote Control on from its switch", async () => {
    const user = userEvent.setup();
    const { calls } = open({ override: (call) => (call.method === "PUT" && call.path === "/v1/remote/tailscale" ? json(200, tailscaleOn()) : undefined) });
    const allow = await screen.findByRole("switch", { name: "Allow remote sessions on this machine" });
    expect(allow).not.toBeChecked();
    await user.click(allow);
    await waitFor(() => expect(allow).toBeChecked());
    // Nothing is saved yet, so the one-click connection comes on with Remote Control.
    expect(calls.filter((call) => call.method !== "GET").map((call) => `${call.method} ${call.path}`)).toEqual(["POST /v1/remote/enable", "PUT /v1/remote/tailscale"]);
    expect(sent(calls, "PUT", "/v1/remote/tailscale")).toEqual([{ enabled: true }]);
  });

  it("opens and closes each FAQ answer", async () => {
    const user = userEvent.setup();
    open();
    const question = await screen.findByRole("button", { name: "What's the difference between the openside CLI and MCP?" });
    expect(question).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("region", { name: "What's the difference between the openside CLI and MCP?" })).toBeNull();
    await user.click(question);
    expect(question).toHaveAttribute("aria-expanded", "true");
    const answer = screen.getByRole("region", { name: "What's the difference between the openside CLI and MCP?" });
    expect(question).toHaveAttribute("aria-controls", answer.id);
    expect(answer).toHaveTextContent(/openside mcp/);
    await user.click(question);
    expect(question).toHaveAttribute("aria-expanded", "false");
    expect(screen.getAllByRole("heading", { level: 3 })).toHaveLength(5);
  });
});
