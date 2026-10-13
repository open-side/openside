import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "../api/types";
import { ClientContext } from "../context";
import { AgentsContext } from "../hooks/useAgents";
import { agentSummary } from "../test/agents";
import { fakeClient, json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { SessionMenu } from "./SessionMenu";

afterEach(cleanup);

const help = readFileSync(resolve(__dirname, "../../../docs/specs/app-help.md"), "utf8");
const session = { id: "ordinary/thread", title: "My conversation", pinned: false, archived: false };

function setup({ agents = [], failure = false, archived = false }: { agents?: AgentSummary[] | null; failure?: boolean; archived?: boolean } = {}) {
  const daemon = fakeClient(() => failure ? json(403, { code: "forbidden", message: "Permission denied" }) : new Response(null, { status: 204 }));
  const onChanged = vi.fn();
  const onDeleted = vi.fn();
  render(
    <ClientContext.Provider value={daemon.client}>
      <AgentsContext.Provider value={{ agents, error: null, offline: false, refresh: vi.fn() }}>
        <ToastProvider>
          <SessionMenu session={{ ...session, archived }} label="Conversation options" trigger={<button>Options</button>} onChanged={onChanged} onDeleted={onDeleted} />
        </ToastProvider>
      </AgentsContext.Provider>
    </ClientContext.Provider>,
  );
  return { ...daemon, onChanged, onDeleted };
}

async function openMenu(user: UserEvent) {
  await user.click(screen.getByRole("button", { name: "Options" }));
  return screen.findByRole("menu", { name: "Conversation options" });
}

describe("SessionMenu", () => {
  it("offers only documented ordinary-conversation actions", async () => {
    const user = userEvent.setup();
    setup();
    const menu = await openMenu(user);
    const actions = within(menu).getAllByRole("menuitem").map((item) => item.textContent);
    expect(actions).toEqual(["Pin conversation", "Rename", "Archive", "Model account…", "Copy link", "Copy session ID", "Delete conversation"]);
    for (const action of actions) expect(help).toContain(action);
    expect(within(menu).queryByRole("menuitem", { name: "Clear" })).toBeNull();
  });

  it.each([false, true])("requires confirmation before deleting an ordinary thread (archived=%s)", async (archived) => {
    const user = userEvent.setup();
    const daemon = setup({ archived });
    const menu = await openMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: "Delete conversation" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete conversation?" });
    expect(dialog).toHaveTextContent("This cannot be undone.");
    expect(daemon.calls).toEqual([]);
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(daemon.calls).toEqual([]);
    const reopened = await openMenu(user);
    await user.click(within(reopened).getByRole("menuitem", { name: "Delete conversation" }));
    const confirm = await screen.findByRole("dialog", { name: "Delete conversation?" });
    await user.click(within(confirm).getByRole("button", { name: "Delete conversation" }));
    await vi.waitFor(() => expect(daemon.onDeleted).toHaveBeenCalledOnce());
    expect(daemon.onChanged).toHaveBeenCalledOnce();
    expect(daemon.calls).toEqual([expect.objectContaining({ method: "DELETE", path: "/v1/sessions/ordinary%2Fthread" })]);
    expect(await screen.findByText("Conversation deleted")).toBeInTheDocument();
  });

  it("keeps a failed delete visible without pretending the thread was removed", async () => {
    const user = userEvent.setup();
    const daemon = setup({ failure: true });
    const menu = await openMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: "Delete conversation" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete conversation?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete conversation" }));
    expect(await within(dialog).findByRole("alert")).toBeInTheDocument();
    expect(daemon.onChanged).not.toHaveBeenCalled();
    expect(daemon.onDeleted).not.toHaveBeenCalled();
    expect(screen.queryByText("Conversation deleted")).toBeNull();
  });

  it.each(["chief", "mina"])("offers neither Delete nor Clear for %s's home conversation", async (owner) => {
    const user = userEvent.setup();
    setup({ agents: [agentSummary(owner, owner, { home_session_id: session.id })] });
    const menu = await openMenu(user);
    expect(within(menu).queryByRole("menuitem", { name: "Delete conversation" })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: "Clear" })).toBeNull();
  });

  it("does not offer deletion before the home-thread roster is known", async () => {
    const user = userEvent.setup();
    setup({ agents: null });
    expect(within(await openMenu(user)).queryByRole("menuitem", { name: "Delete conversation" })).toBeNull();
  });
});
