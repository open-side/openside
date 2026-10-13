import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteStatus } from "../api/types";
import { ClientContext } from "../context";
import { json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { ConnectPhone } from "./ConnectPhone";
import { offer, offStatus, pairingRequests, remoteDaemon, type RemoteDaemon, tailscaleOn } from "./testDaemon";


function open(daemon: RemoteDaemon, status: RemoteStatus = tailscaleOn(), onPaired = vi.fn(), onStatusStale = vi.fn()) {
  render(
    <MemoryRouter>
      <ClientContext.Provider value={daemon.client}>
        <ToastProvider>
          <ConnectPhone status={status} onStatusStale={onStatusStale} onPaired={onPaired} />
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return { onPaired, onStatusStale };
}

afterEach(() => {
  vi.useRealTimers();
  sessionStorage.clear();
});

describe("ConnectPhone gating", () => {
  it("explains where to do it when this browser may not pair, without offering a form", () => {
    open(remoteDaemon(), tailscaleOn({ pairing_allowed: false }));
    expect(screen.getByText(/use openside on the computer that runs it/)).toBeInTheDocument();
    expect(screen.getByText("openside-browser")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show QR code" })).toBeNull();
  });

  it("says the link expired when a stored secret is refused", () => {
    sessionStorage.setItem("openside.local-secret", "old");
    open(remoteDaemon(), tailscaleOn({ pairing_allowed: false }));
    expect(screen.getByText("Reopen openside from this computer")).toBeInTheDocument();
  });

  it("asks for a connection first when none is on", () => {
    open(remoteDaemon(), offStatus());
    expect(screen.getByText(/Turn on a connection first/)).toBeInTheDocument();
  });
});

describe("ConnectPhone flow", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  it("defaults to a phone's usual permissions, issues a person-device code through the connection, and draws the QR", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({ status: tailscaleOn() });
    open(daemon);

    expect(screen.getByLabelText("See conversations and results")).toBeChecked();
    expect(screen.getByLabelText("See conversations and results")).toBeDisabled();
    for (const label of ["Start tasks and send messages", "Approve or reject what the agent asks", "Do steps that need a person, such as logins", "Watch and use the agent’s browser"]) {
      expect(screen.getByLabelText(label)).toBeChecked();
    }
    expect(screen.getByLabelText("Change settings and manage other devices")).not.toBeChecked();

    await user.clear(screen.getByLabelText("Phone name"));
    await user.type(screen.getByLabelText("Phone name"), "Kim's iPhone");
    await user.click(screen.getByRole("button", { name: "Show QR code" }));

    expect(pairingRequests(daemon.calls)).toEqual([{
      name: "Kim's iPhone",
      scopes: ["read", "steer", "approve", "human_step", "browse"],
      kind: "person_device",
      via: "tailscale",
    }]);
    expect(await screen.findByRole("img", { name: "QR code to connect Kim's iPhone" })).toBeInTheDocument();
    expect(screen.getByText("K7QM2XPD")).toBeInTheDocument();
    expect(screen.getByRole("timer")).toHaveTextContent("3:00");
    expect(screen.getByText("Waiting for your phone…")).toBeInTheDocument();
  });

  it("leaves out unchecked permissions and asks for admin only when chosen", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({ status: tailscaleOn() });
    open(daemon);
    await user.click(screen.getByLabelText("Start tasks and send messages"));
    await user.click(screen.getByLabelText("Change settings and manage other devices"));
    await user.click(screen.getByRole("button", { name: "Show QR code" }));
    expect(pairingRequests(daemon.calls)[0]?.scopes).toEqual(["read", "approve", "human_step", "browse", "admin"]);
  });

  it("counts down, notices the new phone and offers another", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({ status: tailscaleOn() });
    const { onPaired } = open(daemon);
    await user.click(screen.getByRole("button", { name: "Show QR code" }));
    await screen.findByRole("timer");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(screen.getByRole("timer")).toHaveTextContent("2:55");
    // The laptop that was already paired is not the phone.
    expect(screen.queryByText(/is connected/)).toBeNull();

    daemon.state.devices = [
      ...daemon.state.devices,
      { device_id: "d2", name: "My phone", kind: "person_device", scopes: ["read", "approve"], created_at: "2026-10-13T00:00:00Z" },
    ];
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_100);
    });
    expect(await screen.findByText("My phone is connected")).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
    expect(onPaired).toHaveBeenCalledWith(expect.objectContaining({ device_id: "d2" }));
    expect(screen.getByRole("link", { name: "See paired devices" })).toHaveAttribute("href", "/devices");

    await user.click(screen.getByRole("button", { name: "Connect another phone" }));
    expect(screen.getByRole("button", { name: "Show QR code" })).toBeInTheDocument();
  });

  it("marks an expired code and issues a new one", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    let n = 0;
    const daemon = remoteDaemon({
      status: tailscaleOn(),
      offer: () => offer({ code: `CODE${++n}`, expires_at: new Date(Date.now() + 60_000).toISOString() }),
    });
    open(daemon);
    await user.click(screen.getByRole("button", { name: "Show QR code" }));
    expect(await screen.findByText("CODE1")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(61_000);
    });
    expect(screen.getByRole("timer")).toHaveTextContent("Expired");
    expect(screen.getByText("This code expired. Make a new one to continue.")).toBeInTheDocument();
    const polls = daemon.calls.filter((call) => call.path === "/v1/devices").length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6_000);
    });
    expect(daemon.calls.filter((call) => call.path === "/v1/devices").length).toBe(polls);

    await user.click(screen.getByRole("button", { name: "Make a new code" }));
    expect(await screen.findByText("CODE2")).toBeInTheDocument();
    expect(screen.getByRole("timer")).toHaveTextContent("1:00");
  });

  it("chooses between several connections", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const status = tailscaleOn({
      modes: ["tailscale", "cloudflare"],
      cloudflare: { state: "running", url: "https://x.trycloudflare.com", restarts: 0, last_error: null },
      configuration: { ...tailscaleOn().configuration, cloudflare: { enabled: true, tunnel: "quick", hostname: null, token_file: null } },
    });
    const daemon = remoteDaemon({ status });
    open(daemon, status);
    await user.click(screen.getByRole("radio", { name: "Cloudflare Tunnel" }));
    await user.click(screen.getByRole("button", { name: "Show QR code" }));
    expect(pairingRequests(daemon.calls)[0]?.via).toBe("cloudflare");
  });

  it("shows a pairing refusal and re-reads the status", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({
      status: tailscaleOn(),
      override: (call) => (call.path === "/v1/pairing" ? json(403, { code: "local_access_required", message: "no" }) : undefined),
    });
    const { onStatusStale } = open(daemon);
    await user.click(screen.getByRole("button", { name: "Show QR code" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Reopen openside using the desktop launcher");
    await waitFor(() => expect(onStatusStale).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: "Show QR code" })).toBeEnabled();
  });
});
