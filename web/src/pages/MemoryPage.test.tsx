import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Memory } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { MemoryPage } from "./MemoryPage";

function memory(id: string, content: string, extra: Partial<Memory> = {}): Memory {
  return {
    id,
    kind: "note",
    content,
    tags: [],
    retention: "permanent",
    created_at: "2026-10-01T00:00:00Z",
    updated_at: "2026-10-01T00:00:00Z",
    version: 1,
    ...extra,
  };
}

function daemon() {
  const memories = [memory("m1", "Prefers window seats", { tags: ["travel"] }), memory("m2", "Visited docs", { kind: "history", title: "Docs" })];
  let next = 3;
  return fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    const [, , , id, verb] = url.pathname.split("/");
    if (url.pathname === "/v1/memory/health") return json(200, { mode: "lexical_only", count: memories.length, next_step: "openside memory model fetch" });
    if (url.pathname === "/v1/memory" && call.method === "GET") {
      const q = url.searchParams.get("q")?.toLowerCase() ?? "";
      const kind = url.searchParams.get("kind");
      const hits = memories
        .filter((m) => m.content.toLowerCase().includes(q) && (!kind || m.kind === kind))
        .map((m) => ({ memory: m, score: 1, matched: q ? "lexical" : "recent" }));
      return json(200, { mode: "lexical_only", hits });
    }
    if (url.pathname === "/v1/memory" && call.method === "POST") {
      const body = call.body as { content: string; tags: string[]; retention: Memory["retention"] };
      if (body.content.includes("sk-")) return json(400, { code: "secret_detected", message: "secret" });
      const created = memory(`m${next++}`, body.content, { tags: body.tags, retention: body.retention });
      memories.push(created);
      return json(201, created);
    }
    const target = memories.find((m) => m.id === id);
    if (!target) return json(404, { code: "not_found", message: "nope" });
    if (verb === "history") {
      return json(200, {
        id,
        versions: [
          { op: "create", actor: "agent:s1", at: "2026-10-01T00:00:00Z", content: "Prefers aisle seats" },
          { op: "update", actor: "api", at: "2026-10-02T00:00:00Z", content: "Prefers window seats" },
        ],
      });
    }
    if (call.method === "PATCH") {
      Object.assign(target, call.body, { version: target.version + 1 });
      return json(200, target);
    }
    if (call.method === "DELETE") {
      memories.splice(memories.indexOf(target), 1);
      return json(200, target);
    }
    return json(200, target);
  });
}

function setup() {
  const fake = daemon();
  render(
    <ClientContext.Provider value={fake.client}>
      <MemoryPage />
    </ClientContext.Provider>,
  );
  return fake;
}

describe("MemoryPage", () => {
  it("shows status, searches with kind filter, and shows history", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    expect(await screen.findByLabelText("Memory status")).toHaveTextContent("2 memories · keyword search");
    expect(screen.getByText("openside memory model fetch")).toBeInTheDocument();
    const list = await screen.findByRole("list", { name: "Memories" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);

    await user.click(within(screen.getByRole("tablist", { name: "Kind" })).getByRole("tab", { name: "Notes" }));
    await user.type(screen.getByPlaceholderText("Search memory"), "window");
    await vi.waitFor(() => expect(calls.at(-1)?.path).toBe("/v1/memory?q=window&kind=note"));
    await vi.waitFor(() => expect(within(screen.getByRole("list", { name: "Memories" })).getAllByRole("listitem")).toHaveLength(1));

    await user.click(screen.getByRole("button", { name: "Change history" }));
    const history = await screen.findByRole("list", { name: "History of m1" });
    expect(within(history).getByText("Prefers aisle seats")).toBeInTheDocument();
    expect(within(history).getByText(/by agent:s1/)).toBeInTheDocument();
  });

  it("creates with tags and retention, edits retention, deletes, and reports a refused secret", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await screen.findByRole("list", { name: "Memories" });
    await user.click(screen.getByRole("button", { name: "New memory" }));
    let form = await screen.findByRole("form", { name: "New memory" });
    await user.type(within(form).getByLabelText("Content"), "Dentist on Fridays");
    await user.type(within(form).getByLabelText("Tags (comma separated)"), "health, calendar ,");
    await user.selectOptions(within(form).getByLabelText("Retention"), "days_30");
    await user.click(within(form).getByRole("button", { name: "Remember" }));
    expect(await screen.findByText("Dentist on Fridays")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ content: "Dentist on Fridays", tags: ["health", "calendar"], retention: "days_30" });

    const row = screen.getByText("Prefers window seats").closest("article") as HTMLElement;
    expect(row).toHaveAttribute("data-memory", "m1");
    await user.click(within(row).getByRole("button", { name: "Options for Prefers window seats" }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const editor = await screen.findByRole("form", { name: "Edit memory m1" });
    await user.selectOptions(within(editor).getByLabelText("Retention"), "days_90");
    await user.click(within(editor).getByRole("button", { name: "Save" }));
    expect(await within(row).findByText("90 days after last edit")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "PATCH")).toMatchObject({ path: "/v1/memory/m1", body: { retention: "days_90", tags: ["travel"] } });

    await user.click(within(row).getByRole("button", { name: "Options for Prefers window seats" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const confirm = await screen.findByRole("dialog", { name: "Delete memory?" });
    await user.click(within(confirm).getByRole("button", { name: "Delete" }));
    await vi.waitFor(() => expect(screen.queryByText("Prefers window seats")).toBeNull());
    expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/memory/m1")).toBe(true);

    await user.click(screen.getByRole("button", { name: "New memory" }));
    form = await screen.findByRole("form", { name: "New memory" });
    await user.type(within(form).getByLabelText("Content"), "key sk-123");
    await user.click(within(form).getByRole("button", { name: "Remember" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("That text looks like a secret, so it was not stored.");
  });
});
