import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../../api/client";
import type { McpServer, SkillDetail } from "../../api/types";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { ToastProvider } from "../../ui";
import { PluginsSettings } from "./Plugins";
import { joinCommand, splitCommand } from "./plugins/command";

function detail(name: string, extra: Partial<SkillDetail> = {}): SkillDetail {
  return { name, description: `${name} helper`, body: `# ${name}`, enabled: true, source: "user", resources: [], allowed_tools: [], ...extra } as SkillDetail;
}

function server(id: string, extra: Partial<McpServer> = {}): McpServer {
  return { id, label: id, enabled: true, allow_private_network: false, secrets: [], status: "connected", tools: [], transport: "stdio", command: ["node", "server.js"], ...extra };
}

const LOCAL_REFUSAL = { code: "local_access_required", message: "Open openside on this computer to change MCP servers." };

/** In-memory `/v1/skills` and `/v1/mcp/servers` with the semantics the page relies on. */
function daemon(options: { skills?: SkillDetail[]; servers?: McpServer[]; refuseMcpWrites?: boolean } = {}) {
  const skills = new Map((options.skills ?? [detail("pdf-tools")]).map((skill) => [skill.name, skill]));
  const servers = new Map((options.servers ?? []).map((entry) => [entry.id, entry]));
  return fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    const parts = url.pathname.split("/");
    if (url.pathname.startsWith("/v1/mcp/servers")) {
      if (call.method !== "GET" && options.refuseMcpWrites) return json(403, LOCAL_REFUSAL);
      const id = decodeURIComponent(parts[4] ?? "");
      if (url.pathname === "/v1/mcp/servers" && call.method === "GET") return json(200, { servers: [...servers.values()] });
      if (url.pathname === "/v1/mcp/servers" && call.method === "POST") {
        const body = call.body as { label: string; transport: McpServer["transport"]; command: string[] | null; cwd: string | null; url: string | null; allow_private_network?: boolean; secrets: Record<string, string> };
        const created = server(body.label.toLowerCase(), {
          label: body.label,
          transport: body.transport,
          command: body.command,
          cwd: body.cwd,
          url: body.url,
          allow_private_network: body.allow_private_network ?? false,
          secrets: Object.keys(body.secrets),
        });
        servers.set(created.id, created);
        return json(201, created);
      }
      const found = servers.get(id);
      if (!found) return json(404, { code: "mcp_server_not_found", message: "nope" });
      if (parts[5] === "refresh") return json(200, found);
      if (call.method === "PATCH") {
        const { secrets, ...rest } = call.body as Record<string, unknown> & { secrets?: Record<string, string | null> };
        Object.assign(found, rest, rest.enabled === false ? { status: "disabled" } : {});
        if (secrets) {
          const names = new Set(found.secrets);
          for (const [name, value] of Object.entries(secrets)) {
            if (value === null) names.delete(name);
            else names.add(name);
          }
          found.secrets = [...names];
        }
        return json(200, found);
      }
      if (call.method === "DELETE") {
        servers.delete(id);
        return new Response(null, { status: 204 });
      }
      return json(200, found);
    }
    const name = decodeURIComponent(parts[3] ?? "");
    if (url.pathname === "/v1/skills" && call.method === "GET") {
      return json(200, {
        skills: [...skills.values()].map(({ name, description, source, enabled }) => ({ name, description, source, enabled })),
        problems: [{ name: "Broken", source: "user", error: "invalid name" }],
      });
    }
    if (url.pathname === "/v1/skills" && call.method === "POST") {
      const body = call.body as { name: string; description: string; body: string };
      if (skills.has(body.name)) return json(409, { code: "skill_exists", message: "A skill with that name exists" });
      skills.set(body.name, detail(body.name, { description: body.description, body: body.body }));
      return json(201, skills.get(body.name));
    }
    const skill = skills.get(name);
    if (!skill) return json(404, { code: "not_found", message: "nope" });
    if (call.method === "PATCH") {
      Object.assign(skill, call.body);
      return json(200, skill);
    }
    if (call.method === "DELETE") {
      skills.delete(name);
      return json(200, skill);
    }
    return json(200, skill);
  });
}

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname + location.search}</output>;
}

function setup(path = "/settings/plugins", options?: Parameters<typeof daemon>[0]) {
  const fake = daemon(options);
  render(
    <MemoryRouter initialEntries={[path]}>
      <ClientContext.Provider value={fake.client}>
        <ToastProvider>
          <PluginsSettings />
          <Location />
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

describe("PluginsSettings tabs", () => {
  it("keeps the selected tab in the URL", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const tablist = () => screen.getByRole("tablist", { name: "Plugins & MCPs" });
    expect(within(tablist()).getByRole("tab", { name: "Skills" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByRole("tabpanel")).toHaveAccessibleName("Skills");
    expect(screen.getByRole("group", { name: "Import" })).toBeInTheDocument();

    // Each tab renders its own strip, so the tablist is looked up again after every switch.
    await user.click(within(tablist()).getByRole("tab", { name: "MCPs" }));
    expect(screen.getByTestId("location")).toHaveTextContent("/settings/plugins?tab=mcps");
    expect(within(tablist()).getByRole("tab", { name: "MCPs" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("group", { name: "Import" })).toBeNull();
    await vi.waitFor(() => expect(calls.some((c) => c.path === "/v1/mcp/servers")).toBe(true));

    // Arrow keys move between tabs as the ARIA tabs pattern asks.
    within(tablist()).getByRole("tab", { name: "MCPs" }).focus();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/settings\/plugins$/);
  });

  it("opens on MCPs when the URL says so", async () => {
    setup("/settings/plugins?tab=mcps");
    expect(screen.getByRole("tab", { name: "MCPs" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByRole("list", { name: "MCP servers" })).toBeInTheDocument();
  });
});

describe("PluginsSettings › Skills", () => {
  it("groups skills, then creates, toggles, edits and deletes one", async () => {
    const user = userEvent.setup();
    const { calls } = setup("/settings/plugins", { skills: [detail("pdf-tools"), detail("repo-guide", { source: "project", description: "From the workspace" })] });
    const yours = await screen.findByRole("list", { name: "Your skills" });
    expect(within(yours).getByText("pdf-tools")).toBeInTheDocument();
    const project = screen.getByRole("list", { name: "Project skills" });
    expect(within(project).getByText("repo-guide")).toBeInTheDocument();
    expect(within(project).getByText("Read-only")).toBeInTheDocument();
    expect(within(project).queryByRole("switch")).toBeNull();
    expect(within(screen.getByRole("list", { name: "Skipped folders" })).getByText("invalid name")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "More ways to add a skill" }));
    await user.click(await screen.findByRole("menuitem", { name: "New skill" }));
    const form = await screen.findByRole("form", { name: "New skill" });
    await user.type(within(form).getByLabelText("Name"), "release-notes");
    await user.type(within(form).getByLabelText("Description"), "Draft release notes");
    await user.click(within(form).getByRole("button", { name: "Create" }));
    await within(yours).findByText("release-notes");
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ name: "release-notes", description: "Draft release notes", body: "", resources: [] });

    const row = within(yours).getByText("release-notes").closest("li")!;
    expect(row).toHaveAttribute("data-skill", "release-notes");
    const toggle = within(row).getByRole("switch", { name: "Enable /release-notes" });
    expect(toggle).toBeChecked();
    await user.click(toggle);
    await vi.waitFor(() => expect(within(row).getByRole("switch", { name: "Enable /release-notes" })).not.toBeChecked());
    expect(calls.find((c) => c.method === "PATCH")).toMatchObject({ path: "/v1/skills/release-notes", body: { enabled: false } });

    // The row itself opens the editor.
    await user.click(within(row).getByRole("button", { name: "Edit /release-notes" }));
    const editor = await screen.findByRole("form", { name: "Edit release-notes" });
    const description = await within(editor).findByDisplayValue("Draft release notes");
    await user.clear(description);
    await user.type(description, "Write the changelog");
    await user.click(within(editor).getByRole("button", { name: "Save" }));
    expect(await within(yours).findByText("Write the changelog")).toBeInTheDocument();

    await user.click(within(row).getByRole("button", { name: "Options for /release-notes" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const confirm = await screen.findByRole("dialog", { name: "Delete skill?" });
    await user.click(within(confirm).getByRole("button", { name: "Delete" }));
    await vi.waitFor(() => expect(within(yours).queryByText("release-notes")).toBeNull());
    expect(calls.at(-2)).toMatchObject({ method: "DELETE", path: "/v1/skills/release-notes" });
  });

  it("shows an empty Your skills card that starts a new skill", async () => {
    const user = userEvent.setup();
    setup("/settings/plugins", { skills: [] });
    expect(await screen.findByText("No skills yet")).toBeInTheDocument();
    // openside has no engine for Aside's built-in, system or password manager groups, and says so.
    expect(screen.getByText("openside doesn't come with built-in, system or password manager skills yet.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "New skill" }));
    expect(await screen.findByRole("form", { name: "New skill" })).toBeInTheDocument();
  });

  it("shows the daemon's error when a create is refused", async () => {
    const user = userEvent.setup();
    setup();
    await screen.findByRole("list", { name: "Your skills" });
    await user.click(screen.getByRole("button", { name: "More ways to add a skill" }));
    await user.click(await screen.findByRole("menuitem", { name: "New skill" }));
    const form = await screen.findByRole("form", { name: "New skill" });
    await user.type(within(form).getByLabelText("Name"), "pdf-tools");
    await user.type(within(form).getByLabelText("Description"), "dup");
    await user.click(within(form).getByRole("button", { name: "Create" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("A skill with that name exists");
  });

  it("explains how to import a skill folder", async () => {
    const user = userEvent.setup();
    setup();
    await screen.findByRole("list", { name: "Your skills" });
    await user.click(screen.getByRole("button", { name: "Import" }));
    const dialog = await screen.findByRole("dialog", { name: "Import a skill folder" });
    expect(within(dialog).getByText("openside skills import <folder>")).toBeInTheDocument();
    expect(within(dialog).getByText("Picking a folder from this page isn't available yet.")).toBeInTheDocument();
  });
});

describe("PluginsSettings › MCPs", () => {
  const servers = () => [
    server("files", { label: "Files", tools: [{ name: "read", exposed_name: "mcp__files__read", read_only_hint: true }, { name: "write", exposed_name: "mcp__files__write", read_only_hint: false }] }),
    server("notes", { label: "Notes", status: "failed", error: "spawn failed: no such file" }),
    server("crm", { label: "CRM", status: "vault_locked", transport: "http", command: null, url: "https://crm.example.com/mcp", secrets: ["Authorization"] }),
    server("old", { label: "Old", enabled: false, status: "disabled" }),
  ];

  it("lists servers with their status and toggles, refreshes and removes one", async () => {
    const user = userEvent.setup();
    const { calls } = setup("/settings/plugins?tab=mcps", { servers: servers() });
    expect(screen.getByRole("note")).toHaveTextContent("You may not need an MCP");
    const list = await screen.findByRole("list", { name: "MCP servers" });
    const row = (label: string) => within(list).getByText(label).closest("li")!;
    expect(row("Files")).toHaveTextContent("Connected · 2 tools");
    expect(row("Notes")).toHaveTextContent("Couldn't connect: spawn failed: no such file");
    expect(row("CRM")).toHaveTextContent("Waiting for Passwords to unlock");
    expect(row("Old")).toHaveTextContent("Off");
    expect(within(row("Old")).getByRole("switch", { name: "Enable Old" })).not.toBeChecked();

    await user.click(within(row("Files")).getByRole("switch", { name: "Enable Files" }));
    await vi.waitFor(() => expect(row("Files")).toHaveTextContent("Off"));
    expect(calls.find((c) => c.method === "PATCH")).toMatchObject({ path: "/v1/mcp/servers/files", body: { enabled: false } });

    await user.click(within(row("Notes")).getByRole("button", { name: "Options for Notes" }));
    await user.click(await screen.findByRole("menuitem", { name: "Refresh" }));
    await vi.waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/v1/mcp/servers/notes/refresh")).toBe(true));

    await user.click(within(row("Old")).getByRole("button", { name: "Options for Old" }));
    await user.click(await screen.findByRole("menuitem", { name: "Remove" }));
    const confirm = await screen.findByRole("dialog", { name: "Remove server?" });
    await user.click(within(confirm).getByRole("button", { name: "Remove" }));
    await vi.waitFor(() => expect(within(list).queryByText("Old")).toBeNull());
    expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/mcp/servers/old")).toBe(true);
  });

  it("connects a command server, splitting the command line without a shell", async () => {
    const user = userEvent.setup();
    const { calls } = setup("/settings/plugins?tab=mcps");
    const list = await screen.findByRole("list", { name: "MCP servers" });
    await user.click(within(list).getByRole("button", { name: "Connect" }));
    const form = await screen.findByRole("form", { name: "Connect an MCP server" });
    // Command servers run locally, so the private-network switch only exists for URL servers.
    expect(within(form).queryByRole("switch", { name: "Allow private network" })).toBeNull();
    await user.type(within(form).getByLabelText("Name"), "Docs");
    await user.type(within(form).getByLabelText("Command"), `npx -y "@acme/docs server" --root '$HOME/my docs'`);
    await user.type(within(form).getByLabelText("Working folder (optional)"), "/srv/docs");
    await user.click(within(form).getByRole("button", { name: "Add variable" }));
    await user.type(within(form).getByLabelText("Name 1"), "DOCS_TOKEN");
    await user.type(within(form).getByLabelText("Value 1"), "s3cret");
    await user.click(within(form).getByRole("button", { name: "Connect" }));
    expect(await within(list).findByText("Docs")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      id: null,
      label: "Docs",
      enabled: null,
      transport: "stdio",
      command: ["npx", "-y", "@acme/docs server", "--root", "$HOME/my docs"],
      cwd: "/srv/docs",
      url: null,
      allow_private_network: false,
      secrets: { DOCS_TOKEN: "s3cret" },
    });
  });

  it("lists the presets and asks before allowing a private network", async () => {
    const user = userEvent.setup();
    const { calls } = setup("/settings/plugins?tab=mcps", { servers: servers() });
    const list = await screen.findByRole("list", { name: "MCP servers" });
    const presets = screen.getByRole("list", { name: "Presets" });
    expect(within(presets).getAllByRole("listitem").map((item) => item.getAttribute("data-preset"))).toEqual(["figma", "blender"]);
    expect(screen.getByText(/Granola isn't offered yet/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Add Figma" }));
    const preset = await screen.findByRole("form", { name: "Connect an MCP server" });
    expect(within(preset).getByLabelText("Name")).toHaveValue("Figma");
    expect(within(preset).getByLabelText("URL")).toHaveValue("http://127.0.0.1:3845/mcp");

    // Cancelling the confirmation leaves the switch off and the request says so.
    await user.click(within(preset).getByRole("switch", { name: "Allow private network" }));
    const ask = await screen.findByRole("dialog", { name: "Allow private network?" });
    expect(ask).toHaveTextContent("send this server its secrets");
    await user.click(within(ask).getByRole("button", { name: "Cancel" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog", { name: "Allow private network?" })).toBeNull());
    expect(within(preset).getByRole("switch", { name: "Allow private network" })).not.toBeChecked();
    await user.click(within(preset).getByRole("button", { name: "Connect" }));
    expect(await within(list).findByText("Figma")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({ label: "Figma", transport: "http", url: "http://127.0.0.1:3845/mcp", allow_private_network: false });
  });

  it("edits a URL server's secrets and confirms the private network opt-in", async () => {
    const user = userEvent.setup();
    const { calls } = setup("/settings/plugins?tab=mcps", { servers: servers() });
    const list = await screen.findByRole("list", { name: "MCP servers" });

    const crm = within(list).getByText("CRM").closest("li")!;
    await user.click(within(crm).getByRole("button", { name: "Options for CRM" }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const form = await screen.findByRole("form", { name: "Edit server" });
    expect(within(form).getByLabelText("Name 1")).toHaveValue("Authorization");
    expect(within(form).getByLabelText("Value 1")).toHaveValue("");
    const url = within(form).getByLabelText("URL");
    await user.clear(url);
    await user.type(url, "https://crm.example.com/v2/mcp");
    await user.click(within(form).getByRole("button", { name: "Add header" }));
    await user.type(within(form).getByLabelText("Name 2"), "X-Team");
    await user.type(within(form).getByLabelText("Value 2"), "sales");
    const privateNetwork = within(form).getByRole("switch", { name: "Allow private network" });
    expect(privateNetwork).not.toBeChecked();
    expect(privateNetwork).toHaveAccessibleDescription(/The server receives its secrets/);
    await user.click(privateNetwork);
    const ask = await screen.findByRole("dialog", { name: "Allow private network?" });
    await user.click(within(ask).getByRole("button", { name: "Allow" }));
    await vi.waitFor(() => expect(within(form).getByRole("switch", { name: "Allow private network" })).toBeChecked());
    // Turning it back off needs no confirmation.
    await user.click(within(form).getByRole("switch", { name: "Allow private network" }));
    expect(within(form).getByRole("switch", { name: "Allow private network" })).not.toBeChecked();
    expect(screen.queryByRole("dialog", { name: "Allow private network?" })).toBeNull();
    await user.click(within(form).getByRole("switch", { name: "Allow private network" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Allow private network?" })).getByRole("button", { name: "Allow" }));
    await vi.waitFor(() => expect(within(form).getByRole("switch", { name: "Allow private network" })).toBeChecked());
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(screen.queryByRole("form", { name: "Edit server" })).toBeNull());
    // The stored Authorization value was left blank, so it is not sent and stays as it is.
    expect(calls.find((c) => c.method === "PATCH")).toMatchObject({
      path: "/v1/mcp/servers/crm",
      body: { label: "CRM", url: "https://crm.example.com/v2/mcp", allow_private_network: true, secrets: { "X-Team": "sales" } },
    });
  });

  it("shows the daemon's message when a change needs local access", async () => {
    const user = userEvent.setup();
    setup("/settings/plugins?tab=mcps", { servers: servers(), refuseMcpWrites: true });
    const list = await screen.findByRole("list", { name: "MCP servers" });
    await user.click(within(within(list).getByText("Files").closest("li")!).getByRole("switch", { name: "Enable Files" }));
    const expected = new ApiError(403, LOCAL_REFUSAL.code, LOCAL_REFUSAL.message).message;
    expect(await screen.findByRole("alert")).toHaveTextContent(expected);
  });
});

describe("splitCommand", () => {
  it("splits like a shell without expanding anything", () => {
    expect(splitCommand(`uvx blender-mcp`)).toEqual(["uvx", "blender-mcp"]);
    expect(splitCommand(`a "b c" 'd "e"' f\\ g "h\\"i" ~ $X`)).toEqual(["a", "b c", 'd "e"', "f g", 'h"i', "~", "$X"]);
    expect(splitCommand(`a ""`)).toEqual(["a", ""]);
    expect(splitCommand(`a "b`)).toBeNull();
    expect(splitCommand(`a \\`)).toBeNull();
  });

  it("round-trips through joinCommand", () => {
    const argv = ["node", "my server.js", "it's", "", "--flag=a b"];
    expect(splitCommand(joinCommand(argv))).toEqual(argv);
  });
});
