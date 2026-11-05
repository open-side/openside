import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Action } from "../api/types";
import { fixture } from "../cards/testing";
import { ClientContext } from "../context";
import { fakeClient, json } from "../test/fakeDaemon";
import { CardSheet } from "./CardSheet";
import { CardHostContext, ConversationCard, type CardHost } from "./ConversationCard";

const haptic = vi.hoisted(() => vi.fn());
vi.mock("../pwa/haptics", () => ({ haptic }));

beforeEach(() => haptic.mockClear());

function mount(ui: React.ReactElement, handler = () => json(200, {})) {
  const daemon = fakeClient(handler);
  const view = render(<ClientContext.Provider value={daemon.client}>{ui}</ClientContext.Provider>);
  return { ...view, daemon };
}

const follow: Action = { id: "ask", label: "Compare these", style: "secondary", target: { type: "follow_up", text: "Compare these" } };
const posts = (calls: { method: string; path: string }[]) => calls.filter((call) => call.method === "POST");

describe("ConversationCard", () => {
  it("answers through the client, buzzes once and reports the outcome", async () => {
    const user = userEvent.setup();
    const onSettled = vi.fn();
    const { daemon } = mount(<ConversationCard card={fixture("ticket:confirm1")} defaultOpen onSettled={onSettled} />);
    await user.click(screen.getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith("resolved", expect.objectContaining({ id: "ticket:confirm1" })));
    expect(posts(daemon.calls)).toEqual([expect.objectContaining({ path: "/v1/tickets/confirm1/resolve", body: { value: true } })]);
    expect(haptic).toHaveBeenCalledTimes(1);
  });

  it("keeps a half-typed value across an update of the same card and starts over for another card", async () => {
    const card = fixture("ticket:otp1");
    const { rerender, daemon } = mount(<ConversationCard card={card} defaultOpen />);
    const input = screen.getByLabelText("Code");
    await userEvent.setup().type(input, "48");
    rerender(
      <ClientContext.Provider value={daemon.client}>
        <ConversationCard card={{ ...card, subtitle: "Still waiting" }} defaultOpen />
      </ClientContext.Provider>,
    );
    expect(screen.getByLabelText("Code")).toBe(input);
    expect(input).toHaveValue("48");
    rerender(
      <ClientContext.Provider value={daemon.client}>
        <ConversationCard card={{ ...card, id: "ticket:otp2", ticket_id: "otp2" }} defaultOpen />
      </ClientContext.Provider>,
    );
    expect(screen.getByLabelText("Code")).toHaveValue("");
  });

  it("routes a follow-up to the screen's composer, the card's own handler first", async () => {
    const user = userEvent.setup();
    const card = { ...fixture("present:p1"), actions: [follow] };
    const followUp = vi.fn();
    const host: CardHost = { followUp };
    const { unmount } = mount(
      <CardHostContext.Provider value={host}>
        <ConversationCard card={card} />
      </CardHostContext.Provider>,
    );
    await user.click(screen.getByRole("button", { name: "Compare these" }));
    expect(followUp).toHaveBeenCalledWith("Compare these");
    unmount();

    const own = vi.fn(async () => undefined);
    const second = vi.fn();
    mount(
      <CardHostContext.Provider value={{ followUp: second }}>
        <ConversationCard card={card} onAction={own} />
      </CardHostContext.Provider>,
    );
    await user.click(screen.getByRole("button", { name: "Compare these" }));
    expect(own).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();
  });

  it("offers no follow-up when nothing on screen can take it", () => {
    mount(<ConversationCard card={{ ...fixture("present:p1"), actions: [follow] }} />);
    expect(screen.queryByRole("button", { name: "Compare these" })).toBeNull();
  });

  it("an inert card stays visible and cannot be pressed", () => {
    const { container } = mount(<ConversationCard card={fixture("ticket:confirm1")} defaultOpen inert />);
    const root = container.querySelector(".oc-root")!;
    expect(root).toHaveAttribute("inert");
    expect(root).toHaveAttribute("aria-hidden", "true");
  });
});

describe("CardSheet", () => {
  it("hosts the same card always open, closes on Close, on Escape and once the request is answered", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    mount(<CardSheet card={fixture("ticket:otp1")} sessionId="s1" onClose={onClose} />);
    const sheet = screen.getByRole("dialog", { name: "Verification code" });
    expect(sheet).toHaveClass("card-sheet");
    expect(sheet.querySelector(".oc-card")).toHaveAttribute("data-open", "true");
    expect(sheet.querySelector("button.oc-close")).toBeNull();
    await waitFor(() => expect(within(sheet).getByLabelText("Code")).toHaveFocus());

    await user.click(within(sheet).getByRole("button", { name: "Close" }));
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(2);

    onClose.mockClear();
    await user.type(within(sheet).getByLabelText("Code"), "123456");
    await act(async () => {
      await user.click(within(sheet).getByRole("button", { name: "Submit code" }));
    });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("is modal: focus enters, Tab stays inside, the page behind is unreachable, and focus returns on close", async () => {
    const user = userEvent.setup();
    function Host() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>
            Open sheet
          </button>
          <button type="button">Behind</button>
          {open ? <CardSheet card={fixture("ticket:otp1")} sessionId="s1" onClose={() => setOpen(false)} /> : null}
        </>
      );
    }
    mount(<Host />);
    const opener = screen.getByRole("button", { name: "Open sheet" });
    await user.click(opener);
    const sheet = await screen.findByRole("dialog", { name: "Verification code" });
    await waitFor(() => expect(within(sheet).getByLabelText("Code")).toHaveFocus());

    const behind = screen.getByText("Behind");
    expect(behind.closest("[aria-hidden=true], [inert]")).not.toBeNull();
    // Base UI's focus guards hand focus back on the next tick, so each step waits for it to settle inside.
    for (let step = 0; step < 8; step += 1) {
      await user.tab();
      await waitFor(() => expect(sheet.contains(document.activeElement)).toBe(true));
    }
    for (let step = 0; step < 8; step += 1) {
      await user.tab({ shift: true });
      await waitFor(() => expect(sheet.contains(document.activeElement)).toBe(true));
    }

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(opener).toHaveFocus());
    expect(behind.closest("[aria-hidden=true], [inert]")).toBeNull();
  });
});
