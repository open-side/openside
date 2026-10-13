import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import type { Device } from "../api/types";
import { savePairing } from "../auth";
import { ClientContext } from "../context";
import { offer } from "../remote/testDaemon";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { DevicesPage } from "./DevicesPage";

function daemon() {
  const devices: Device[] = [
    { device_id: "d1", name: "My phone", kind: "person_device", scopes: ["read", "approve", "human_step"], created_at: "2026-10-01T00:00:00Z" },
    { device_id: "d2", name: "Tablet", kind: "person_device", scopes: ["read", "admin"], created_at: "2026-10-02T00:00:00Z" },
  ];
  return fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (url.pathname === "/v1/devices") return json(200, { devices });
    if (call.method === "POST" && url.pathname === "/v1/pairing") return json(200, offer({ kind: "agent_client", scopes: ["read", "steer", "browse", "approve"], url: "http://127.0.0.1:47820/pair#AGENT123" }));
    if (url.pathname.startsWith("/v1/devices/")) {
      const id = url.pathname.split("/").pop();
      devices.splice(devices.findIndex((d) => d.device_id === id), 1);
      return json(204, undefined);
    }
    return json(404, { code: "not_found", message: "nope" });
  });
}

describe("DevicesPage", () => {
  it("lists paired devices, marks this one and revokes another after confirmation", async () => {
    const user = userEvent.setup();
    savePairing("token", "d1", ["read"]);
    const fake = daemon();
    render(
      <MemoryRouter>
        <ClientContext.Provider value={fake.client}>
          <DevicesPage />
        </ClientContext.Provider>
      </MemoryRouter>,
    );
    const list = await screen.findByRole("list", { name: "Devices" });
    const phone = within(list).getByText("My phone").closest("li") as HTMLElement;
    expect(within(phone).getByText("This device")).toBeInTheDocument();
    expect(within(phone).getByLabelText("Scopes")).toHaveTextContent("readapprovehuman_step");

    const tablet = within(list).getByText("Tablet").closest("li") as HTMLElement;
    await user.click(within(tablet).getByRole("button", { name: "Revoke" }));
    const cancel = await screen.findByRole("dialog", { name: "Revoke Tablet?" });
    await user.click(within(cancel).getByRole("button", { name: "Cancel" }));
    expect(fake.calls.some((c) => c.method === "DELETE")).toBe(false);

    await user.click(within(tablet).getByRole("button", { name: "Revoke" }));
    const confirm = await screen.findByRole("dialog", { name: "Revoke Tablet?" });
    await user.click(within(confirm).getByRole("button", { name: "Revoke" }));
    await vi.waitFor(() => expect(within(list).queryByText("Tablet")).toBeNull());
    expect(fake.calls.some((c) => c.method === "DELETE" && c.path === "/v1/devices/d2")).toBe(true);
  });

  it("warns before revoking the device in use", async () => {
    const user = userEvent.setup();
    savePairing("token", "d1", ["read"]);
    render(
      <MemoryRouter>
        <ClientContext.Provider value={daemon().client}>
          <DevicesPage />
        </ClientContext.Provider>
      </MemoryRouter>,
    );
    const phone = (await screen.findByText("My phone")).closest("li") as HTMLElement;
    await user.click(within(phone).getByRole("button", { name: "Revoke" }));
    expect(await screen.findByRole("dialog", { name: "Revoke My phone?" })).toHaveTextContent("it will be signed out");
  });

  it("pairs a phone from the screen and keeps the terminal command as a secondary hint", async () => {
    render(<MemoryRouter><ClientContext.Provider value={daemon().client}><DevicesPage /></ClientContext.Provider></MemoryRouter>);
    expect(screen.getByRole("radio", { name: "Pair a phone/browser" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText(/do not give it to an agent/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Connect a phone with a QR code" })).toHaveAttribute("href", "/remote");
    const hint = screen.getByText('openside pair --name "My phone"').closest("p") as HTMLElement;
    expect(hint).toHaveTextContent("Or from a terminal:");
  });

  it("creates an agent link from the screen, with no command needed to issue it", async () => {
    const user = userEvent.setup();
    const fake = daemon();
    render(<MemoryRouter><ClientContext.Provider value={fake.client}><DevicesPage /></ClientContext.Provider></MemoryRouter>);
    await user.click(screen.getByRole("radio", { name: "Connect an agent / CLI" }));
    expect(screen.queryByText(/openside pair/)).toBeNull();
    await user.clear(screen.getByLabelText("Agent name"));
    await user.type(screen.getByLabelText("Agent name"), "CI box");
    await user.click(screen.getByRole("button", { name: "Create a link" }));
    expect(await screen.findByLabelText("Agent link")).toHaveValue("http://127.0.0.1:47820/pair#AGENT123");
    expect(fake.calls.find((call) => call.path === "/v1/pairing")?.body).toEqual({ name: "CI box", scopes: ["read", "steer", "browse", "approve"], kind: "agent_client", via: null });
    expect(screen.getByRole("status")).toHaveTextContent("expires in");
    expect(screen.getByText("openside connect").closest("p")).toHaveTextContent("On the agent’s computer");
  });

  it("says issuing a link needs the computer itself when the daemon asks for local access", async () => {
    const user = userEvent.setup();
    const fake = fakeClient((call: Call) => (call.path === "/v1/devices" ? json(200, { devices: [] }) : json(403, { code: "local_access_required", message: "local" })));
    render(<MemoryRouter><ClientContext.Provider value={fake.client}><DevicesPage /></ClientContext.Provider></MemoryRouter>);
    await user.click(screen.getByRole("radio", { name: "Connect an agent / CLI" }));
    await user.click(screen.getByRole("button", { name: "Create a link" }));
    expect(await screen.findByText(/use openside on the computer that runs it/)).toBeInTheDocument();
  });
});
