import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useSessionFiles } from "../hooks/useSessionFiles";
import { replaySession } from "../state/session";
import { fakeClient } from "../test/fakeDaemon";
import { isOutputArtifact } from "./artifacts";
import type { Event } from "./types";

function events(payloads: Record<string, unknown>[]): Event[] {
  return payloads.map((payload, index) => ({ session_id: "s1", seq: index + 1, ts: "2026-01-01T00:00:00Z", run_id: null, ...payload })) as Event[];
}

describe("artifact delivery classification", () => {
  it("hides working captures and infers legacy screenshot provenance without hiding files or attachments", () => {
    for (const artifact of [
      { path: "screenshots/current.png", purpose: "working" as const },
      { path: "toolu_old.png" },
      { path: "old-cli.png", tool: "browser_screenshot" },
    ]) expect(isOutputArtifact(artifact)).toBe(false);
    for (const artifact of [
      { path: "toolu_shared.png", purpose: "output" as const },
      { path: "screenshots/shared.png", tool: "browser_screenshot" },
      { path: "files/toolu_chart.png", tool: "write" },
      { path: "attachments/m1/toolu_upload.png" },
      { path: "chart.png", tool: "write" },
    ]) expect(isOutputArtifact(artifact)).toBe(true);
  });

  it("replays legacy observations without losing their tool rows and retains explicit deliverables", () => {
    const view = replaySession(events([
      { kind: "tool.started", tool_call_id: "toolu_old", name: "browser_screenshot", args: { tab_id: "t1" } },
      { kind: "artifact.saved", path: "toolu_old.png", size: 42, mime: "image/png", tool_call_id: "toolu_old", tool: "browser_screenshot" },
      { kind: "tool.ended", tool_call_id: "toolu_old", result: { path: "/old/artifacts/s1/toolu_old.png" }, is_error: false },
      { kind: "tool.started", tool_call_id: "working", name: "browser_screenshot", args: { tab_id: "t1" } },
      { kind: "tool.ended", tool_call_id: "working", result: { saved: false, purpose: "working", width: 1440, height: 900 }, is_error: false },
      { kind: "artifact.saved", path: "screenshots/shared.png", size: 42, mime: "image/png", tool_call_id: "shared", tool: "browser_screenshot" },
    ]));
    expect(view.artifacts.map((artifact) => artifact.path)).toEqual(["screenshots/shared.png"]);
    expect(view.items.filter((item) => item.type === "tool")).toMatchObject([
      { id: "toolu_old", done: true, isError: false },
      { id: "working", done: true, isError: false },
    ]);
  });

  it("keeps working screenshots out of the Files data even with a mixed old daemon listing", async () => {
    const provenance = [
      { path: "toolu_old.png", tool: "browser_screenshot" },
      { path: "working.png", tool: "browser_screenshot", purpose: "working" },
      { path: "screenshots/shared.png", tool: "browser_screenshot", purpose: "output" },
    ];
    const { client } = fakeClient((call) => new Response(JSON.stringify(call.path.endsWith("/visuals") ? { visuals: [] } : {
      artifacts: provenance.map((artifact) => ({ ...artifact, source: "tool", mime: "image/png", size: 42, created_at: "2026-01-01T00:00:00Z", modified_at: "2026-01-01T00:00:00Z" })),
    }), { status: 200, headers: { "Content-Type": "application/json" } }));
    const { result, unmount } = renderHook(() => useSessionFiles(client, "s1", replaySession([])));
    await waitFor(() => expect(result.current.artifacts.map((artifact) => artifact.path)).toEqual(["screenshots/shared.png"]));
    unmount();
  });
});
