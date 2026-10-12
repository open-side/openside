import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { json } from "./test/fakeDaemon";

afterEach(() => {
  vi.unstubAllGlobals();
  window.history.pushState({}, "", "/");
});

describe("App", () => {
  it("drops a token the live event stream rejects and returns to token entry", async () => {
    localStorage.setItem("openside.token", "revoked-token");
    window.history.pushState({}, "", "/sessions/s1");
    const requested: string[] = [];
    // Only the SSE stream rejects the token, so the reset must come from that path.
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const path = new URL(String(input), "http://daemon.test").pathname;
        requested.push(path);
        return path.endsWith("/events")
          ? json(401, { code: "unauthorized", message: "unauthorized" })
          : json(200, { status: "ok" });
      }),
    );
    render(<App />);
    expect(await screen.findByLabelText("Device token")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("The saved token was rejected");
    expect(localStorage.getItem("openside.token")).toBeNull();
    expect(requested).toContain("/v1/sessions/s1/events");
  });
});

describe("saved relay connection", () => {
  it.each(["{broken", "null", JSON.stringify({ kind: "relay" }), JSON.stringify({
    kind: "relay", relay: "http://localhost:3000", daemon_id: "daemon", daemon_key: "invalid",
  })])("does not leak the token to a direct API for invalid metadata: %s", (transport) => {
    localStorage.setItem("openside.token", "must-not-leak");
    localStorage.setItem("openside.transport", transport);
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    render(<App />);
    expect(screen.getByRole("alert")).toHaveTextContent("saved relay connection is incomplete");
    expect(screen.queryByLabelText("Device token")).toBeNull();
    expect(screen.getByText("openside pair --via relay")).toBeInTheDocument();
    expect(fetch).not.toHaveBeenCalled();
  });

  it("requires pairing on a relay origin even if a bare token was saved", () => {
    const marker = document.createElement("meta");
    marker.name = "openside-transport";
    marker.content = "relay";
    document.head.append(marker);
    try {
      localStorage.setItem("openside.token", "must-not-leak");
      const fetch = vi.fn();
      vi.stubGlobal("fetch", fetch);
      render(<App />);
      expect(screen.queryByLabelText("Device token")).toBeNull();
      expect(screen.getByText("openside pair --via relay")).toBeInTheDocument();
      expect(fetch).not.toHaveBeenCalled();
    } finally {
      marker.remove();
    }
  });
});
