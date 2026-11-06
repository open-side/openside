import { act, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteStatus } from "../api/types";
import { ClientContext } from "../context";
import { json } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { RemoteControlSection } from "./RemoteAccess";
import { offStatus, quickTunnel, remoteDaemon, tailscaleOn, type RemoteDaemon } from "./testDaemon";

function open(daemon: RemoteDaemon) {
  render(
    <MemoryRouter>
      <ClientContext.Provider value={daemon.client}>
        <ToastProvider>
          <RemoteControlSection />
        </ToastProvider>
      </ClientContext.Provider>
    </MemoryRouter>,
  );
}

const setup = () => userEvent.setup({ advanceTimers: vi.advanceTimersByTime });

/** Opens Advanced, where each connection has its own card. */
async function advanced(user: UserEvent) {
  await user.click(await screen.findByRole("button", { name: "Advanced" }));
}

const card = async (name: string) => (await screen.findByRole("heading", { name: new RegExp(name) })).closest("article") as HTMLElement;

/** The last body sent with `method path`. */
function sent(daemon: RemoteDaemon, method: string, path: string): unknown {
  return daemon.calls.filter((call) => call.method === method && call.path === path).at(-1)?.body;
}

const allow = () => screen.findByRole("switch", { name: "Allow remote sessions on this machine" });

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
});
afterEach(() => {
  vi.useRealTimers();
  sessionStorage.clear();
});

describe("RemoteControlSection", () => {
  it("shows Remote Control with the switch off, the phone and paired-device rows, and Advanced closed", async () => {
    open(remoteDaemon({ status: offStatus({ enabled: false }) }));
    expect(await screen.findByRole("heading", { name: "Remote Control" })).toBeInTheDocument();
    expect(await allow()).not.toBeChecked();
    expect(screen.getByRole("button", { name: /^Connect your phone/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /^Paired devices/ })).toHaveAttribute("href", "/settings/account");
    const toggle = screen.getByRole("button", { name: "Advanced" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("heading", { name: /Tailscale/ })).toBeNull();
  });

  it("turns on Remote Control and then Tailscale with the switch where Tailscale is installed and nothing is saved", async () => {
    const user = setup();
    const daemon = remoteDaemon({
      status: offStatus({ enabled: false }),
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/tailscale" ? json(200, tailscaleOn()) : undefined),
    });
    open(daemon);
    await user.click(await allow());
    expect(await allow()).toBeChecked();
    const order = daemon.calls.filter((call) => call.method !== "GET").map((call) => `${call.method} ${call.path}`);
    expect(order).toEqual(["POST /v1/remote/enable", "PUT /v1/remote/tailscale"]);
    expect(sent(daemon, "PUT", "/v1/remote/tailscale")).toEqual({ enabled: true });
  });

  it("turns on a quick Cloudflare tunnel with the switch when Tailscale is missing and no relay is saved", async () => {
    const user = setup();
    const daemon = remoteDaemon({
      status: offStatus({ enabled: false, tailscale: { installed: false, state: "unavailable", url: null, last_error: null } }),
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/cloudflare" ? json(200, quickTunnel({ state: "starting" })) : undefined),
    });
    open(daemon);
    await user.click(await allow());
    expect(sent(daemon, "PUT", "/v1/remote/cloudflare")).toEqual({ enabled: true, tunnel: "quick", hostname: null, token_file: null });
  });

  it("turns Remote Control off only after a confirmation, keeping every connection's settings", async () => {
    const user = setup();
    const config = { ...tailscaleOn().configuration, relay: { enabled: true, url: "https://relay.example.com", token_file: "/home/me/.relay" } };
    const daemon = remoteDaemon({ status: tailscaleOn({ configuration: config }) });
    open(daemon);
    await user.click(await allow());
    const confirm = await screen.findByRole("dialog", { name: "Turn off remote sessions?" });
    expect(daemon.calls.some((call) => call.method === "POST")).toBe(false);
    await user.click(within(confirm).getByRole("button", { name: "Turn off" }));
    expect(await allow()).not.toBeChecked();
    expect(daemon.calls.some((call) => call.method === "POST" && call.path === "/v1/remote/disable")).toBe(true);
    // One switch for everything: no connection is turned off one by one, so their settings stay.
    expect(daemon.calls.some((call) => call.method === "PUT")).toBe(false);
    expect(daemon.state.status.configuration).toEqual(config);

    // The saved connections are still marked on, yet the switch follows Remote Control alone,
    // and turning it back on asks the daemon to restore them rather than starting one.
    await user.click(await allow());
    expect(await allow()).toBeChecked();
    expect(daemon.calls.filter((call) => call.method !== "GET").map((call) => `${call.method} ${call.path}`)).toEqual(["POST /v1/remote/disable", "POST /v1/remote/enable"]);
  });

  it("names a connection that Remote Control could not stop", async () => {
    const user = setup();
    const daemon = remoteDaemon({
      status: tailscaleOn(),
      override: (call) =>
        call.method === "POST" && call.path === "/v1/remote/disable" ? json(200, { status: { ...tailscaleOn(), enabled: false, modes: [], public_url: null }, still_on: ["tailscale"] }) : undefined,
    });
    open(daemon);
    await user.click(await allow());
    await user.click(within(await screen.findByRole("dialog", { name: "Turn off remote sessions?" })).getByRole("button", { name: "Turn off" }));
    expect(await screen.findByText(/could not stop Tailscale/)).toBeInTheDocument();
    expect(await allow()).not.toBeChecked();
  });

  it("turns Remote Control back on with the saved connections, without turning on another", async () => {
    const user = setup();
    const saved = tailscaleOn({ enabled: false, modes: [], public_url: null });
    const daemon = remoteDaemon({
      status: saved,
      override: (call) => (call.method === "POST" && call.path === "/v1/remote/enable" ? json(200, tailscaleOn()) : undefined),
    });
    open(daemon);
    // Saved settings do not count as on while Remote Control is off.
    expect(await allow()).not.toBeChecked();
    await user.click(await allow());
    expect(await allow()).toBeChecked();
    expect(daemon.calls.some((call) => call.method === "POST" && call.path === "/v1/remote/enable")).toBe(true);
    expect(daemon.calls.some((call) => call.method === "PUT")).toBe(false);
  });

  it("explains how Remote Control works", async () => {
    const user = setup();
    open(remoteDaemon({ status: offStatus() }));
    await user.click(await screen.findByRole("button", { name: "How does Remote Control work?" }));
    const help = await screen.findByRole("dialog", { name: "How Remote Control works" });
    expect(within(help).getByText(/never leave this computer/)).toBeInTheDocument();
  });

  it("opens Connect your phone in a dialog with the pairing QR code", async () => {
    const user = setup();
    open(remoteDaemon({ status: tailscaleOn() }));
    await user.click(await screen.findByRole("button", { name: /^Connect your phone/ }));
    const dialog = await screen.findByRole("dialog", { name: "Connect your phone" });
    expect(await within(dialog).findByRole("img", { name: /QR code to connect/ })).toBeInTheDocument();
  });

  it("lists the three connections under Advanced and recommends Tailscale where it is installed", async () => {
    const user = setup();
    open(remoteDaemon({ status: offStatus() }));
    await advanced(user);
    expect(screen.getByRole("button", { name: "Advanced" })).toHaveAttribute("aria-expanded", "true");
    const tailscale = await card("Tailscale");
    expect(within(tailscale).getByText("Recommended")).toBeInTheDocument();
    expect(within(tailscale).getByText("Off")).toBeInTheDocument();
    expect(within(tailscale).getByText(/Tailscale is already on this computer/)).toBeInTheDocument();
    expect(within(await card("Cloudflare Tunnel")).queryByText("Recommended")).toBeNull();
    expect(within(await card("Your own relay")).getByText("Off")).toBeInTheDocument();
  });

  it("recommends the relay when Tailscale is missing and warns that Tailscale needs installing", async () => {
    const user = setup();
    open(remoteDaemon({ status: offStatus({ tailscale: { installed: false, state: "unavailable", url: null, last_error: null } }) }));
    await advanced(user);
    expect(within(await card("Your own relay")).getByText("Recommended")).toBeInTheDocument();
    expect(within(await card("Tailscale")).getByText(/Install the Tailscale app/)).toBeInTheDocument();
  });

  it("turns Tailscale on from its card and shows its address", async () => {
    const user = setup();
    const daemon = remoteDaemon({
      status: offStatus({ enabled: false }),
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/tailscale" ? json(200, tailscaleOn()) : undefined),
    });
    open(daemon);
    await advanced(user);
    await user.click(within(await card("Tailscale")).getByRole("button", { name: "Turn on Tailscale" }));
    expect(sent(daemon, "PUT", "/v1/remote/tailscale")).toEqual({ enabled: true });
    // The daemon starts no connection while Remote Control is off, so the card turns it on first.
    expect(daemon.calls.filter((call) => call.method !== "GET").map((call) => `${call.method} ${call.path}`)).toEqual(["POST /v1/remote/enable", "PUT /v1/remote/tailscale"]);
    const tailscale = await card("Tailscale");
    expect(await within(tailscale).findByText("On")).toBeInTheDocument();
    expect(within(tailscale).getByText("https://desk.tail1234.ts.net")).toBeInTheDocument();
    expect(within(tailscale).getByRole("button", { name: "Copy the Tailscale address" })).toBeInTheDocument();
    expect(within(tailscale).getByRole("button", { name: "Turn off Tailscale" })).toBeInTheDocument();
    expect(await allow()).toBeChecked();
  });

  it("starts a quick Cloudflare tunnel, shows it starting and then its public address", async () => {
    const user = setup();
    const config = { ...offStatus().configuration, cloudflare: { enabled: true, tunnel: "quick" as const, hostname: null, token_file: null } };
    const starting = offStatus({ configuration: config, cloudflare: { state: "starting", url: null, restarts: 0, last_error: null } });
    const daemon = remoteDaemon({
      status: offStatus(),
      override: (call) => (call.method === "PUT" && call.path === "/v1/remote/cloudflare" ? json(200, starting) : undefined),
    });
    open(daemon);
    await advanced(user);
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
    const user = setup();
    const daemon = remoteDaemon({ status: offStatus() });
    open(daemon);
    await advanced(user);
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
    const user = setup();
    const daemon = remoteDaemon({ status: offStatus() });
    open(daemon);
    await advanced(user);
    const relay = await card("Your own relay");
    await user.click(within(relay).getByRole("button", { name: "Turn on your own relay" }));
    expect(within(relay).getByRole("alert")).toHaveTextContent("Enter the address of your relay");

    await user.type(within(relay).getByLabelText("Relay address"), "https://relay.example.com");
    await user.click(within(relay).getByRole("button", { name: "Turn on your own relay" }));
    expect(sent(daemon, "PUT", "/v1/remote/relay")).toEqual({ enabled: true, url: "https://relay.example.com", token_file: null });
  });

  it("shows a failed tunnel with the daemon's reason and a way to retry", async () => {
    const user = setup();
    const config = { ...offStatus().configuration, cloudflare: { enabled: true, tunnel: "quick" as const, hostname: null, token_file: null } };
    const status = offStatus({ configuration: config, cloudflare: { state: "failed", url: null, restarts: 3, last_error: "cloudflared: executable not found" } });
    open(remoteDaemon({ status }));
    await advanced(user);
    const cloudflare = await card("Cloudflare Tunnel");
    expect(within(cloudflare).getByText("Needs attention")).toBeInTheDocument();
    expect(within(cloudflare).getByRole("alert")).toHaveTextContent(/executable not found.*installed/);
    expect(within(cloudflare).getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });

  it("shows the daemon's refusal on the card", async () => {
    const user = setup();
    const daemon = remoteDaemon({
      status: offStatus(),
      override: (call) => (call.method === "PUT" ? json(503, { code: "transport_unavailable", message: "serve unsupported" }) : undefined),
    });
    open(daemon);
    await advanced(user);
    const tailscale = await card("Tailscale");
    await user.click(within(tailscale).getByRole("button", { name: "Turn on Tailscale" }));
    expect(await within(tailscale).findByRole("alert")).toHaveTextContent("That connection cannot start yet");
  });

  it("shows the daemon's refusal under the switch", async () => {
    const user = setup();
    const daemon = remoteDaemon({
      status: offStatus({ enabled: false }),
      override: (call) => (call.method === "PUT" ? json(503, { code: "transport_unavailable", message: "serve unsupported" }) : undefined),
    });
    open(daemon);
    await user.click(await allow());
    expect(await screen.findByText(/That connection cannot start yet/)).toBeInTheDocument();
    // Remote Control itself did come on; only the connection failed, and the switch says so.
    expect(await allow()).toBeChecked();
  });

  it("tells the person to restart openside when a change is saved but not live", async () => {
    open(remoteDaemon({ status: tailscaleOn({ restart_required: true }) }));
    expect(await screen.findByText(/Restart openside to finish turning this on/)).toBeInTheDocument();
  });

  it("is read-only away from the computer: states stay visible, controls and the phone flow become instructions", async () => {
    const user = setup();
    open(remoteDaemon({ status: tailscaleOn({ configure_allowed: false, pairing_allowed: false }) }));
    expect(await allow()).toBeDisabled();
    expect(screen.getByText(/To change these connections, use openside on the computer that runs it/)).toBeInTheDocument();
    await advanced(user);
    const tailscale = await card("Tailscale");
    expect(within(tailscale).getByText("On")).toBeInTheDocument();
    expect(within(tailscale).queryByRole("button", { name: /Turn (on|off)/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: /^Connect your phone/ }));
    const dialog = await screen.findByRole("dialog", { name: "Connect your phone" });
    expect(within(dialog).getByText(/To connect a phone, use openside on the computer that runs it/)).toBeInTheDocument();
  });

  it("shows the scope notice when this device cannot read remote status", async () => {
    open(remoteDaemon({ override: (call) => (call.path === "/v1/remote/status" ? json(403, { code: "forbidden", message: "no" }) : undefined) }));
    expect(await screen.findByRole("region", { name: "Access needed" })).toBeInTheDocument();
  });
});
