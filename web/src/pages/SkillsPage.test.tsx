import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { SkillDetail } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { SkillsPage } from "./SkillsPage";

function detail(name: string, extra: Partial<SkillDetail> = {}): SkillDetail {
  return {
    name,
    description: `${name} helper`,
    body: `# ${name}`,
    enabled: true,
    source: "user",
    resources: [],
    allowed_tools: [],
    ...extra,
  } as SkillDetail;
}

/** In-memory `/v1/skills` with the same create/patch/delete semantics the screen relies on. */
function daemon() {
  const skills = new Map([["pdf-tools", detail("pdf-tools")]]);
  return fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    const name = decodeURIComponent(url.pathname.split("/")[3] ?? "");
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

function setup() {
  const fake = daemon();
  render(
    <ClientContext.Provider value={fake.client}>
      <ToastProvider>
        <SkillsPage />
      </ToastProvider>
    </ClientContext.Provider>,
  );
  return fake;
}

describe("SkillsPage", () => {
  it("lists skills and rejected folders, then creates, toggles, edits and deletes a skill", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const list = await screen.findByRole("list", { name: "Skills" });
    expect(within(list).getByText("/pdf-tools")).toBeInTheDocument();
    expect(screen.getByText("invalid name")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "New skill" }));
    const form = await screen.findByRole("form", { name: "New skill" });
    await user.type(within(form).getByLabelText("Name"), "release-notes");
    await user.type(within(form).getByLabelText("Description"), "Draft release notes");
    await user.click(within(form).getByRole("button", { name: "Create" }));
    const created = await within(list).findByText("/release-notes");
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "release-notes",
      description: "Draft release notes",
      body: "",
      resources: [],
    });

    const row = created.closest("article")!;
    expect(row).toHaveAttribute("data-skill", "release-notes");
    const toggle = within(row as HTMLElement).getByRole("switch", { name: "Enable /release-notes" });
    expect(toggle).toBeChecked();
    await user.click(toggle);
    await vi.waitFor(() => expect(within(row as HTMLElement).getByRole("switch", { name: "Enable /release-notes" })).not.toBeChecked());
    expect(calls.find((c) => c.method === "PATCH")).toMatchObject({ path: "/v1/skills/release-notes", body: { enabled: false } });

    await user.click(within(row as HTMLElement).getByRole("button", { name: "Options for /release-notes" }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const editor = await screen.findByRole("form", { name: "Edit release-notes" });
    await within(editor).findByDisplayValue("Draft release notes");
    const description = within(editor).getByLabelText("Description");
    await user.clear(description);
    await user.type(description, "Write the changelog");
    await user.click(within(editor).getByRole("button", { name: "Save" }));
    expect(await within(list).findByText("Write the changelog")).toBeInTheDocument();

    await user.click(within(row as HTMLElement).getByRole("button", { name: "Options for /release-notes" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const confirm = await screen.findByRole("dialog", { name: "Delete skill?" });
    await user.click(within(confirm).getByRole("button", { name: "Delete" }));
    await vi.waitFor(() => expect(within(list).queryByText("/release-notes")).toBeNull());
    expect(calls.at(-2)).toMatchObject({ method: "DELETE", path: "/v1/skills/release-notes" });
  });

  it("shows the daemon's error when a create is refused", async () => {
    const user = userEvent.setup();
    setup();
    await screen.findByRole("list", { name: "Skills" });
    await user.click(screen.getByRole("button", { name: "New skill" }));
    const form = await screen.findByRole("form", { name: "New skill" });
    await user.type(within(form).getByLabelText("Name"), "pdf-tools");
    await user.type(within(form).getByLabelText("Description"), "dup");
    await user.click(within(form).getByRole("button", { name: "Create" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("A skill with that name exists");
  });
});
