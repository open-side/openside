import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import type { TimelineItem } from "../state/session";
import { fakeClient } from "../test/fakeDaemon";
import { DownloadChip } from "./DownloadChip";

type DownloadItem = Extract<TimelineItem, { type: "download" }>;

const SAVED: DownloadItem = {
  type: "download",
  id: "G",
  url: "https://files.example/report.pdf",
  state: "saved",
  artifact: "downloads/G/report.pdf",
  fileName: "report.pdf",
  size: 2048,
  mime: "application/pdf",
  quarantined: false,
};

function setup(item: DownloadItem) {
  const daemon = fakeClient(() => new Response("%PDF", { status: 200, headers: { "Content-Type": "application/pdf" } }));
  render(
    <ClientContext.Provider value={daemon.client}>
      <DownloadChip sessionId="s1" item={item} />
    </ClientContext.Provider>,
  );
  return daemon;
}

afterEach(() => vi.restoreAllMocks());

describe("DownloadChip", () => {
  it("says where a running download comes from, then that it is being saved", () => {
    const { client } = fakeClient(() => new Response(null, { status: 404 }));
    const chip = (state: DownloadItem["state"]) => (
      <ClientContext.Provider value={client}>
        <DownloadChip sessionId="s1" item={{ type: "download", id: "G", url: "https://files.example/report.pdf", state }} />
      </ClientContext.Provider>
    );
    const { rerender } = render(chip("in_progress"));
    expect(screen.getByText("Downloading from files.example")).toBeInTheDocument();
    rerender(chip("saving"));
    expect(screen.getByText("Saving to Files")).toBeInTheDocument();
  });

  it("falls back to a generic reason for an error code it doesn't know", () => {
    setup({ type: "download", id: "G", url: "", state: "failed", error: "toString" });
    expect(screen.getByText("Couldn’t save this download")).toBeInTheDocument();
  });

  it("downloads a saved file through the authenticated route, its path as one encoded segment", async () => {
    const user = userEvent.setup();
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      expect(this.download).toBe("report.pdf");
    });
    const { calls } = setup(SAVED);
    expect(screen.getByText("report.pdf")).toBeInTheDocument();
    expect(screen.getByText("2.0 KB · files.example")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Download report.pdf" }));

    await waitFor(() => expect(click).toHaveBeenCalledTimes(1));
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ method: "GET", path: "/v1/sessions/s1/artifacts/downloads%2FG%2Freport.pdf" });
    expect(calls[0]?.headers.Authorization).toBe("Bearer test-token");
  });

  it("offers a quarantined file for download only, without fetching or previewing it", () => {
    const { calls } = setup({ ...SAVED, artifact: "downloads/G/setup.exe", fileName: "setup.exe", mime: "application/octet-stream", quarantined: true });
    expect(screen.getByText("Download only — this file type isn’t opened here")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Download setup.exe" })).toBeInTheDocument();
    expect(document.querySelector("iframe, img, embed, object")).toBeNull();
    expect(calls).toHaveLength(0);
  });

  it("names the limit a failed download ran into", () => {
    setup({ type: "download", id: "G", url: "", state: "failed", error: "session_quota" });
    expect(screen.getByText("Not saved — this chat has reached its 200 MB limit for downloads")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
