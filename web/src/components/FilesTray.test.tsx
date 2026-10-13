import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Artifact } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient } from "../test/fakeDaemon";
import { FilesTray, TEXT_LIMIT } from "./FilesTray";

function artifact(path: string, mime: string, source: Artifact["source"], preview: boolean, extra: Partial<Artifact> = {}): Artifact {
  return {
    path,
    mime,
    size: 2048,
    source,
    purpose: "output",
    created_at: "2026-01-01T00:00:00Z",
    modified_at: "2026-01-01T00:00:00Z",
    preview_url: preview ? `/v1/sessions/s1/artifacts/${encodeURIComponent(path)}/preview?exp=1&sig=s` : null,
    ...extra,
  };
}

const LIST = [
  artifact("out/report.html", "text/html", "tool", true),
  artifact("data.bin", "application/octet-stream", "tool", false),
  artifact("transcript.jsonl", "application/jsonl", "transcript", false),
  // The agent's own browser observation, not something the user asked for.
  artifact("toolu_01.png", "image/png", "tool", false, { tool: "browser_screenshot", purpose: "working" }),
];

function setup(artifacts = LIST, body = "<h1>Report</h1>") {
  const daemon = fakeClient(() => new Response(body, { status: 200, headers: { "Content-Type": "text/html" } }));
  render(
    <ClientContext.Provider value={daemon.client}>
      <FilesTray sessionId="s1" artifacts={artifacts} />
    </ClientContext.Provider>,
  );
  return daemon;
}

afterEach(() => vi.restoreAllMocks());

describe("FilesTray (A-12)", () => {
  it("renders nothing when the session has no user-visible files", () => {
    setup([LIST[2]!, LIST[3]!]);
    expect(screen.queryByRole("button", { name: /files?$/ })).toBeNull();
  });

  it("shows an explicitly published screenshot but not a working one", () => {
    setup([LIST[3]!, artifact("screenshots/shot.png", "image/png", "tool", false, { tool: "browser_screenshot" })]);
    expect(screen.getByRole("button", { name: "1 file" })).toBeInTheDocument();
  });

  it("counts the files in one pill and lists them only once the sheet is opened", async () => {
    const user = userEvent.setup();
    setup();
    const pill = screen.getByRole("button", { name: "2 files" });
    expect(screen.queryByRole("dialog")).toBeNull();

    await user.click(pill);
    const list = within(screen.getByRole("dialog", { name: "Files" })).getByRole("list", { name: "Files" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows.map((row) => row.querySelector(".files-name")?.textContent)).toEqual(["report.html", "data.bin"]);
    expect(rows[0]).toHaveTextContent("2.0 KB · Created");
  });

  it("says 1 file in the singular", () => {
    setup([LIST[0]!]);
    expect(screen.getByRole("button", { name: "1 file" })).toBeInTheDocument();
  });

  it("opens a file as a sandboxed preview and goes back to the list", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(screen.getByRole("button", { name: "2 files" }));
    await user.click(screen.getByText("report.html"));

    const frame = screen.getByTitle("Preview of out/report.html");
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(frame.getAttribute("src")).toContain("/v1/sessions/s1/artifacts/out%2Freport.html/preview?exp=1&sig=s");
    expect(calls).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: "Source" }));
    await waitFor(() => expect(document.querySelector(".visual-source")?.textContent).toBe("<h1>Report</h1>"));
    expect(calls[0]).toMatchObject({ method: "GET", path: "/v1/sessions/s1/artifacts/out%2Freport.html" });
    expect(calls[0]?.headers.Authorization).toBe("Bearer test-token");
    expect(document.querySelector("iframe")).toBeNull();
    expect(screen.queryByRole("note")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Back to files" }));
    expect(screen.getByRole("list", { name: "Files" })).toBeInTheDocument();
  });

  it("downloads straight from a row through the authenticated route without opening it", async () => {
    const user = userEvent.setup();
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      expect(this.download).toBe("data.bin");
    });
    const { calls } = setup();
    await user.click(screen.getByRole("button", { name: "2 files" }));
    await user.click(screen.getByRole("button", { name: "Download data.bin" }));

    await waitFor(() => expect(click).toHaveBeenCalledTimes(1));
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ method: "GET", path: "/v1/sessions/s1/artifacts/data.bin" });
    expect(calls[0]?.headers.Authorization).toBe("Bearer test-token");
    expect(screen.getByRole("list", { name: "Files" })).toBeInTheDocument();
  });

  it("says when a file has no preview", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: "2 files" }));
    await user.click(screen.getByText("data.bin"));
    expect(screen.getByText("No preview for this file type.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Download" })).toBeInTheDocument();
  });

  it("marks a source view cut at the limit and keeps the full size visible", async () => {
    const user = userEvent.setup();
    setup(LIST, `${"x".repeat(TEXT_LIMIT)}tail`);
    await user.click(screen.getByRole("button", { name: "2 files" }));
    await user.click(screen.getByText("report.html"));
    await user.click(screen.getByRole("button", { name: "Source" }));
    await waitFor(() => expect(document.querySelector(".visual-source")?.textContent).toHaveLength(TEXT_LIMIT));
    expect(screen.getByRole("note")).toHaveTextContent("Showing the first 200,000 of 200,004 bytes.");
  });
});
