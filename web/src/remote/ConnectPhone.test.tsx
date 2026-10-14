import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteStatus } from "../api/types";
import { ClientContext } from "../context";
import { json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { ConnectPhone } from "./ConnectPhone";
import { offer, offStatus, pairingRequests, quickTunnel, remoteDaemon, type RemoteDaemon, tailscaleOn } from "./testDaemon";

function tree(daemon: RemoteDaemon, status: RemoteStatus, onPaired: () => void, onStatusStale: () => void) {
  return (
    <MemoryRouter>
      <ClientContext.Provider value={daemon.client}>
        <ToastProvider>
          <ConnectPhone status={status} onStatusStale={onStatusStale} onPaired={onPaired} />
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>
  );
}

function open(daemon: RemoteDaemon, status: RemoteStatus = tailscaleOn(), onPaired = vi.fn(), onStatusStale = vi.fn()) {
  const view = render(tree(daemon, status, onPaired, onStatusStale));
  /** The status poll of the screen around it delivers a new status object. */
  const update = (next: RemoteStatus) => view.rerender(tree(daemon, next, onPaired, onStatusStale));
  return { onPaired, onStatusStale, update };
}

const tick = (ms: number) =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });

afterEach(() => {
  vi.useRealTimers();
  sessionStorage.clear();
});

describe("ConnectPhone gating", () => {
  it("explains where to do it when this browser may not pair, without issuing a code", () => {
    const daemon = remoteDaemon();
    open(daemon, tailscaleOn({ pairing_allowed: false }));
    expect(screen.getByText(/use openside on the computer that runs it/)).toBeInTheDocument();
    expect(screen.getByText("openside-browser")).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
    expect(pairingRequests(daemon.calls)).toEqual([]);
  });

  it("says the link expired when a stored secret is refused", () => {
    sessionStorage.setItem("openside.local-secret", "old");
    open(remoteDaemon(), tailscaleOn({ pairing_allowed: false }));
    expect(screen.getByText("Reopen openside from this computer")).toBeInTheDocument();
  });
});

describe("ConnectPhone without a connection", () => {
  it("offers Tailscale in one click where it is installed, and issues no code", async () => {
    const user = userEvent.setup();
    const daemon = remoteDaemon({
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/tailscale" ? json(200, tailscaleOn()) : undefined),
    });
    const { onStatusStale } = open(daemon, offStatus());
    expect(screen.getByText("Your phone needs a way to reach this computer.")).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Turn on Tailscale" }));
    await waitFor(() => expect(onStatusStale).toHaveBeenCalled());
    expect(daemon.calls.find((call) => call.method === "PUT")?.body).toEqual({ enabled: true });
    expect(pairingRequests(daemon.calls)).toEqual([]);
  });

  it("falls back to a Cloudflare quick tunnel, which needs no account, when Tailscale is missing", async () => {
    const user = userEvent.setup();
    const daemon = remoteDaemon({
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/cloudflare" ? json(200, offStatus()) : undefined),
    });
    open(daemon, offStatus({ tailscale: { installed: false, state: "unavailable", url: null, last_error: null } }));
    expect(screen.getByText(/needs no account/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Turn on Cloudflare Tunnel" }));
    await waitFor(() => expect(daemon.calls.some((call) => call.method === "PUT")).toBe(true));
    expect(daemon.calls.find((call) => call.method === "PUT")?.body).toEqual({ enabled: true, tunnel: "quick", hostname: null, token_file: null });
  });

  it("offers nothing to click away from the computer's settings", () => {
    open(remoteDaemon(), offStatus({ configure_allowed: false }));
    expect(screen.getByText("Your phone needs a way to reach this computer.")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("shows a tunnel that is still starting instead of a code that would lead nowhere", () => {
    const daemon = remoteDaemon();
    open(daemon, quickTunnel({ state: "starting" }));
    expect(screen.getByRole("status")).toHaveTextContent("Starting Cloudflare Tunnel… The QR code appears as soon as it is ready.");
    expect(screen.queryByRole("img")).toBeNull();
    expect(pairingRequests(daemon.calls)).toEqual([]);
  });

  it("says the tunnel isn't reachable yet and retries while cloudflared restarts", () => {
    open(remoteDaemon(), quickTunnel({ state: "restarting", restarts: 2 }));
    expect(screen.getByRole("status")).toHaveTextContent("The tunnel isn't reachable yet. Retrying…");
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("shows a failed connection's reason with a way to try again", async () => {
    const user = userEvent.setup();
    const daemon = remoteDaemon({
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/cloudflare" ? json(200, quickTunnel({ state: "starting" })) : undefined),
    });
    const { onStatusStale } = open(daemon, quickTunnel({ state: "failed", restarts: 3, last_error: "cloudflared: executable not found" }));
    expect(screen.getByRole("alert")).toHaveTextContent(/executable not found.*installed/);
    expect(screen.queryByRole("img")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(onStatusStale).toHaveBeenCalled());
  });
});

describe("ConnectPhone with a connection", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  it("issues a person-device code through the best connection on its own and shows the QR with its status line and link", async () => {
    const daemon = remoteDaemon({ status: tailscaleOn() });
    open(daemon);

    expect(await screen.findByRole("img", { name: "QR code to connect My phone" })).toBeInTheDocument();
    expect(pairingRequests(daemon.calls)).toEqual([{
      name: "My phone",
      scopes: ["read", "steer", "approve", "human_step", "browse"],
      kind: "person_device",
      via: "tailscale",
    }]);
    expect(screen.getByText("Waiting for your phone…")).toBeInTheDocument();
    expect(screen.getByRole("timer")).toHaveTextContent("3:00");
    expect(screen.getByText("https://desk.tail1234.ts.net/pair#K7QM2XPD")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy the pairing link" })).toBeInTheDocument();
    expect(screen.getByText("Tailscale")).toBeInTheDocument();
    // The choice and the permissions are tucked away, not a form in front of the code.
    expect(screen.queryByLabelText("Phone name")).toBeNull();
    expect(screen.queryByRole("button", { name: "Change connection" })).toBeNull();
  });

  it("issues exactly one code while the screen keeps handing in fresh status objects", async () => {
    const daemon = remoteDaemon({ status: tailscaleOn() });
    const { update } = open(daemon);
    await screen.findByRole("img");
    update(tailscaleOn());
    update(tailscaleOn());
    await tick(5_000);
    expect(pairingRequests(daemon.calls)).toHaveLength(1);
  });

  it("names the relay in lower case inside the sentence that says which connection is used", async () => {
    const base = offStatus();
    const status = offStatus({
      configuration: { ...base.configuration, relay: { enabled: true, url: "https://relay.example.com", token_file: null } },
      relay: { state: "connected", url: "https://relay.example.com", daemon_id: "id", daemon_key: "k", clients: 0, last_error: null, e2ee: true },
    });
    open(remoteDaemon({ status }), status);
    await screen.findByRole("img");
    expect(screen.getByText(/^Through/)).toHaveTextContent("Through your own relay");
  });

  it("starts with the connection the daemon prefers and switches under Change connection", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const status = tailscaleOn({
      modes: ["cloudflare", "tailscale"],
      cloudflare: { state: "running", url: "https://x.trycloudflare.com", restarts: 0, last_error: null },
      configuration: { ...tailscaleOn().configuration, cloudflare: { enabled: true, tunnel: "quick", hostname: null, token_file: null } },
    });
    const daemon = remoteDaemon({ status });
    open(daemon, status);
    await screen.findByRole("img");
    expect(pairingRequests(daemon.calls).map((request) => request.via)).toEqual(["cloudflare"]);

    await user.click(screen.getByRole("button", { name: "Change connection" }));
    await user.click(screen.getByRole("radio", { name: "Tailscale" }));
    await waitFor(() => expect(pairingRequests(daemon.calls).map((request) => request.via)).toEqual(["cloudflare", "tailscale"]));
    expect(screen.queryByRole("radio")).toBeNull();
  });

  it("counts down, notices the new phone, says who connected and offers another", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({ status: tailscaleOn() });
    const { onPaired } = open(daemon);
    await screen.findByRole("timer");

    await tick(5_000);
    expect(screen.getByRole("timer")).toHaveTextContent("2:55");
    // The laptop that was already paired is not the phone.
    expect(screen.queryByText(/Connected as/)).toBeNull();

    daemon.state.devices = [
      ...daemon.state.devices,
      { device_id: "d2", name: "My phone", kind: "person_device", scopes: ["read", "approve"], created_at: "2026-10-13T00:00:00Z" },
    ];
    await tick(3_100);
    expect(await screen.findByText("Connected as My phone")).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
    expect(onPaired).toHaveBeenCalledWith(expect.objectContaining({ device_id: "d2" }));
    expect(screen.getByRole("link", { name: "See paired devices" })).toHaveAttribute("href", "/devices");

    await user.click(screen.getByRole("button", { name: "Connect another phone" }));
    await screen.findByRole("img");
    expect(pairingRequests(daemon.calls)).toHaveLength(2);
  });

  it("marks an expired code and makes a new one on request", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    let n = 0;
    const daemon = remoteDaemon({
      status: tailscaleOn(),
      offer: () => offer({ code: `CODE${++n}`, url: `https://desk.tail1234.ts.net/pair#CODE${n}`, expires_at: new Date(Date.now() + 60_000).toISOString() }),
    });
    open(daemon);
    expect(await screen.findByText("https://desk.tail1234.ts.net/pair#CODE1")).toBeInTheDocument();

    await tick(61_000);
    expect(screen.getByText("Expired")).toBeInTheDocument();
    expect(screen.getByText("This code expired.")).toBeInTheDocument();
    expect(screen.queryByRole("timer")).toBeNull();
    const polls = daemon.calls.filter((call) => call.path === "/v1/devices").length;
    await tick(6_000);
    expect(daemon.calls.filter((call) => call.path === "/v1/devices").length).toBe(polls);

    await user.click(screen.getByRole("button", { name: "Make a new code" }));
    expect(await screen.findByText("https://desk.tail1234.ts.net/pair#CODE2")).toBeInTheDocument();
    expect(screen.getByRole("timer")).toHaveTextContent("1:00");
  });

  it("refreshes the code before it expires", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    let n = 0;
    const daemon = remoteDaemon({ status: tailscaleOn(), offer: () => offer({ url: `https://desk.tail1234.ts.net/pair#CODE${++n}` }) });
    open(daemon);
    await screen.findByText("https://desk.tail1234.ts.net/pair#CODE1");
    await user.click(screen.getByRole("button", { name: "New code" }));
    expect(await screen.findByText("https://desk.tail1234.ts.net/pair#CODE2")).toBeInTheDocument();
  });

  it("takes the phone's name and permissions from under Phone name and permissions and makes a new code with them", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({ status: tailscaleOn() });
    open(daemon);
    await screen.findByRole("img");

    await user.click(screen.getByRole("button", { name: "Phone name and permissions" }));
    expect(screen.getByLabelText("See conversations and results")).toBeChecked();
    expect(screen.getByLabelText("See conversations and results")).toBeDisabled();
    for (const label of ["Start tasks and send messages", "Approve or reject what the agent asks", "Do steps that need a person, such as logins", "Watch and use the agent’s browser"]) {
      expect(screen.getByLabelText(label)).toBeChecked();
    }
    expect(screen.getByLabelText("Change settings and manage other devices")).not.toBeChecked();

    await user.clear(screen.getByLabelText("Phone name"));
    await user.type(screen.getByLabelText("Phone name"), "Kim's iPhone");
    await user.click(screen.getByLabelText("Start tasks and send messages"));
    await user.click(screen.getByLabelText("Change settings and manage other devices"));
    await user.click(screen.getByRole("button", { name: "Make a new code" }));

    expect(await screen.findByRole("img", { name: "QR code to connect Kim's iPhone" })).toBeInTheDocument();
    expect(pairingRequests(daemon.calls)).toHaveLength(2);
    expect(pairingRequests(daemon.calls)[1]).toEqual({
      name: "Kim's iPhone",
      scopes: ["read", "approve", "human_step", "browse", "admin"],
      kind: "person_device",
      via: "tailscale",
    });
  });

  it("swaps the QR code for the tunnel's state while it is down, and issues a new code for its new address", async () => {
    const daemon = remoteDaemon({
      status: quickTunnel({ state: "running", url: "https://calm-fox.trycloudflare.com" }),
      offer: () => offer({ url: `${daemon.state.status.cloudflare?.url}/pair#CODE` }),
    });
    const { update } = open(daemon, daemon.state.status);
    expect(await screen.findByText("https://calm-fox.trycloudflare.com/pair#CODE")).toBeInTheDocument();

    update(quickTunnel({ state: "restarting", restarts: 1 }));
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("The tunnel isn't reachable yet. Retrying…");

    daemon.state.status = quickTunnel({ state: "running", url: "https://brave-owl.trycloudflare.com", restarts: 1 });
    update(daemon.state.status);
    expect(await screen.findByText("https://brave-owl.trycloudflare.com/pair#CODE")).toBeInTheDocument();
    expect(pairingRequests(daemon.calls)).toHaveLength(2);
  });

  it("shows a pairing refusal, re-reads the status and tries again on request", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    let refuse = true;
    const daemon = remoteDaemon({
      status: tailscaleOn(),
      override: (call) => (refuse && call.path === "/v1/pairing" ? json(403, { code: "local_access_required", message: "no" }) : undefined),
    });
    const { onStatusStale } = open(daemon);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Reopen openside using the desktop launcher");
    await waitFor(() => expect(onStatusStale).toHaveBeenCalled());
    expect(screen.queryByRole("img")).toBeNull();
    // A refusal is not retried on its own.
    await tick(5_000);
    expect(daemon.calls.filter((call) => call.path === "/v1/pairing")).toHaveLength(1);

    refuse = false;
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("img")).toBeInTheDocument();
  });
});
