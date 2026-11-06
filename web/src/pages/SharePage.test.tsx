import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SharePage } from "./SharePage";

afterEach(() => {
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

describe("share page", () => {
  it.each(["%E0%A4%A", "%", "", "too-short"])("shows an incomplete link without calling the daemon: %s", async (fragment) => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    window.history.replaceState(null, "", `/share#${fragment}`);
    render(<SharePage />);
    expect(await screen.findByText("This link is incomplete")).toBeInTheDocument();
    expect(fetch).not.toHaveBeenCalled();
  });
});
