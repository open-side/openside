import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import type { Event } from "../api/types";
import { ClientContext } from "../context";
import { replaySession } from "../state/session";
import { fakeClient } from "../test/fakeDaemon";
import { Timeline } from "./Timeline";

it("replays old and new observation tool rows without inline images or broken image requests", () => {
  const payloads = [
    { kind: "tool.started", tool_call_id: "toolu_old", name: "browser_screenshot", args: { tab_id: "t1" } },
    { kind: "artifact.saved", path: "toolu_old.png", size: 42, mime: "image/png", tool_call_id: "toolu_old", tool: "browser_screenshot" },
    { kind: "tool.ended", tool_call_id: "toolu_old", result: { path: "/old/artifacts/s1/toolu_old.png" }, is_error: false },
    { kind: "tool.started", tool_call_id: "working", name: "browser_screenshot", args: { tab_id: "t1" } },
    { kind: "tool.ended", tool_call_id: "working", result: { saved: false, purpose: "working", width: 1440, height: 900 }, is_error: false },
  ];
  const events = payloads.map((payload, index) => ({ session_id: "s1", seq: index + 1, ts: "2026-01-01T00:00:00Z", run_id: null, ...payload })) as Event[];
  const daemon = fakeClient(() => { throw new Error("Working images must not be fetched"); });
  render(<ClientContext.Provider value={daemon.client}><Timeline sessionId="s1" view={replaySession(events)} /></ClientContext.Provider>);
  expect(document.querySelector('[data-tool="toolu_old"]')).not.toBeNull();
  expect(document.querySelector('[data-tool="working"]')).not.toBeNull();
  expect(document.querySelector('[data-tool="toolu_old"] pre:last-child')).toHaveTextContent("/old/artifacts/s1/toolu_old.png");
  expect(document.querySelector('[data-tool="working"] pre:last-child')).toHaveTextContent('"purpose": "working"');
  expect(document.querySelector('[data-tool="working"] pre:last-child')).not.toHaveTextContent("path");
  expect(screen.queryByRole("img", { hidden: true })).toBeNull();
  expect(daemon.calls).toHaveLength(0);
});
