import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccessRequest, MyDevice } from "../api/types";
import { mayUse } from "../auth";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ScopeNotice } from "./ScopeNotice";

const REQUEST: AccessRequest = { id: "r1", device_id: "d1", name: "Phone", scopes: ["admin"], created_at: "2026-10-13T00:00:00Z", expires_at: "2026-10-13T00:10:00Z" };

function daemon(patch: Partial<MyDevice> = {}, extra?: (call: Call) => Response | undefined) {
  const me: MyDevice = { device_id: "d1", name: "Phone", kind: "person_device", scopes: ["read"], request: null, ...patch };
  const fake = fakeClient((call: Call) => {
    const forced = extra?.(call);
    if (forced) return forced;
    if (call.method === "GET" && call.path === "/v1/devices/me") return json(200, me);
    if (call.method === "POST" && call.path === "/v1/devices/me/access-requests") {
      me.request = REQUEST;
      return json(201, REQUEST);
    }
    return json(404, { code: "not_found", message: "nope" });
  });
  return { ...fake, me };
}

function show(client: ReturnType<typeof daemon>["client"], onGranted = vi.fn()) {
  render(
    <ClientContext.Provider value={client}>
      <ScopeNotice what="vault items" onGranted={onGranted} />
    </ClientContext.Provider>,
  );
  return onGranted;
}

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("ScopeNotice", () => {
  it("asks the computer for access instead of telling the person to run a command", async () => {
    const user = userEvent.setup();
    const fake = daemon();
    show(fake.client);
    expect(await screen.findByText("This device can’t manage vault items yet")).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/openside token|--scope|config\.toml/);
    await user.click(screen.getByRole("button", { name: "Ask for access" }));
    expect(await screen.findByRole("status")).toHaveTextContent("approve it on the computer running openside");
    expect(fake.calls.find((call) => call.method === "POST")?.body).toEqual({ scopes: ["admin"] });
  });

  it("notices the approval, remembers the new scopes and tells the screen to reload", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const fake = daemon({ request: REQUEST });
    const onGranted = show(fake.client);
    expect(await screen.findByRole("status")).toHaveTextContent("Waiting for you to approve");
    fake.me.scopes = ["read", "admin"];
    fake.me.request = null;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_100);
    });
    expect(await screen.findByText("This device now has access")).toBeInTheDocument();
    expect(onGranted).toHaveBeenCalled();
    expect(mayUse("admin")).toBe(true);
  });

  it("says so when the request was declined or timed out, and lets the person ask again", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const fake = daemon({ request: REQUEST });
    show(fake.client);
    await screen.findByText(/Waiting for you to approve/);
    fake.me.request = null;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_100);
    });
    await waitFor(() => expect(screen.getByText(/declined or timed out/)).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Ask for access" })).toBeInTheDocument();
  });

  it("points an older daemon's user to Devices rather than a command", async () => {
    const fake = daemon({}, (call) => (call.path.startsWith("/v1/devices/me") ? json(404, { code: "not_found", message: "no" }) : undefined));
    show(fake.client);
    expect(await screen.findByText(/Workspace → Devices/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Ask for access" })).toBeNull();
  });
});
