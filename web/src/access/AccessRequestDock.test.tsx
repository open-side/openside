import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AccessRequest } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { AccessRequestDock } from "./AccessRequestDock";

const REQUEST: AccessRequest = { id: "r1", device_id: "d9", name: "My phone", scopes: ["admin"], created_at: "2026-10-13T00:00:00Z", expires_at: "2026-10-13T00:10:00Z" };

function daemon(requests: AccessRequest[], extra?: (call: Call) => Response | undefined) {
  const open = [...requests];
  return fakeClient((call: Call) => {
    const forced = extra?.(call);
    if (forced) return forced;
    if (call.method === "GET" && call.path === "/v1/access-requests") return json(200, { requests: open });
    if (call.method === "POST" && call.path === "/v1/access-requests/r1/approve") return json(200, { device_id: "d9", name: "My phone", kind: "person_device", scopes: ["read", "admin"], created_at: "2026-10-01T00:00:00Z" });
    if (call.method === "POST" && call.path === "/v1/access-requests/r1/deny") return json(204, undefined);
    return json(404, { code: "not_found", message: "nope" });
  });
}

function show(client: ReturnType<typeof daemon>["client"]) {
  render(
    <ClientContext.Provider value={client}>
      <AccessRequestDock />
    </ClientContext.Provider>,
  );
}

beforeEach(() => localStorage.clear());
afterEach(cleanup);

describe("AccessRequestDock", () => {
  it("shows nothing when nobody is asking", async () => {
    const fake = daemon([]);
    show(fake.client);
    await waitFor(() => expect(fake.calls.some((call) => call.path === "/v1/access-requests")).toBe(true));
    expect(screen.queryByRole("region", { name: "Access requests" })).toBeNull();
  });

  it("shows who is asking for what and allows it with one click", async () => {
    const user = userEvent.setup();
    const fake = daemon([REQUEST]);
    show(fake.client);
    const card = await screen.findByRole("region", { name: "Access requests" });
    expect(card).toHaveTextContent("My phone is asking for access");
    expect(card).toHaveTextContent("manage openside");
    await user.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Access requests" })).toBeNull());
    expect(fake.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/access-requests/r1/approve" }));
  });

  it("declines a request", async () => {
    const user = userEvent.setup();
    const fake = daemon([REQUEST]);
    show(fake.client);
    await user.click(await screen.findByRole("button", { name: "Decline" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Access requests" })).toBeNull());
    expect(fake.calls).toContainEqual(expect.objectContaining({ method: "POST", path: "/v1/access-requests/r1/deny" }));
  });

  it("explains a refusal, such as approving from a device that is not the computer", async () => {
    const user = userEvent.setup();
    const fake = daemon([REQUEST], (call) => (call.path.endsWith("/approve") ? json(403, { code: "local_access_required", message: "local" }) : undefined));
    show(fake.client);
    await user.click(await screen.findByRole("button", { name: "Allow" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Access requests" })).toBeInTheDocument();
  });

  it("stops asking when this device may not list requests", async () => {
    const fake = daemon([], (call) => (call.path === "/v1/access-requests" ? json(403, { code: "forbidden", message: "no" }) : undefined));
    show(fake.client);
    await waitFor(() => expect(fake.calls).toHaveLength(1));
    expect(screen.queryByRole("region", { name: "Access requests" })).toBeNull();
  });
});
