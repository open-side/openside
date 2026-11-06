import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MemoryFile, MemoryFileInfo } from "../../api/types";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { ToastProvider } from "../../ui";
import { MemoryOverview } from "./Memory";

const HOUR = 3_600_000;
const MEMORY_MD = '# MEMORY\n\n<!-- memory id=mem_a -->\nProject uses Rust\n\n<!-- memory id=mem_b tags=["work"] -->\nShip on Fridays\n';

const INFOS: MemoryFileInfo[] = [
  { name: "MEMORY.md", view: "memory", editable: true, items: 2, updated_at: new Date(Date.now() - 2 * HOUR).toISOString() },
  { name: "USER.md", view: "user", editable: true, items: 0, updated_at: null },
  { name: "TAXONOMY.md", view: "taxonomy", editable: false, items: 1, updated_at: new Date(Date.now() - 2 * HOUR).toISOString() },
];

function file(name: string, markdown: string, revision = 7): MemoryFile {
  const info = INFOS.find((entry) => entry.name === name)!;
  return { name, view: info.view, editable: info.editable, updated_at: info.updated_at, revision, markdown };
}

const FILES: Record<string, MemoryFile> = {
  "MEMORY.md": file("MEMORY.md", MEMORY_MD),
  "USER.md": file("USER.md", "# USER\n"),
  "TAXONOMY.md": file("TAXONOMY.md", "# TAXONOMY\n\n## tags\n\n- work: 1\n\n## kinds\n\n- note: 2\n- history: 0\n"),
};

interface Device {
  /** The settings document's verdict: true for a paired device without local authority. */
  remote?: boolean;
  reveal?: () => Response;
}

function setup(put?: (call: Call) => Response, files: Record<string, MemoryFile> = FILES, device: Device = {}) {
  const fake = fakeClient((call) => {
    if (call.path === "/v1/settings/config")
      return json(200, { config: {}, restart_required: false, file_only: [], remote: device.remote ?? true, remote_editable: [], schema: {} });
    if (call.path === "/v1/memory/reveal") return device.reveal?.() ?? new Response(null, { status: 204 });
    if (call.path === "/v1/memory/files") return json(200, { files: INFOS });
    const name = decodeURIComponent(call.path.replace("/v1/memory/files/", ""));
    if (call.method === "PUT" && put) return put(call);
    if (files[name]) return json(200, files[name]);
    return json(404, { code: "not_found", message: "nope" });
  });
  render(
    <ClientContext.Provider value={fake.client}>
      <ToastProvider>
        <MemoryOverview />
      </ToastProvider>
    </ClientContext.Provider>,
  );
  return fake;
}

afterEach(() => vi.restoreAllMocks());

describe("Memory › Overview", () => {
  it("lists the three files and renders the open one without its note markers", async () => {
    const user = userEvent.setup();
    setup();
    const list = await screen.findByRole("list", { name: "Memory files" });
    expect(within(list).getAllByRole("button").map((row) => row.textContent)).toEqual(["MEMORY.md", "USER.md", "TAXONOMY.md"]);
    expect(within(list).getByRole("button", { name: "MEMORY.md" })).toHaveAttribute("aria-current", "true");

    const pane = await screen.findByRole("region", { name: "MEMORY.md" });
    expect(await within(pane).findByText("Project uses Rust")).toBeInTheDocument();
    expect(within(pane).getByText("Ship on Fridays")).toBeInTheDocument();
    expect(pane).not.toHaveTextContent("<!-- memory");
    expect(within(pane).getByText("2 hours ago")).toBeInTheDocument();
    expect(within(pane).getByRole("button", { name: "Edit" })).toBeInTheDocument();

    await user.click(within(list).getByRole("button", { name: "USER.md" }));
    expect(await screen.findByText("Nothing saved here yet.")).toBeInTheDocument();

    await user.click(within(list).getByRole("button", { name: "TAXONOMY.md" }));
    const taxonomy = await screen.findByRole("region", { name: "TAXONOMY.md" });
    expect(await within(taxonomy).findByText("work: 1")).toBeInTheDocument();
    expect(within(taxonomy).queryByRole("button", { name: "Edit" })).toBeNull();
  });

  it("edits the raw markdown, markers included, and saves it with the revision it started from", async () => {
    const user = userEvent.setup();
    const edited = `${MEMORY_MD}\n<!-- memory -->\nShip on Mondays\n`;
    const { calls } = setup(() => json(200, { created: 1, updated: 0, deleted: 0, file: file("MEMORY.md", edited, 8) }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    const editor = screen.getByRole("textbox", { name: "Edit MEMORY.md" });
    expect(editor).toHaveValue(MEMORY_MD);
    expect(screen.getByText(/mark where each note starts/)).toBeInTheDocument();
    fireEvent.change(editor, { target: { value: edited } });
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(calls.find((call) => call.method === "PUT")).toMatchObject({ path: "/v1/memory/files/MEMORY.md", body: { markdown: edited, revision: 7 } });
    expect(await screen.findByText("Ship on Mondays")).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByText("Saved MEMORY.md.")).toBeInTheDocument();
    // The list reloads so its times follow the save.
    await waitFor(() => expect(calls.filter((call) => call.path === "/v1/memory/files")).toHaveLength(2));
  });

  it("explains a conflict and reloads the latest version into the editor", async () => {
    const user = userEvent.setup();
    const files = { ...FILES };
    setup(() => json(409, { code: "memory_conflict", message: "changed" }), files);
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "# MEMORY\nmine\n" } });
    await user.click(screen.getByRole("button", { name: "Save" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("changed elsewhere while you were editing");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();

    files["MEMORY.md"] = file("MEMORY.md", "# MEMORY\n\n<!-- memory id=mem_c -->\nTheirs\n", 9);
    await user.click(within(alert).getByRole("button", { name: "Reload" }));
    await waitFor(() => expect(screen.getByRole("textbox")).toHaveValue("# MEMORY\n\n<!-- memory id=mem_c -->\nTheirs\n"));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  it("shows the daemon's message when it refuses an edit", async () => {
    const user = userEvent.setup();
    setup(() => json(400, { code: "redacted_memory", message: "A note still shows a masked value; edit that note without the mask." }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("A note still shows a masked value");
    expect(screen.getByRole("textbox")).toBeInTheDocument();
  });

  it("downloads the three files on a device that cannot reach the folder", async () => {
    const user = userEvent.setup();
    const names: string[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      names.push(this.download);
    });
    const { calls } = setup(undefined, FILES, { remote: true });
    const button = await screen.findByRole("button", { name: "Download files" });
    expect(screen.queryByRole("button", { name: "Open folder" })).toBeNull();
    expect(screen.getByText("The folder opens only on the computer running openside. From this device, download the files instead.")).toBeInTheDocument();
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    expect(await screen.findByText("Saved the memory files to your downloads.")).toBeInTheDocument();
    expect(names).toEqual(["MEMORY.md", "USER.md", "TAXONOMY.md"]);
    expect(calls.filter((call) => call.path.startsWith("/v1/memory/files/")).map((call) => call.path)).toEqual(
      expect.arrayContaining(["/v1/memory/files/USER.md", "/v1/memory/files/TAXONOMY.md"]),
    );
    expect(calls.some((call) => call.path === "/v1/memory/reveal")).toBe(false);
  });

  it("opens the folder on the computer running openside and says the copy is not read back", async () => {
    const user = userEvent.setup();
    const { calls } = setup(undefined, FILES, { remote: false });
    const button = await screen.findByRole("button", { name: "Open folder" });
    expect(screen.queryByRole("button", { name: "Download files" })).toBeNull();
    expect(screen.getByText(/Changes made there aren't read back, so edit them here/)).toBeInTheDocument();
    await user.click(button);
    await waitFor(() => expect(calls.filter((call) => call.method === "POST").map((call) => call.path)).toEqual(["/v1/memory/reveal"]));
    await waitFor(() => expect(button).toBeEnabled());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("tells the person when the computer has no file manager to open", async () => {
    const user = userEvent.setup();
    setup(undefined, FILES, { remote: false, reveal: () => json(503, { code: "file_manager_unavailable", message: "no file manager" }) });
    await user.click(await screen.findByRole("button", { name: "Open folder" }));
    expect(await screen.findByText("openside could not open a file manager on this computer.")).toBeInTheDocument();
  });
});
