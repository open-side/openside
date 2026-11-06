import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PolicySettings, PolicyValidation, Preferences, SettingsDocument } from "../../api/types";
import type * as CompletionSoundModule from "../../completionSound";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { chooseOption } from "../../test/select";
import { ToastProvider } from "../../ui";
import { AgentsSettings } from "./Agents";

const sound = vi.hoisted(() => ({ play: vi.fn() }));
vi.mock("../../completionSound", async (importOriginal) => {
  const actual = await importOriginal<typeof CompletionSoundModule>();
  return { ...actual, playCompletionSound: sound.play };
});

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

const POLICY: PolicySettings = {
  default_mode: "guard",
  allow_origins: ["https://*.example.com"],
  deny_origins: ["https://ads.example"],
  rules: [{ effect: "deny", capability: "exec", pattern: "rm *" }],
};

interface Options {
  validate?: (call: Call) => PolicyValidation;
  remote?: boolean;
  runtime?: string | null;
}

function daemon(options: Options = {}) {
  let preferences = PREFERENCES;
  let policy = POLICY;
  let config: SettingsDocument = {
    config: { sandbox: { required: false, backend: "auto" }, memory: { history: true } },
    file_only: ["credentials"],
    remote: options.remote ?? false,
    remote_editable: options.remote ? ["memory.history", "preferences."] : [],
    restart_required: false,
    schema: {},
  };
  return fakeClient((call) => {
    const path = new URL(call.path, "http://daemon.test").pathname;
    if (path === "/v1/settings/preferences" && call.method === "GET") return json(200, preferences);
    if (path === "/v1/settings/preferences" && call.method === "PATCH") {
      preferences = { ...preferences, ...(call.body as Partial<Preferences>) };
      return json(200, preferences);
    }
    if (path === "/v1/policy" && call.method === "GET") return json(200, policy);
    if (path === "/v1/policy" && call.method === "PUT") {
      policy = call.body as PolicySettings;
      return json(200, policy);
    }
    if (path === "/v1/policy/validate") return json(200, options.validate?.(call) ?? { valid: true, problems: [] });
    if (path === "/v1/settings/config" && call.method === "GET") return json(200, config);
    if (path === "/v1/settings/config" && call.method === "PUT") {
      config = { ...config, config: (call.body as { config: unknown }).config, restart_required: true };
      return json(200, config);
    }
    if (path === "/v1/computer-use") return json(200, { supported: true, reasons: [], parts: [] });
    if (path === "/v1/about") return json(200, { version: "0.9.0", runtime_version: options.runtime === undefined ? "1.4.2" : options.runtime, runtime_versions: "^1.4", releases_url: "https://example.test" });
    if (path === "/v1/daemon/restart") return json(202, { boot_id: "boot-1" });
    if (path === "/v1/daemon") return json(200, { boot_id: "boot-2" });
    if (path === "/v1/push/subscriptions") return json(200, { subscriptions: [] });
    return json(404, { code: "not_found", message: "nope" });
  });
}

function show(options?: Options) {
  const fake = daemon(options);
  const user = userEvent.setup();
  render(
    <MemoryRouter>
      <ToastProvider>
        <ClientContext.Provider value={fake.client}>
          <AgentsSettings />
        </ClientContext.Provider>
      </ToastProvider>
    </MemoryRouter>,
  );
  return { ...fake, user };
}

const row = (name: string) => screen.getByRole("group", { name });
const control = (name: string) => within(row(name)).getByRole("combobox");
const lastPut = (calls: Call[], path: string) => calls.filter((call) => call.method === "PUT" && call.path === path).at(-1);
const lastPatch = (calls: Call[]) => calls.filter((call) => call.method === "PATCH").at(-1)?.body;

beforeEach(() => {
  sound.play.mockClear();
});

afterEach(() => {
  localStorage.clear();
});

describe("Settings › Agents · Chat", () => {
  it("reads all three task notification kinds as Everything and saves a narrower choice", async () => {
    const { user, calls } = show();
    await waitFor(() => expect(control("Task notifications")).toHaveTextContent("Everything"));
    await user.click(control("Task notifications"));
    await user.click(await screen.findByRole("option", { name: /Routine completions/ }));
    await waitFor(() => expect(lastPatch(calls)).toEqual({ notifications: ["needs_attention", "task_completed"] }));
    expect(control("Task notifications")).toHaveTextContent("Needs attention, Task completions");
  });

  it("turns the completion sound off, previews it and remembers another sound for this browser", async () => {
    const { user, calls } = show();
    const toggle = await within(row("Completion sound")).findByRole("switch");
    await waitFor(() => expect(toggle).toBeChecked());
    await user.click(toggle);
    await waitFor(() => expect(lastPatch(calls)).toEqual({ completion_sound: false }));

    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(sound.play).toHaveBeenLastCalledWith("chime");
    await user.click(screen.getByRole("button", { name: "Change" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Bell" }));
    expect(localStorage.getItem("openside.completionSound")).toBe("bell");
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(sound.play).toHaveBeenLastCalledWith("bell");
  });

  it("sets the default access mode and sends the origin lists back untouched", async () => {
    const { user, calls } = show();
    await waitFor(() => expect(control("Access permission")).toHaveTextContent("Guard"));
    await chooseOption(user, control("Access permission"), "Full access");
    await waitFor(() => expect(lastPut(calls, "/v1/policy")?.body).toEqual({ ...POLICY, default_mode: "full" }));
    expect(calls.some((call) => call.path === "/v1/policy/validate")).toBe(true);
  });

  it("switches follow-ups from queue to steer", async () => {
    const { user, calls } = show();
    await waitFor(() => expect(control("Follow-up behavior")).toHaveTextContent("Queue"));
    await chooseOption(user, control("Follow-up behavior"), "Steer");
    await waitFor(() => expect(lastPatch(calls)).toEqual({ follow_up: "steer" }));
  });

  it("keeps one chat per tab and shows the other choice as unavailable", async () => {
    const { user } = show();
    expect(control("Ask openside on tab switch")).toHaveTextContent("New chat per tab");
    await user.click(control("Ask openside on tab switch"));
    expect(await screen.findByRole("option", { name: "Keep current chat" })).toHaveAttribute("aria-disabled", "true");
  });

  it("offers Aside's tab clean-up delays and saves minutes", async () => {
    const { user, calls } = show();
    await waitFor(() => expect(control("Clean up agent tabs")).toHaveTextContent("After 15 min"));
    await user.click(control("Clean up agent tabs"));
    const options = (await screen.findAllByRole("option")).map((option) => option.textContent);
    expect(options).toEqual(["After 5 min", "After 10 min", "After 15 min", "After 30 min", "After an hour", "After 2 hours"]);
    await user.click(screen.getByRole("option", { name: "After an hour" }));
    await waitFor(() => expect(lastPatch(calls)).toEqual({ agent_tab_cleanup_minutes: 60 }));
  });
});

describe("Settings › Agents · Sandbox", () => {
  it("writes sandbox.required into the whole configuration and asks for a restart", async () => {
    const { user, calls } = show();
    const toggle = await within(row("Enable sandbox")).findByRole("switch");
    await waitFor(() => expect(toggle).toBeEnabled());
    await user.click(toggle);
    await waitFor(() =>
      expect(lastPut(calls, "/v1/settings/config")?.body).toEqual({ config: { sandbox: { required: true, backend: "auto" }, memory: { history: true } } }),
    );
    expect(await screen.findByText("Restart openside to apply the sandbox change.")).toBeInTheDocument();
  });

  it("cannot be changed from a remote browser", async () => {
    show({ remote: true });
    await waitFor(() => expect(within(row("Enable sandbox")).getByText(/Change this on the computer that runs openside/)).toBeInTheDocument());
    expect(within(row("Enable sandbox")).getByRole("switch")).toBeDisabled();
  });
});

describe("Settings › Agents · File permissions", () => {
  it("adds a folder agents can view as an fs_read allow rule and removes it again", async () => {
    const { user, calls } = show();
    await waitFor(() => expect(within(row("Can view")).getByRole("button", { name: "Add" })).toBeEnabled());
    expect(screen.getAllByText("Project root")).toHaveLength(2);
    await user.click(within(row("Can view")).getByRole("button", { name: "Add" }));
    const dialog = await screen.findByRole("dialog", { name: "Add a folder agents can view" });
    await user.type(within(dialog).getByLabelText("Folder"), "/home/me/docs/");
    await user.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(lastPut(calls, "/v1/policy")?.body).toEqual({ ...POLICY, rules: [...POLICY.rules, { effect: "allow", capability: "fs_read", pattern: "/home/me/docs/**" }] }),
    );
    await user.click(await screen.findByRole("button", { name: "Remove /home/me/docs" }));
    await waitFor(() => expect(lastPut(calls, "/v1/policy")?.body).toEqual(POLICY));
  });

  it("adds a folder agents can edit as an fs_write rule alone, without granting fs_read", async () => {
    const { user, calls } = show();
    await waitFor(() => expect(within(row("Can edit")).getByRole("button", { name: "Add" })).toBeEnabled());
    await user.click(within(row("Can edit")).getByRole("button", { name: "Add" }));
    const dialog = await screen.findByRole("dialog", { name: "Add a folder agents can edit" });
    await user.type(within(dialog).getByLabelText("Folder"), "/home/me/out");
    await user.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(lastPut(calls, "/v1/policy")?.body).toEqual({ ...POLICY, rules: [...POLICY.rules, { effect: "allow", capability: "fs_write", pattern: "/home/me/out/**" }] }),
    );
  });
});

describe("Settings › Agents · Tool permissions", () => {
  it("lists rules by outcome and adds an allow rule from plain action names", async () => {
    const { user, calls } = show();
    const allow = () => screen.getByRole("group", { name: "Allow" });
    await waitFor(() => expect(within(screen.getByRole("group", { name: "Deny" })).getByText("rm *")).toBeInTheDocument());
    expect(within(allow()).getByText("No allow rules")).toBeInTheDocument();
    await user.click(within(allow()).getByRole("button", { name: "Add a Allow rule" }));
    const dialog = await screen.findByRole("dialog");
    await chooseOption(user, within(dialog).getByRole("combobox"), "Browse sites");
    const pattern = within(dialog).getByLabelText("Applies to");
    await user.clear(pattern);
    await user.type(pattern, "https://docs.example/**");
    await user.click(within(dialog).getByRole("button", { name: "Add Rule" }));
    await waitFor(() =>
      expect(lastPut(calls, "/v1/policy")?.body).toEqual({ ...POLICY, rules: [...POLICY.rules, { effect: "allow", capability: "browse", pattern: "https://docs.example/**" }] }),
    );
    expect(await within(allow()).findByText("https://docs.example/**")).toBeInTheDocument();
  });

  it("offers sensitive actions only for Deny rules, which is all the daemon accepts", async () => {
    const { user } = show();
    const optionsIn = async (effect: string) => {
      await user.click(await screen.findByRole("button", { name: `Add a ${effect} rule` }));
      const dialog = await screen.findByRole("dialog");
      await user.click(within(dialog).getByRole("combobox"));
      const names = (await screen.findAllByRole("option")).map((option) => option.textContent);
      await user.keyboard("{Escape}");
      await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      return names;
    };
    expect(await optionsIn("Allow")).not.toContain("Sensitive actions");
    expect(await optionsIn("Ask")).not.toContain("Sensitive actions");
    expect(await optionsIn("Deny")).toContain("Sensitive actions");
  });

  it("removes a rule", async () => {
    const { user, calls } = show();
    await user.click(await screen.findByRole("button", { name: "Remove rule Run commands rm *" }));
    await waitFor(() => expect(lastPut(calls, "/v1/policy")?.body).toEqual({ ...POLICY, rules: [] }));
  });

  it("keeps the old rules and says why when the daemon rejects the change", async () => {
    const { user, calls } = show({ validate: () => ({ valid: false, problems: [{ list: "rules", index: 1, message: "pattern is empty" }] }) });
    await user.click(await screen.findByRole("button", { name: "Remove rule Run commands rm *" }));
    expect(await screen.findByText(/Couldn't save the permissions: pattern is empty/)).toBeInTheDocument();
    expect(lastPut(calls, "/v1/policy")).toBeUndefined();
    expect(screen.getByText("rm *")).toBeInTheDocument();
  });
});

describe("Settings › Agents · Agent runtime", () => {
  it("shows the running runtime version and restarts it after confirmation", async () => {
    const { user, calls } = show();
    expect(await screen.findByText("Current version is 1.4.2.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Restart" }));
    const confirm = await screen.findByRole("dialog", { name: "Restart openside?" });
    await user.click(within(confirm).getByRole("button", { name: "Restart" }));
    await waitFor(() => expect(calls.some((call) => call.method === "POST" && call.path === "/v1/daemon/restart")).toBe(true));
  });

  it("says when no runtime has started yet", async () => {
    show({ runtime: null });
    expect(await screen.findByText("Shows once the first chat has started.")).toBeInTheDocument();
  });
});
