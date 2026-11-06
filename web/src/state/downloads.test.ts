import { describe, expect, it } from "vitest";
import type { Event } from "../api/types";
import { replaySession } from "./session";

type Payload = { kind: string } & Record<string, unknown>;
const AT = "2026-01-01T00:00:00Z";

function log(payloads: Payload[]): Event[] {
  return payloads.map((payload, index) => ({ session_id: "s1", seq: index + 1, ts: AT, run_id: null, ...payload })) as Event[];
}

const started = (id: string): Payload => ({ kind: "tool.updated", tool_call_id: id, update: { browser_event: "download.started", url: "https://files.example/report.pdf", tab_id: "t1" } });
const finished = (id: string, isError: boolean): Payload => ({ kind: "tool.ended", tool_call_id: id, card: null, result: { browser_event: "download.finished" }, is_error: isError });

describe("browser downloads on the timeline", () => {
  it("follows a download from start through saving to saved, after the work that triggered it", () => {
    const events = log([
      { kind: "tool.started", tool_call_id: "c1", name: "browser_pointer", args: { action: "click" } },
      started("G"),
      finished("G", false),
      { kind: "artifact.saved", path: "downloads/G/report.pdf", size: 2048, mime: "application/pdf", tool_call_id: "G", tool: "browser_download" },
      {
        kind: "tool.updated",
        tool_call_id: "G",
        update: { browser_event: "download.saved", artifact: "downloads/G/report.pdf", file_name: "report.pdf", size: 2048, mime: "application/pdf", quarantined: false },
      },
    ]);
    expect(replaySession(events.slice(0, 2)).items.at(-1)).toEqual({ type: "download", id: "G", url: "https://files.example/report.pdf", state: "in_progress" });
    expect(replaySession(events.slice(0, 3)).items.at(-1)).toMatchObject({ id: "G", state: "saving" });
    const view = replaySession(events);
    expect(view.items.map((item) => item.type)).toEqual(["tool", "download"]);
    expect(view.items[1]).toEqual({
      type: "download",
      id: "G",
      url: "https://files.example/report.pdf",
      state: "saved",
      artifact: "downloads/G/report.pdf",
      fileName: "report.pdf",
      size: 2048,
      mime: "application/pdf",
      quarantined: false,
    });
    expect(view.artifacts.map((artifact) => artifact.path)).toEqual(["downloads/G/report.pdf"]);
  });

  it("marks a canceled download failed, and a later failure reason replaces the default", () => {
    const events = log([started("G"), finished("G", true), { kind: "tool.updated", tool_call_id: "G", update: { browser_event: "download.failed", error: "too_large" } }]);
    expect(replaySession(events.slice(0, 2)).items).toMatchObject([{ id: "G", state: "failed", error: "canceled" }]);
    expect(replaySession(events).items).toMatchObject([{ id: "G", state: "failed", error: "too_large" }]);
  });

  it("ignores lifecycle events for a download it never saw start", () => {
    const view = replaySession(log([finished("X", false), { kind: "tool.updated", tool_call_id: "X", update: { browser_event: "download.failed", error: "unavailable" } }]));
    expect(view.items).toEqual([]);
  });

  it("leaves a real tool call's row alone even if its payload looks like a download event", () => {
    const view = replaySession(log([{ kind: "tool.started", tool_call_id: "c1", name: "browser_downloads", args: {} }, started("c1"), finished("c1", false)]));
    expect(view.items).toEqual([
      expect.objectContaining({ type: "tool", id: "c1", updates: [expect.objectContaining({ browser_event: "download.started" })], done: true, isError: false }),
    ]);
  });
});
