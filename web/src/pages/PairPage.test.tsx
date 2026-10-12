import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PairPage } from "./PairPage";

const marker = document.createElement("meta");
marker.name = "openside-transport";
marker.content = "relay";

afterEach(() => {
  marker.remove();
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

describe("relay pairing fails closed", () => {
  it.each([
    "c=one-time-secret&t=relay&d=daemon",
    "c=one-time-secret&t=unknown&d=daemon&k=key",
    "c=one-time-secret&d=daemon&k=key",
    "%E0%A4%A",
  ])("never sends a malformed link to the direct API: %s", async (fragment) => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    window.history.replaceState(null, "", `/pair#${fragment}`);
    render(<PairPage onPaired={vi.fn()} />);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    await waitFor(() => expect(window.location.hash).toBe(""));
    expect(fetch).not.toHaveBeenCalled();
    expect(screen.queryByRole("link", { name: "enter a device token instead" })).toBeNull();
  });

  it("refuses a legacy direct code at the relay origin", async () => {
    document.head.append(marker);
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    window.history.replaceState(null, "", "/pair#one-time-secret");
    render(<PairPage onPaired={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("full pairing link");
    expect(fetch).not.toHaveBeenCalled();
    expect(screen.getByText("openside pair --via relay")).toBeInTheDocument();
  });

  it("refuses an invalid relay key without sending the code", async () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    window.history.replaceState(null, "", "/pair#c=one-time-secret&t=relay&d=daemon&k=invalid");
    render(<PairPage onPaired={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("invalid encryption key");
    expect(fetch).not.toHaveBeenCalled();
  });
});
