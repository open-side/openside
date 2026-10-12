import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";
import type { Session } from "../api/types";
import { ClientContext, useClient } from "../context";
import { useSessionList } from "../hooks/useSessionList";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { Sidebar } from "./Sidebar";

function session(id: string, title: string, extra: Partial<Session> = {}): Session {
  return {
    id,
    title,
    model: "provider:model",
    mode: "guard",
    status: "idle",
    last_seq: 1,
    queue: [],
    pinned: false,
    archived: false,
    incognito: false,
    ...extra,
  };
}

/** Minimal daemon double applying the same list rules as `GET /v1/sessions`. */
function daemon() {
  const sessions = [
    session("a", "Groceries", { status: "waiting_human" }),
    session("b", "Quarterly report", { status: "running" }),
    session("c", "Old trip", { archived: true }),
  ];
  return fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (call.method === "PATCH") {
      const target = sessions.find((s) => s.id === url.pathname.split("/").pop());
      Object.assign(target!, call.body);
      return json(200, target);
    }
    const q = url.searchParams.get("q")?.toLowerCase();
    const archived = url.searchParams.get("archived") === "true";
    const listed = sessions
      .filter((s) => s.archived === archived && (!q || s.title.toLowerCase().includes(q)))
      .sort((x, y) => Number(y.pinned) - Number(x.pinned));
    return json(200, { sessions: listed });
  });
}

function Harness() {
  const list = useSessionList(useClient());
  return <Sidebar list={list} />;
}

function setup(path = "/") {
  const fake = daemon();
  render(
    <ClientContext.Provider value={fake.client}>
      <MemoryRouter initialEntries={[path]}>
        <Harness />
      </MemoryRouter>
    </ClientContext.Provider>,
  );
  return fake;
}

/** Row titles in display order across the Pinned and Conversations groups. */
async function titles(): Promise<string[]> {
  await screen.findAllByRole("list", { name: /Conversations/i });
  return Array.from(document.querySelectorAll("[data-session] .session-title"), (title) => title.textContent ?? "");
}

function row(title: string): HTMLElement {
  return screen.getByText(title).closest("li") as HTMLElement;
}

async function choose(user: ReturnType<typeof userEvent.setup>, title: string, item: string) {
  await user.click(within(row(title)).getByRole("button", { name: `Options for ${title}` }));
  await user.click(await screen.findByRole("menuitem", { name: item }));
}

describe("Sidebar conversation list", () => {
  it("lists active conversations with run status and searches through the API", async () => {
    const user = userEvent.setup();
    const { calls } = setup("/sessions/b");
    expect(await titles()).toEqual(["Groceries", "Quarterly report"]);
    expect(within(row("Groceries")).getByRole("img", { name: "Needs you" })).toBeInTheDocument();
    expect(within(row("Quarterly report")).getByRole("img", { name: "Running" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Quarterly report/ })).toHaveAttribute("aria-current", "page");

    await user.type(screen.getByRole("searchbox", { name: "Search conversations" }), "report");
    await waitFor(() => expect(calls.at(-1)?.path).toBe("/v1/sessions?q=report"));
    await waitFor(async () => expect(await titles()).toEqual(["Quarterly report"]));
    expect(screen.getByRole("link", { name: /Quarterly report/ })).toHaveAttribute("href", "/sessions/b");
  });

  it("pins a conversation into the Pinned group and archives another", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    expect(await titles()).toEqual(["Groceries", "Quarterly report"]);
    await choose(user, "Quarterly report", "Pin conversation");
    await waitFor(async () => expect(await titles()).toEqual(["Quarterly report", "Groceries"]));
    expect(calls.find((c) => c.method === "PATCH")).toMatchObject({ path: "/v1/sessions/b", body: { pinned: true } });
    const pinned = screen.getByRole("list", { name: "Pinned conversations" });
    expect(within(pinned).getByRole("img", { name: "Pinned" })).toBeInTheDocument();

    await choose(user, "Groceries", "Archive");
    await waitFor(async () => expect(await titles()).toEqual(["Quarterly report"]));

    await user.click(screen.getByRole("tab", { name: "Archived" }));
    await waitFor(async () => expect(await titles()).toEqual(["Groceries", "Old trip"]));
    expect(calls.at(-1)?.path).toBe("/v1/sessions?archived=true");
    await choose(user, "Old trip", "Unarchive");
    await waitFor(async () => expect(await titles()).toEqual(["Groceries"]));
  });

  it("renames a conversation from its menu", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await titles();
    await choose(user, "Groceries", "Rename");
    const dialog = await screen.findByRole("dialog", { name: "Rename conversation" });
    const input = within(dialog).getByLabelText("Conversation name");
    await user.clear(input);
    await user.type(input, "Weekly groceries");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(async () => expect(await titles()).toEqual(["Weekly groceries", "Quarterly report"]));
    expect(calls.find((c) => c.method === "PATCH")).toMatchObject({ path: "/v1/sessions/a", body: { title: "Weekly groceries" } });
  });

  it("links Workspace and Settings and a new chat", () => {
    setup("/sessions/a");
    const nav = screen.getByRole("navigation", { name: "Sections" });
    expect(within(nav).getAllByRole("link").map((link) => link.textContent)).toEqual(["Workspace", "Settings"]);
    expect(within(nav).getByRole("link", { name: "Workspace" })).toHaveAttribute("href", "/workspace");
    expect(within(nav).getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/settings");
    expect(screen.getByRole("link", { name: "New chat" })).toHaveAttribute("href", "/");
  });
});
