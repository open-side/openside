import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Device } from "../api/types";
import { savePairing } from "../auth";
import { ClientContext } from "../context";
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
      <ClientContext.Provider value={fake.client}>
        <DevicesPage />
      </ClientContext.Provider>,
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
      <ClientContext.Provider value={daemon().client}>
        <DevicesPage />
      </ClientContext.Provider>,
    );
    const phone = (await screen.findByText("My phone")).closest("li") as HTMLElement;
    await user.click(within(phone).getByRole("button", { name: "Revoke" }));
    expect(await screen.findByRole("dialog", { name: "Revoke My phone?" })).toHaveTextContent("it will be signed out");
  });

  it("lets the operator choose a person's device or an agent before issuing a link", async () => {
    const user = userEvent.setup();
    render(<ClientContext.Provider value={daemon().client}><DevicesPage /></ClientContext.Provider>);
    const person = screen.getByRole("radio", { name: "Pair a phone/browser" });
    expect(person).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText('openside pair --name "My phone"')).toBeInTheDocument();
    expect(screen.getByText(/do not give it to an agent/)).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: "Connect an agent / CLI" }));
    expect(person).toHaveAttribute("aria-checked", "false");
    expect(screen.getByText('openside pair --agent --name "My agent"')).toBeInTheDocument();
    expect(screen.getByText(/must wait for a person’s approval/)).toBeInTheDocument();
    expect(screen.getByText("openside connect")).toBeInTheDocument();
  });
});
