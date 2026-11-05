import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { ShellContext } from "../shell";
import { emptySessionView } from "../state/session";
import { fakeClient, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { ThreadView } from "./ThreadView";

vi.mock("../hooks/useSessionView", () => ({
  useSessionView: () => ({ view: { ...emptySessionView, status: "running" }, connection: "live", error: null }),
}));
vi.mock("../hooks/useSessionFiles", () => ({
  useSessionFiles: () => ({ artifacts: [], visuals: {} }),
}));

it("closes Stop everything as soon as it is confirmed, before the daemon answers", async () => {
  const user = userEvent.setup();
  const calls: Call[] = [];
  // Every request stays pending: the sheet must not wait on the stop to land.
  const daemon = fakeClient((call) => {
    calls.push(call);
    return new Promise<Response>(() => undefined);
  });
  render(
    <MemoryRouter initialEntries={["/sessions/s1"]}>
      <ClientContext.Provider value={daemon.client}>
        <ShellContext.Provider value={{ openDrawer: vi.fn(), refreshSessions: vi.fn(), sidebarCollapsed: false, toggleSidebar: vi.fn() }}>
          <ToastProvider>
            <ThreadView sessionId="s1" header={() => null} />
          </ToastProvider>
        </ShellContext.Provider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  await user.click(screen.getByRole("button", { name: "Stop" }));
  const dialog = await screen.findByRole("dialog", { name: "Stop everything?" });
  await user.click(within(dialog).getByRole("button", { name: "Stop everything" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(calls.some((call) => call.method === "POST" && call.path === "/v1/sessions/s1/stop")).toBe(true);
});
