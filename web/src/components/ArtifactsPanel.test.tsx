import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Artifact } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient } from "../test/fakeDaemon";
import { ArtifactsPanel, TEXT_LIMIT } from "./ArtifactsPanel";

function artifact(path: string, mime: string, source: Artifact["source"], preview: boolean): Artifact {
  return {
    path,
    mime,
    size: 2048,
    source,
    created_at: "2026-01-01T00:00:00Z",
    modified_at: "2026-01-01T00:00:00Z",
    preview_url: preview ? `/v1/sessions/s1/artifacts/${encodeURIComponent(path)}/preview?exp=1&sig=s` : null,
  };
}

const LIST = [
  artifact("report.html", "text/html", "tool", true),
  artifact("data.bin", "application/octet-stream", "tool", false),
  artifact("transcript.jsonl", "application/jsonl", "transcript", false),
];

function setup(body = "<h1>Report</h1>") {
  const daemon = fakeClient(() => new Response(body, { status: 200, headers: { "Content-Type": "text/html" } }));
  render(
    <ClientContext.Provider value={daemon.client}>
      <ArtifactsPanel sessionId="s1" artifacts={LIST} />
    </ClientContext.Provider>,
  );
  return daemon;
}

describe("ArtifactsPanel (A-12)", () => {
  it("lists the session's files and previews the selected one in a sandboxed iframe", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const rows = screen.getAllByRole("button", { pressed: false });
    expect(rows.map((row) => row.querySelector(".artifact-path")?.textContent)).toEqual(["report.html", "data.bin"]);
    expect(rows[0]).toHaveTextContent("tool · 2.0 KB");

    await user.click(rows[0]!);
    const frame = screen.getByTitle("Preview of report.html");
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(frame.getAttribute("src")).toContain("/v1/sessions/s1/artifacts/report.html/preview?exp=1&sig=s");
    expect(calls).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: "Source" }));
    await waitFor(() => expect(document.querySelector(".visual-source")?.textContent).toBe("<h1>Report</h1>"));
    expect(calls[0]).toMatchObject({ method: "GET", path: "/v1/sessions/s1/artifacts/report.html" });
    expect(calls[0]?.headers.Authorization).toBe("Bearer test-token");
    expect(document.querySelector("iframe")).toBeNull();
    expect(screen.queryByRole("note")).toBeNull();
  });

  it("says when a file has no preview", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByText("data.bin"));
    expect(screen.getByText("No preview for this file type.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Download" })).toBeInTheDocument();
  });

  it("marks a source view cut at the limit and keeps the full size visible", async () => {
    const user = userEvent.setup();
    setup(`${"x".repeat(TEXT_LIMIT)}tail`);
    await user.click(screen.getByText("report.html"));
    await user.click(screen.getByRole("button", { name: "Source" }));
    await waitFor(() => expect(document.querySelector(".visual-source")?.textContent).toHaveLength(TEXT_LIMIT));
    expect(screen.getByRole("note")).toHaveTextContent("Showing the first 200,000 of 200,004 bytes.");
  });
});
