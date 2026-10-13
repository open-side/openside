import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ExternalAgent } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { AgentsPage } from "./AgentsPage";

function setup(override: (call: Call) => Response | undefined = () => undefined) {
  const agents: ExternalAgent[] = [
    { id: "claude", label: "Claude Code", detected: true, skill_installed: false, skills_dir: "~/.claude/skills", custom: false },
    { id: "my-agent", label: "my-agent", detected: false, skill_installed: false, skills_dir: "~/agents/skills", custom: true },
  ];
  const fake = fakeClient((call) => {
    const custom = override(call);
    if (custom) return custom;
    if (call.path === "/v1/external-agents") return json(200, { agents });
    if (call.method === "PUT" && call.path.startsWith("/v1/external-agents/")) {
      const id = call.path.split("/").pop()!;
      const { skills_dir } = call.body as { skills_dir: string };
      const known = agents.find((agent) => agent.id === id);
      if (known) known.skills_dir = skills_dir;
      else agents.push({ id, label: id, detected: false, skill_installed: false, skills_dir, custom: true });
      return json(200, agents.find((agent) => agent.id === id));
    }
    if (call.method === "DELETE") {
      agents.splice(1, 1);
      return json(200, { agents });
    }
    return json(404, { code: "not_found", message: "nope" });
  });
  render(
    <ClientContext.Provider value={fake.client}>
      <AgentsPage />
    </ClientContext.Provider>,
  );
  return fake;
}

describe("AgentsPage", () => {
  it("adds another agent by its skills folder and lists it", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await screen.findByRole("list", { name: "Agents" });
    await user.click(screen.getByRole("button", { name: "Add agent" }));
    const dialog = await screen.findByRole("dialog", { name: "Add an agent" });
    await user.type(within(dialog).getByLabelText("Agent name"), "gemini-cli");
    await user.type(within(dialog).getByLabelText("Skills folder"), "~/.gemini/skills");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await screen.findByText("gemini-cli")).toBeInTheDocument();
    expect(calls.find((call) => call.method === "PUT")).toMatchObject({ path: "/v1/external-agents/gemini-cli", body: { skills_dir: "~/.gemini/skills" } });
  });

  it("changes the folder of an added agent but not a built-in one", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const list = await screen.findByRole("list", { name: "Agents" });
    expect(within(list).queryByRole("button", { name: "Options for Claude Code" })).toBeNull();
    await user.click(within(list).getByRole("button", { name: "Options for my-agent" }));
    await user.click(await screen.findByRole("menuitem", { name: "Change skills folder" }));
    const dialog = await screen.findByRole("dialog", { name: "Skills folder of my-agent" });
    expect(within(dialog).getByLabelText("Agent name")).toHaveAttribute("readonly");
    const folder = within(dialog).getByLabelText("Skills folder");
    await user.clear(folder);
    await user.type(folder, "~/agents/new-skills");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/new-skills/)).toBeInTheDocument();
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({ skills_dir: "~/agents/new-skills" });
  });

  it("forgets an added agent after confirmation", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const list = await screen.findByRole("list", { name: "Agents" });
    await user.click(within(list).getByRole("button", { name: "Options for my-agent" }));
    await user.click(await screen.findByRole("menuitem", { name: "Forget agent" }));
    const confirm = await screen.findByRole("dialog", { name: "Forget my-agent?" });
    await user.click(within(confirm).getByRole("button", { name: "Forget" }));
    await within(list).findByText("Claude Code");
    expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/external-agents/my-agent")).toBe(true);
    expect(within(await screen.findByRole("list", { name: "Agents" })).queryByText("my-agent")).toBeNull();
  });

  it("says a name is invalid before asking the daemon, and explains local-only refusals", async () => {
    const user = userEvent.setup();
    const { calls } = setup((call) => (call.method === "PUT" ? json(403, { code: "local_access_required", message: "local" }) : undefined));
    await screen.findByRole("list", { name: "Agents" });
    await user.click(screen.getByRole("button", { name: "Add agent" }));
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
});
