import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Event } from "../api/types";
import { ClientContext } from "../context";
import { replaySession } from "../state/session";
import { fakeClient } from "../test/fakeDaemon";
import { Timeline } from "./Timeline";

const NOTES = "# Trip notes\n\nPack light.\n\n- passport\n- charger";
const PLAN = "## Plan\n\nDay one: museums.\n\nDay two: hike.";

function mount() {
  const events = [
    { kind: "turn.started" },
    {
      kind: "assistant.final",
      text: "Here are both.",
      attachments: [
        { path: "documents/delivered/a.md", title: "Trip notes", display: "inline", bytes: NOTES.length, content_type: "text/markdown" },
        { path: "documents/delivered/b.md", title: "Itinerary", display: "panel", bytes: PLAN.length, content_type: "text/markdown" },
      ],
    },
  ].map((p, i) => ({ session_id: "s1", seq: i + 1, ts: "2026-01-01T00:00:00Z", run_id: null, ...p })) as Event[];
  const daemon = fakeClient((call) => new Response(call.path.endsWith("a.md") ? NOTES : PLAN, { status: 200, headers: { "content-type": "text/markdown" } }));
  render(
    <ClientContext.Provider value={daemon.client}>
      <Timeline view={replaySession(events)} sessionId="s1" />
    </ClientContext.Provider>,
  );
  return daemon;
}

describe("documents attached to a message", () => {
  it("shows them at the top of the agent's bubble: inline ones read in place, panel ones as a card", async () => {
    const daemon = mount();
    const bubble = document.querySelector<HTMLElement>(".bubble.assistant")!;
    const inline = within(bubble).getByRole("region", { name: "Trip notes" });
    // Before its text arrives the document is a placeholder, never an empty box.
    expect(within(inline).getByRole("status", { name: "Loading document" })).toBeInTheDocument();
    expect(await within(inline).findByRole("heading", { name: "Trip notes" })).toBeInTheDocument();
    expect(within(inline).queryByRole("status")).toBeNull();
    expect(within(inline).getByText("passport")).toBeInTheDocument();
    expect(bubble.firstElementChild).toHaveClass("message-documents");
    expect(within(bubble).getByText("Here are both.")).toBeInTheDocument();
    const card = within(bubble).getByRole("button", { name: /Itinerary/ });
    await waitFor(() => expect(card).toHaveTextContent("Day one: museums."));
    expect(daemon.calls.map((call) => call.path)).toEqual(
      expect.arrayContaining(["/v1/sessions/s1/artifacts/documents%2Fdelivered%2Fa.md", "/v1/sessions/s1/artifacts/documents%2Fdelivered%2Fb.md"]),
    );
  });

  it("opens a panel document with Copy and Close", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(screen.getByRole("button", { name: /Itinerary/ }));
    const panel = await screen.findByRole("dialog", { name: "Itinerary" });
    expect(within(panel).getByRole("heading", { name: "Plan" })).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "Copy" })).toBeEnabled();
    await user.click(within(panel).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Itinerary" })).toBeNull());
  });
});
