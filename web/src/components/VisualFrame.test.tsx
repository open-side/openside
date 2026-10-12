import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { fakeClient } from "../test/fakeDaemon";
import { SandboxFrame } from "./VisualFrame";

describe("SandboxFrame", () => {
  it("revokes blob URLs when the signed source changes and when unmounted", async () => {
    const daemon = fakeClient(() => new Response("<h1>Preview</h1>", { headers: { "Content-Type": "text/html" } }));
    const client = { ...daemon.client, relay: true };
    const calls = daemon.calls;
    const revoke = vi.spyOn(URL, "revokeObjectURL");
    try {
      const view = render(<ClientContext.Provider value={client}><SandboxFrame src="/first" title="Preview" /></ClientContext.Provider>);
      const frame = screen.getByTitle("Preview");
      await waitFor(() => expect(frame.getAttribute("src")).toMatch(/^blob:/));
      const first = frame.getAttribute("src");
      view.rerender(<ClientContext.Provider value={client}><SandboxFrame src="/second" title="Preview" /></ClientContext.Provider>);
      await waitFor(() => expect(frame.getAttribute("src")).not.toBe(first));
      await waitFor(() => expect(frame.getAttribute("src")).toMatch(/^blob:/));
      expect(revoke).toHaveBeenCalledWith(first);
      expect(calls.map((call) => call.path)).toEqual(["/first", "/second"]);
      expect(frame).toHaveAttribute("sandbox", "");
      expect(frame).toHaveAttribute("referrerpolicy", "no-referrer");
      const second = frame.getAttribute("src");
      view.unmount();
      expect(revoke).toHaveBeenCalledWith(second);
    } finally {
      revoke.mockRestore();
    }
  });
});
