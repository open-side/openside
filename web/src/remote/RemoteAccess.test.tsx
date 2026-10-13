import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteStatus } from "../api/types";
import { ClientContext } from "../context";
import { json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { RemoteAccess } from "./RemoteAccess";
import { offStatus, remoteDaemon, tailscaleOn, type RemoteDaemon } from "./testDaemon";

function open(daemon: RemoteDaemon) {
  render(
    <MemoryRouter>
      <ClientContext.Provider value={daemon.client}>
        <ToastProvider>
          <RemoteAccess />
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
}

const card = async (name: string) => (await screen.findByRole("heading", { name: new RegExp(name) })).closest("article") as HTMLElement;

/** The last body sent with `method path`. */
function sent(daemon: RemoteDaemon, method: string, path: string): unknown {
  return daemon.calls.filter((call) => call.method === method && call.path === path).at(-1)?.body;
}

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
});
afterEach(() => {
  vi.useRealTimers();
  sessionStorage.clear();
});

describe("RemoteAccess", () => {
  it("lists the three connections off, recommends Tailscale where it is installed, and says no phone can connect yet", async () => {
    open(remoteDaemon({ status: offStatus() }));
    const tailscale = await card("Tailscale");
    expect(within(tailscale).getByText("Recommended")).toBeInTheDocument();
    expect(within(tailscale).getByText("Off")).toBeInTheDocument();
    expect(within(tailscale).getByText(/Tailscale is already on this computer/)).toBeInTheDocument();
    expect(within(await card("Cloudflare Tunnel")).queryByText("Recommended")).toBeNull();
    expect(within(await card("Your own relay")).getByText("Off")).toBeInTheDocument();
    expect(screen.getByText(/Turn on a connection first/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "See paired devices" })).toHaveAttribute("href", "/devices");
  });

  it("recommends the relay when Tailscale is missing and warns that Tailscale needs installing", async () => {
    open(remoteDaemon({ status: offStatus({ tailscale: { installed: false, state: "unavailable", url: null, last_error: null } }) }));
    expect(within(await card("Your own relay")).getByText("Recommended")).toBeInTheDocument();
    expect(within(await card("Tailscale")).getByText(/Install the Tailscale app/)).toBeInTheDocument();
  });

  it("turns Tailscale on and shows its address with a working phone flow", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({
      status: offStatus(),
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/tailscale" ? json(200, tailscaleOn()) : undefined),
    });
    open(daemon);
    await user.click(within(await card("Tailscale")).getByRole("button", { name: "Turn on Tailscale" }));
    expect(sent(daemon, "PUT", "/v1/remote/tailscale")).toEqual({ enabled: true });
    const tailscale = await card("Tailscale");
    expect(await within(tailscale).findByText("On")).toBeInTheDocument();
    expect(within(tailscale).getByText("https://desk.tail1234.ts.net")).toBeInTheDocument();
    expect(within(tailscale).getByRole("button", { name: "Copy the Tailscale address" })).toBeInTheDocument();
    expect(within(tailscale).getByRole("button", { name: "Turn off Tailscale" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show QR code" })).toBeInTheDocument();
  });

  it("starts a quick Cloudflare tunnel, shows it starting and then its public address", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const config = { ...offStatus().configuration, cloudflare: { enabled: true, tunnel: "quick" as const, hostname: null, token_file: null } };
    const starting = offStatus({ configuration: config, cloudflare: { state: "starting", url: null, restarts: 0, last_error: null } });
    const daemon = remoteDaemon({
      status: offStatus(),
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/cloudflare" ? json(200, starting) : undefined),
    });
    open(daemon);
    await user.click(within(await card("Cloudflare Tunnel")).getByRole("button", { name: "Turn on Cloudflare Tunnel" }));
    expect(sent(daemon, "PUT", "/v1/remote/cloudflare")).toEqual({ enabled: true, tunnel: "quick", hostname: null, token_file: null });
    expect(await within(await card("Cloudflare Tunnel")).findByText(/Starting… this can take/)).toBeInTheDocument();

    const running: RemoteStatus = {
      ...starting,
      modes: ["cloudflare"],
      cloudflare: { state: "running", url: "https://calm-fox.trycloudflare.com", restarts: 0, last_error: null },
    };
    daemon.state.status = running;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4_100);
    });
    const cloudflare = await card("Cloudflare Tunnel");
    expect(await within(cloudflare).findByText("https://calm-fox.trycloudflare.com")).toBeInTheDocument();
    expect(within(cloudflare).getByText("On")).toBeInTheDocument();
  });

  it("needs a hostname for a named tunnel and sends the token file path, never a token", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({ status: offStatus() });
    open(daemon);
    const cloudflare = await card("Cloudflare Tunnel");
    await user.click(within(cloudflare).getByRole("radio", { name: "Your domain" }));
    await user.click(within(cloudflare).getByRole("button", { name: "Turn on Cloudflare Tunnel" }));
    expect(within(cloudflare).getByRole("alert")).toHaveTextContent("Enter the hostname");
    expect(daemon.calls.some((call) => call.method === "PUT")).toBe(false);

    await user.type(within(cloudflare).getByLabelText("Hostname"), "desk.example.com");
    await user.type(within(cloudflare).getByLabelText(/Token file/), "/home/me/.cf-token");
    await user.click(within(cloudflare).getByRole("button", { name: "Turn on Cloudflare Tunnel" }));
    expect(sent(daemon, "PUT", "/v1/remote/cloudflare")).toEqual({
      enabled: true,
      tunnel: "named",
      hostname: "desk.example.com",
      token_file: "/home/me/.cf-token",
    });
  });

  it("requires a relay address, then registers the relay with its token file", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({ status: offStatus() });
    open(daemon);
    const relay = await card("Your own relay");
    await user.click(within(relay).getByRole("button", { name: "Turn on Your own relay" }));
    expect(within(relay).getByRole("alert")).toHaveTextContent("Enter the address of your relay");

    await user.type(within(relay).getByLabelText("Relay address"), "https://relay.example.com");
    await user.click(within(relay).getByRole("button", { name: "Turn on Your own relay" }));
    expect(sent(daemon, "PUT", "/v1/remote/relay")).toEqual({ enabled: true, url: "https://relay.example.com", token_file: null });
  });

  it("shows a failed tunnel with the daemon's reason and a way to retry", async () => {
    const config = { ...offStatus().configuration, cloudflare: { enabled: true, tunnel: "quick" as const, hostname: null, token_file: null } };
    const status = offStatus({ configuration: config, cloudflare: { state: "failed", url: null, restarts: 3, last_error: "cloudflared: executable not found" } });
    open(remoteDaemon({ status }));
    const cloudflare = await card("Cloudflare Tunnel");
    expect(within(cloudflare).getByText("Needs attention")).toBeInTheDocument();
    expect(within(cloudflare).getByRole("alert")).toHaveTextContent(/executable not found.*installed/);
    expect(within(cloudflare).getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });

  it("shows the daemon's refusal on the card", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const daemon = remoteDaemon({
      status: offStatus(),
      override: (call) => (call.method === "PUT" ? json(503, { code: "transport_unavailable", message: "serve unsupported" }) : undefined),
    });
    open(daemon);
    const tailscale = await card("Tailscale");
    await user.click(within(tailscale).getByRole("button", { name: "Turn on Tailscale" }));
    expect(await within(tailscale).findByRole("alert")).toHaveTextContent("That connection cannot start yet");
  });

  it("tells the person to restart openside when a change is saved but not live", async () => {
    open(remoteDaemon({ status: tailscaleOn({ restart_required: true }) }));
    expect(await screen.findByText(/Restart openside to finish turning this on/)).toBeInTheDocument();
  });

  it("is read-only away from the computer: states stay visible, controls and the phone flow become instructions", async () => {
    open(remoteDaemon({ status: tailscaleOn({ configure_allowed: false, pairing_allowed: false }) }));
    const tailscale = await card("Tailscale");
    expect(within(tailscale).getByText("On")).toBeInTheDocument();
    expect(within(tailscale).queryByRole("button", { name: /Turn (on|off)/ })).toBeNull();
    expect(screen.getByText(/To change these connections, use openside on the computer that runs it/)).toBeInTheDocument();
    expect(screen.getByText(/To connect a phone, use openside on the computer that runs it/)).toBeInTheDocument();
  });

  it("shows the scope notice when this device cannot read remote status", async () => {
    open(remoteDaemon({ override: (call) => (call.path === "/v1/remote/status" ? json(403, { code: "forbidden", message: "no" }) : undefined) }));
    expect(await screen.findByRole("region", { name: "Access needed" })).toBeInTheDocument();
  });
});
