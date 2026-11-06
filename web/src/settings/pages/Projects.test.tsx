import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Project } from "../../api/types";
import { ClientContext } from "../../context";
import { fakeClient, json } from "../../test/fakeDaemon";
import { ToastProvider } from "../../ui";
import { ProjectsSettings } from "./Projects";

const WEBSITE: Project = {
  id: "p1",
  name: "Website",
  root: "/home/me/code/website",
  scratch: false,
  needs_confirmation: false,
  instructions: "Use pnpm.",
  created_at: "2026-11-06T00:00:00Z",
  updated_at: "2026-11-06T00:00:00Z",
};

function setup({ remote = false, projects = [WEBSITE], picked = null }: { remote?: boolean; projects?: Project[]; picked?: string | null } = {}) {
  const list = [...projects];
  const fake = fakeClient((call) => {
    const path = call.path;
    if (path === "/v1/settings/config") return json(200, { config: {}, restart_required: false, file_only: [], remote, remote_editable: [], schema: {} });
    if (path === "/v1/folders/pick") return json(200, { path: picked });
    if (path === "/v1/projects" && call.method === "GET") return json(200, { projects: list });
    if (path === "/v1/projects" && call.method === "POST") {
      const body = call.body as { name: string; root?: string; instructions?: string };
      const created: Project = { ...WEBSITE, id: "p2", name: body.name, root: body.root ?? "/data/projects/p2", scratch: !body.root, instructions: body.instructions ?? "" };
      list.push(created);
      return json(201, created);
    }
    const one = /^\/v1\/projects\/([^/]+)$/.exec(path);
    if (one && call.method === "PATCH") {
      const index = list.findIndex((project) => project.id === one[1]);
      const { confirm_root, ...changes } = call.body as Partial<Project> & { confirm_root?: boolean };
      list[index] = { ...list[index]!, ...changes, ...(confirm_root ? { needs_confirmation: false } : {}) };
      return json(200, list[index]);
    }
    if (one && call.method === "DELETE") {
      list.splice(
        list.findIndex((project) => project.id === one[1]),
        1,
      );
      return json(200, { projects: list });
    }
    return json(404, { code: "not_found", message: "nope" });
  });
  render(
    <ClientContext.Provider value={fake.client}>
      <ToastProvider>
        <ProjectsSettings />
      </ToastProvider>
    </ClientContext.Provider>,
  );
  return fake;
}

const writes = (calls: { method: string; path: string; body: unknown }[]) => calls.filter((call) => call.method !== "GET").map((call) => [call.method, call.path, call.body]);

describe("Settings › Projects", () => {
  it("lists the projects under Create, each with its folder", async () => {
    setup();
    const list = await screen.findByRole("list", { name: "Projects" });
    expect(within(list).getByText("Website")).toBeInTheDocument();
    expect(within(list).getByText("/home/me/code/website")).toBeInTheDocument();
  });

  it("creates a project from a folder chosen in this computer's folder picker", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ projects: [], picked: "/home/me/code/notes" });
    const create = screen.getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toBeEnabled());
    await user.click(create);
    const dialog = await screen.findByRole("dialog", { name: "Create project" });
    await user.click(within(dialog).getByRole("button", { name: "Choose…" }));
    await waitFor(() => expect(within(dialog).getByLabelText("Folder")).toHaveValue("/home/me/code/notes"));
    expect(within(dialog).getByLabelText("Name")).toHaveValue("notes");
    await user.type(within(dialog).getByLabelText("Instructions"), "Answer in short notes.");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));

    const list = await screen.findByRole("list", { name: "Projects" });
    expect(within(list).getByText("notes")).toBeInTheDocument();
    expect(writes(calls)).toEqual([
      ["POST", "/v1/folders/pick", undefined],
      ["POST", "/v1/projects", { name: "notes", root: "/home/me/code/notes", instructions: "Answer in short notes." }],
    ]);
  });

  it("starts a project from scratch when no folder is given", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ projects: [] });
    const create = screen.getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toBeEnabled());
    await user.click(create);
    const dialog = await screen.findByRole("dialog", { name: "Create project" });
    await user.type(within(dialog).getByLabelText("Name"), "Ideas");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(await screen.findByRole("list", { name: "Projects" })).findByText("/data/projects/p2")).toBeInTheDocument();
    expect(writes(calls)).toEqual([["POST", "/v1/projects", { name: "Ideas" }]]);
  });

  it("keeps Create to the computer running openside and says so", async () => {
    const { calls } = setup({ remote: true });
    expect(await screen.findByText(/Projects are created on the computer running openside/)).toBeInTheDocument();
    const create = screen.getByRole("button", { name: "Create" });
    expect(create).toBeDisabled();
    expect(create).toHaveAccessibleDescription(/Projects are created on the computer running openside/);
    // Existing projects are still listed with their menu.
    expect(await screen.findByRole("button", { name: /Website/ })).toBeInTheDocument();
    expect(writes(calls)).toEqual([]);
  });

  it("edits a project's name and instructions, and deletes it after confirmation", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ remote: true });
    await user.click(await screen.findByRole("button", { name: /Website/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit project" });
    expect(within(dialog).queryByLabelText("Folder")).toBeNull();
    const name = within(dialog).getByLabelText("Name");
    await user.clear(name);
    await user.type(name, "Site");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(await screen.findByRole("list", { name: "Projects" })).findByText("Site")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Site/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const confirm = await screen.findByRole("dialog", { name: "Delete Site?" });
    await user.click(within(confirm).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("list", { name: "Projects" })).toBeNull());
    expect(writes(calls)).toEqual([
      ["PATCH", "/v1/projects/p1", { name: "Site", instructions: "Use pnpm." }],
      ["DELETE", "/v1/projects/p1", undefined],
    ]);
  });

  it("badges a folder that needs confirming and confirms it on this computer", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ projects: [{ ...WEBSITE, needs_confirmation: true }] });
    const list = await screen.findByRole("list", { name: "Projects" });
    expect(within(list).getByText("Needs confirming")).toBeInTheDocument();
    expect(await within(list).findByText(/can't tell whether this is still the folder/)).toBeInTheDocument();
    await user.click(await within(list).findByRole("button", { name: "Confirm folder" }));
    await waitFor(() => expect(screen.queryByText("Needs confirming")).toBeNull());
    expect(within(screen.getByRole("list", { name: "Projects" })).getByText("Website")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Confirm folder" })).toBeNull();
    expect(writes(calls)).toEqual([["PATCH", "/v1/projects/p1", { confirm_root: true }]]);
  });

  it("shows the badge elsewhere but leaves confirming to the computer running openside", async () => {
    const { calls } = setup({ remote: true, projects: [{ ...WEBSITE, needs_confirmation: true }] });
    const list = await screen.findByRole("list", { name: "Projects" });
    expect(within(list).getByText("Needs confirming")).toBeInTheDocument();
    expect(await within(list).findByText(/once its folder is confirmed on the computer running openside/)).toBeInTheDocument();
    expect(within(list).queryByRole("button", { name: "Confirm folder" })).toBeNull();
    expect(writes(calls)).toEqual([]);
  });
});
