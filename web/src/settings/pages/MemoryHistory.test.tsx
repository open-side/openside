import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { MemoryChange } from "../../api/types";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { MemoryHistory } from "./MemoryHistory";

const AT = new Date(Date.now() - 3 * 3_600_000).toISOString();

function change(seq: number, patch: Partial<MemoryChange>): MemoryChange {
  return { seq, memory_id: `mem_${seq}`, op: "update", actor: "person", at: AT, version: 1, ...patch };
}

function setup(handler: (call: Call) => Response) {
  const fake = fakeClient(handler);
  render(
    <ClientContext.Provider value={fake.client}>
      <MemoryHistory />
    </ClientContext.Provider>,
  );
  return fake;
}

describe("Memory › History", () => {
  it("says so when nothing has changed yet", async () => {
    setup(() => json(200, { changes: [] }));
    expect(await screen.findByText("No memory history yet.")).toBeInTheDocument();
    expect(screen.getByText("Select a history entry to view its details.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Load older" })).toBeNull();
  });

  it("lists changes newest first, loads older ones and shows the chosen entry", async () => {
    const user = userEvent.setup();
    const { calls } = setup((call) => {
      if (call.path === "/v1/memory/history?limit=50")
        return json(200, {
          changes: [
            change(5, { op: "update", content: "Project uses Rust 1.80", tags: ["work", "lang"], kind: "note", version: 2, actor: "agent:chief" }),
            change(4, { op: "delete", content: null, tags: null, memory_id: "mem_old", version: 3 }),
          ],
          next_before: 4,
        });
      if (call.path === "/v1/memory/history?limit=50&before=4") return json(200, { changes: [change(2, { op: "create", content: "Ship on Fridays", tags: [], kind: "note" })] });
      return json(404, { code: "not_found", message: "nope" });
    });

    const list = await screen.findByRole("list", { name: "Memory changes" });
    const rows = () => within(list).getAllByRole("button");
    expect(rows().map((row) => row.querySelector(".memory-change-op")?.textContent)).toEqual(["Updated", "Deleted"]);
    expect(rows()[0]).toHaveTextContent("Project uses Rust 1.80");
    expect(rows()[0]).toHaveTextContent("agent:chief");
    expect(rows()[0]).toHaveTextContent("3 hours ago");
    expect(rows()[1]).toHaveTextContent("Content removed");

    await user.click(screen.getByRole("button", { name: "Load older" }));
    expect(await within(list).findByText("Ship on Fridays")).toBeInTheDocument();
    expect(rows().map((row) => row.querySelector(".memory-change-op")?.textContent)).toEqual(["Updated", "Deleted", "Created"]);
    expect(calls.map((call) => call.path)).toEqual(["/v1/memory/history?limit=50", "/v1/memory/history?limit=50&before=4"]);
    expect(screen.queryByRole("button", { name: "Load older" })).toBeNull();

    await user.click(rows()[0]!);
    expect(rows()[0]).toHaveAttribute("aria-current", "true");
    const detail = screen.getByRole("region", { name: "Change details" });
    expect(within(detail).getByRole("heading", { name: "Updated" })).toBeInTheDocument();
    expect(detail).toHaveTextContent("Changed byagent:chief");
    expect(detail).toHaveTextContent("Memorymem_5");
    expect(detail).toHaveTextContent("Version2");
    expect(detail).toHaveTextContent("TypeNote");
    expect(detail).toHaveTextContent("Tagswork, lang");
    expect(within(detail).getByText("Project uses Rust 1.80")).toBeInTheDocument();
    expect(screen.queryByText("Select a history entry to view its details.")).toBeNull();

    await user.click(rows()[1]!);
    const scrubbed = screen.getByRole("region", { name: "Change details" });
    expect(within(scrubbed).getByRole("heading", { name: "Deleted" })).toBeInTheDocument();
    expect(scrubbed).toHaveTextContent("The content was removed when this memory was deleted or expired.");
    expect(scrubbed).not.toHaveTextContent("Tags");
  });

  it("shows why the list could not load", async () => {
    setup(() => json(500, { code: "internal", message: "database is locked" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
  });
});
