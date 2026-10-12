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
