import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { ToastProvider } from "../ui";
import { SessionAccountDialog } from "./SessionAccountDialog";
import { keyInfo, modelsDaemon } from "./testDaemon";

afterEach(cleanup);

const ref = (id: string) => ({ source: "vault", account_id: id });

function open(daemon: ReturnType<typeof modelsDaemon>, onClose = vi.fn()) {
  render(
    <ClientContext.Provider value={daemon.client}>
      <ToastProvider>
        <SessionAccountDialog open sessionId="s1" onClose={onClose} />
      </ToastProvider>
    </ClientContext.Provider>,
  );
  return onClose;
}

describe("SessionAccountDialog", () => {
  it("lists only accounts that are in a rotation and prefers one for the next requests", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({
      keys: [keyInfo("anthropic", "k1", "Work"), keyInfo("anthropic", "k2", "Spare")],
      pools: { anthropic: [ref("k1")] },
    });
    const onClose = open(daemon);
    const group = await screen.findByRole("region", { name: "Anthropic" });
    expect(within(group).getByText("Work")).toBeInTheDocument();
    expect(within(group).queryByText("Spare")).toBeNull();
    await user.click(within(group).getByRole("button", { name: "Prefer Work" }));
    await waitFor(() => expect(daemon.state.sessionAccounts.s1).toEqual(ref("k1")));
    expect(daemon.calls).toContainEqual(expect.objectContaining({ method: "PUT", path: "/v1/sessions/s1/account", body: { account: ref("k1") } }));
    expect(onClose).toHaveBeenCalled();
  });

  it("returns to automatic selection", async () => {
    const user = userEvent.setup();
    const daemon = modelsDaemon({ keys: [keyInfo("openai", "k1", "Work")], pools: { openai: [ref("k1")] }, sessionAccounts: { s1: ref("k1") } });
    open(daemon);
    await user.click(await screen.findByRole("button", { name: "Pick automatically" }));
    await waitFor(() => expect(daemon.state.sessionAccounts.s1).toBeNull());
  });
});
